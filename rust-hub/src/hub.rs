use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use chrono::{Local, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::backups::{BackupEntry, BackupStore, GitHubBackups};
use crate::contract;
use crate::github::GitHubCatalog;
use crate::installer::PackageInstaller;
use crate::models::{Catalog, CatalogApp, HubReleaseInfo, HubSettings, InstalledApp};
use crate::process::{ModuleProcess, ModuleProcessGuard};
use crate::store::HubStore;
use crate::system::{GitHubCredentials, StandaloneLauncher, WindowsInstallation};
use crate::updater::HubUpdater;
use crate::util;

pub enum UserEvent {
    Message(String),
    Deliver(String),
    Execute(String),
    Open(String),
    Exit,
    TrayRestore,
    TrayExit,
    MapHost {
        host: String,
        root: PathBuf,
        reply: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    },
    Fullscreen {
        enabled: bool,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Print {
        request: crate::printer::PrintRequest,
        reply: String,
    },
    PrintSignal(u64, crate::printer::PrintSignal),
}

pub struct Hub {
    pub store: HubStore,
    pub github: GitHubCatalog,
    pub installer: PackageInstaller,
    proxy: Mutex<tao::event_loop::EventLoopProxy<UserEvent>>,
    catalog: Mutex<Catalog>,
    hub_update: Mutex<Option<HubReleaseInfo>>,
    workers: tokio::sync::Mutex<HashMap<String, Arc<ModuleProcess>>>,
    module_gates: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    downloads: Mutex<HashMap<String, CancellationToken>>,
    removing: Mutex<HashSet<String>>,
    active_module: Mutex<Option<String>>,
    pub module_dirty: AtomicBool,
    refresh_gate: tokio::sync::Mutex<()>,
    hub_gate: tokio::sync::Mutex<()>,
    backup_gate: tokio::sync::Mutex<()>,
    bundled_catalog: PathBuf,
    pub closing: AtomicBool,
    pub exit_requested: AtomicBool,
}

impl Hub {
    pub fn new(
        store: HubStore,
        proxy: tao::event_loop::EventLoopProxy<UserEvent>,
        bundled_catalog: PathBuf,
    ) -> Result<Self> {
        let github = GitHubCatalog::new()?;
        let catalog = github.load_cached(&store, &bundled_catalog)?;
        Ok(Self {
            store,
            github,
            installer: PackageInstaller::new(),
            proxy: Mutex::new(proxy),
            catalog: Mutex::new(catalog),
            hub_update: Mutex::new(None),
            workers: tokio::sync::Mutex::new(HashMap::new()),
            module_gates: tokio::sync::Mutex::new(HashMap::new()),
            downloads: Mutex::new(HashMap::new()),
            removing: Mutex::new(HashSet::new()),
            active_module: Mutex::new(None),
            module_dirty: AtomicBool::new(false),
            refresh_gate: tokio::sync::Mutex::new(()),
            hub_gate: tokio::sync::Mutex::new(()),
            backup_gate: tokio::sync::Mutex::new(()),
            bundled_catalog,
            closing: AtomicBool::new(false),
            exit_requested: AtomicBool::new(false),
        })
    }

    pub fn emit<T: Serialize>(&self, value: &T) {
        let Ok(text) = serde_json::to_string(value) else {
            return;
        };
        self.send_text(text);
    }

    pub fn send_text(&self, text: String) {
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let proxy = self.proxy.lock().unwrap_or_else(|e| e.into_inner());
        let _ = proxy.send_event(UserEvent::Deliver(text));
    }

    pub fn execute_script(&self, script: String) {
        let proxy = self.proxy.lock().unwrap_or_else(|e| e.into_inner());
        let _ = proxy.send_event(UserEvent::Execute(script));
    }

    pub async fn map_host(&self, host: String, root: PathBuf) -> Result<()> {
        let (reply, wait) = tokio::sync::oneshot::channel();
        {
            let proxy = self.proxy.lock().unwrap_or_else(|e| e.into_inner());
            proxy
                .send_event(UserEvent::MapHost { host, root, reply })
                .map_err(|_| anyhow!("Окно хаба закрыто."))?;
        }
        wait.await
            .map_err(|_| anyhow!("Окно хаба закрыто."))?
            .map_err(|error| anyhow!(error))
    }

    pub async fn set_fullscreen(&self, enabled: bool) -> bool {
        let (reply, wait) = tokio::sync::oneshot::channel();
        {
            let proxy = self.proxy.lock().unwrap_or_else(|e| e.into_inner());
            if proxy
                .send_event(UserEvent::Fullscreen { enabled, reply })
                .is_err()
            {
                return enabled;
            }
        }
        wait.await.unwrap_or(enabled)
    }

    pub fn request_restart(&self) {
        let proxy = self.proxy.lock().unwrap_or_else(|e| e.into_inner());
        let _ = proxy.send_event(UserEvent::TrayExit);
    }

    pub fn catalog(&self) -> Catalog {
        self.catalog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn catalog_app(&self, id: &str) -> Option<CatalogApp> {
        self.catalog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .apps
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    pub fn catalog_name(&self, id: &str) -> String {
        self.catalog_app(id)
            .map(|a| a.name)
            .unwrap_or_else(|| id.to_string())
    }

    pub fn active_module(&self) -> Option<String> {
        self.active_module
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_active_module(&self, value: Option<String>) {
        *self
            .active_module
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = value;
    }

    pub fn is_busy(&self, id: &str) -> bool {
        self.downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
            || self
                .removing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(id)
    }

    pub fn has_running_operations(&self) -> bool {
        !self
            .downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
            || !self
                .removing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }

    pub fn snapshot(&self) -> Value {
        let hub_update = self
            .hub_update
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        json!({
            "catalog": self.catalog(),
            "settings": self.store.settings_view(),
            "dataRoot": self.store.data_root().to_string_lossy(),
            "hubVersion": util::host_version(),
            "hubUpdate": hub_update,
            "apiVersion": contract::API_VERSION,
            "native": true,
        })
    }

    pub async fn handle(&self, method: &str, params: Option<Value>) -> std::result::Result<Value, String> {
        match self.handle_inner(method, params).await {
            Ok(value) => Ok(value),
            Err(error) => Err(format!("{error}")),
        }
    }

    async fn handle_inner(&self, method: &str, params: Option<Value>) -> Result<Value> {
        if method.starts_with("backups.") {
            return self.handle_backup(method, params).await;
        }
        let p = params.unwrap_or(Value::Null);
        match method {
            "hub.state" => Ok(self.snapshot()),
            "hub.ready" => {
                for installed in self.store.settings().installed {
                    if !self.store.app_present(&installed) {
                        continue;
                    }
                    let name = self.catalog_name(&installed.id);
                    if let Err(error) = WindowsInstallation::register(&self.store, &installed.id, &name)
                    {
                        self.emit(&json!({
                            "event": "notice",
                            "text": format!("Не удалось зарегистрировать приложение Windows: {error}"),
                        }));
                    }
                }
                Ok(Value::Bool(true))
            }
            "catalog.refresh" => {
                let _gate = self.refresh_gate.lock().await;
                let previous = self.catalog();
                let catalog = self.github.refresh(&self.store, &previous).await?;
                *self.catalog.lock().unwrap_or_else(|e| e.into_inner()) = catalog;
                Ok(self.snapshot())
            }
            "settings.save" => {
                let theme = get_str(&p, "theme")?;
                if !matches!(theme.as_str(), "light" | "dark" | "system") {
                    return Err(anyhow!("Неизвестная тема."));
                }
                let check_on_startup = get_bool(&p, "checkOnStartup")?;
                let auto = p
                    .get("autoInstallUpdates")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                self.store.change(|settings| {
                    settings.theme = theme.clone();
                    settings.check_on_startup = check_on_startup;
                    settings.auto_install_updates = auto;
                })?;
                Ok(self.snapshot())
            }
            "sidebar.order" => {
                let order: Vec<String> = p
                    .get("order")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| anyhow!("Некорректный порядок."))?
                    .iter()
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .collect();
                let installed = self.store.settings().installed;
                let distinct: HashSet<&String> = order.iter().collect();
                if order.len() != installed.len()
                    || distinct.len() != order.len()
                    || order.iter().any(|id| !installed.iter().any(|a| &a.id == id))
                {
                    return Err(anyhow!(
                        "Порядок должен содержать все установленные программы."
                    ));
                }
                self.store.change(|settings| settings.sidebar_order = order)?;
                Ok(Value::Bool(true))
            }
            "hub.openData" => {
                util::open_path(&self.store.data_root())?;
                Ok(Value::Bool(true))
            }
            "hub.releases" => {
                util::open_url("https://github.com/ThodProd/ThodHUB/releases/latest")?;
                Ok(Value::Bool(true))
            }
            "hub.check" => {
                let _gate = self.hub_gate.lock().await;
                let quiet = p.get("quiet").and_then(|v| v.as_bool()).unwrap_or(false);
                match self.github.check_hub_update(util::host_version()).await {
                    Ok(update) => {
                        *self.hub_update.lock().unwrap_or_else(|e| e.into_inner()) = update;
                    }
                    Err(error) => {
                        if !quiet {
                            return Err(anyhow!("Не удалось проверить обновления хаба. {error}"));
                        }
                    }
                }
                Ok(self.snapshot())
            }
            "hub.installUpdate" => self.install_hub_update().await,
            "hub.openUpdate" => {
                let update = self
                    .hub_update
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .ok_or_else(|| anyhow!("Сведения об обновлении хаба ещё не получены."))?;
                util::open_url(&update.release_url)?;
                Ok(Value::Bool(true))
            }
            "hub.prompt" => {
                let path = hub_directory().join("agent-prompt.md");
                Ok(Value::String(std::fs::read_to_string(path)?))
            }
            "hub.fullscreen" => {
                let enabled = p
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(json!(self.set_fullscreen(enabled).await))
            }
            "hub.restart" => {
                self.request_restart();
                Ok(Value::Bool(true))
            }
            "clipboard.copy" => {
                util::copy_text(&p.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string())?;
                Ok(Value::Bool(true))
            }
            "app.install" => self.install(&get_str(&p, "id")?).await,
            "app.open" => self.open_module(&get_str(&p, "id")?).await,
            "app.cancel" => {
                let id = get_str(&p, "id")?;
                let token = self
                    .downloads
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&id)
                    .cloned();
                if let Some(token) = token {
                    token.cancel();
                }
                Ok(Value::Bool(true))
            }
            "app.close" => {
                if !util::confirm_discard(self.module_dirty.load(Ordering::SeqCst)) {
                    return Err(anyhow!("Сохраните проект перед закрытием программы."));
                }
                self.module_dirty.store(false, Ordering::SeqCst);
                self.set_active_module(None);
                Ok(Value::Bool(true))
            }
            "app.uninstall" => self.uninstall(&get_str(&p, "id")?, &p).await,
            "app.rollback" => self.rollback(&get_str(&p, "id")?).await,
            "app.info" => {
                let manifest = self.store.manifest(&get_str(&p, "id")?)?;
                Ok(json!({
                    "actions": manifest.actions,
                    "integration": manifest.integration.is_some(),
                }))
            }
            "app.launch" => {
                let id = get_str(&p, "id")?;
                if self.is_busy(&id) {
                    return Err(anyhow!("Программа сейчас обновляется."));
                }
                StandaloneLauncher::launch(&self.store, &id, &[])?;
                Ok(Value::Bool(true))
            }
            "app.action" => {
                let id = get_str(&p, "id")?;
                if self.is_busy(&id) {
                    return Err(anyhow!("Программа сейчас обновляется."));
                }
                let action_id = get_str(&p, "action")?;
                let manifest = self.store.manifest(&id)?;
                let action = manifest
                    .actions
                    .iter()
                    .find(|a| a.id == action_id)
                    .cloned()
                    .ok_or_else(|| anyhow!("Действие программы не найдено."))?;
                if action.wait_for_exit {
                    let launch = manifest
                        .launch
                        .as_ref()
                        .ok_or_else(|| anyhow!("У программы нет исполняемой части."))?;
                    let mut arguments = launch.arguments.clone();
                    arguments.extend(action.arguments.clone());
                    let hook = crate::models::WorkerDefinition {
                        executable: launch.executable.clone(),
                        arguments,
                    };
                    StandaloneLauncher::run_hook(&self.store, &id, &manifest, Some(&hook)).await?;
                } else {
                    StandaloneLauncher::launch(&self.store, &id, &action.arguments)?;
                }
                Ok(Value::Bool(true))
            }
            "app.integrate" => {
                let id = get_str(&p, "id")?;
                if self.is_busy(&id) {
                    return Err(anyhow!("Дождитесь завершения установки."));
                }
                StandaloneLauncher::integrate(&self.store, &id).await?;
                self.store.change(|settings| {
                    if let Some(app) = settings.installed.iter_mut().find(|a| a.id == id) {
                        app.integration_error = None;
                    }
                })?;
                Ok(self.snapshot())
            }
            "app.shortcut" => {
                let id = get_str(&p, "id")?;
                let name = self.catalog_name(&id);
                StandaloneLauncher::create_shortcut(&self.store, &id, &name)?;
                Ok(Value::Bool(true))
            }
            "app.openData" => {
                let id = get_str(&p, "id")?;
                self.store.manifest(&id)?;
                util::open_path(&self.store.data_directory(&id)?)?;
                Ok(Value::Bool(true))
            }
            "module.call" => self.module_call(&p).await,
            _ => Err(anyhow!("Неизвестная команда хаба.")),
        }
    }

    // -----------------------------------------------------------------------
    // Install / uninstall / rollback
    // -----------------------------------------------------------------------

    async fn open_module(&self, id: &str) -> Result<Value> {
        if self.is_busy(id) {
            return Err(anyhow!("Программа обновляется."));
        }
        let manifest = self.store.manifest(id)?;
        if !util::confirm_discard(self.module_dirty.load(Ordering::SeqCst)) {
            return Err(anyhow!(
                "Сохраните проект перед переключением программы."
            ));
        }
        self.clear_dirty();
        if manifest.mode == "standalone" {
            self.set_active_module(None);
            return Ok(json!({
                "mode": "standalone",
                "id": id,
                "version": manifest.version,
                "requiresElevation": manifest.requires_elevation,
                "actions": manifest.actions,
            }));
        }
        let root = self.store.app_version(id, &manifest.version)?;
        // A versioned origin invalidates HTML, scripts and styles together after an update.
        let host = util::module_host(id, &manifest.version);
        self.map_host(host.clone(), root).await?;
        self.set_active_module(Some(id.to_string()));
        Ok(json!({
            "mode": "embedded",
            "url": format!("https://{host}/{}", manifest.ui.clone().unwrap_or_default()),
            "id": id,
            "version": manifest.version,
            "permissions": manifest.permissions,
        }))
    }

    /// Validates a document.print/document.pdf bridge call on the UI thread.
    pub fn document_request(&self, p: &Value) -> Result<crate::printer::PrintRequest> {
        let id = get_str(p, "appId")?;
        if self.active_module().as_deref() != Some(id.as_str()) || self.is_busy(&id) {
            return Err(anyhow!("Программа не открыта."));
        }
        let method = get_str(p, "method")?;
        let to_pdf = match method.as_str() {
            "document.pdf" => true,
            "document.print" => false,
            _ => return Err(anyhow!("Неизвестная команда модуля.")),
        };
        let manifest = self.store.manifest(&id)?;
        if !manifest.permissions.iter().any(|p| p == &method) {
            return Err(anyhow!("Не объявлено разрешение: {method}"));
        }
        let args = p.get("params").cloned().unwrap_or(Value::Null);
        let html = args
            .get("html")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Некорректный документ."))?
            .to_string();
        if html.encode_utf16().count() > 8 * 1024 * 1024 {
            return Err(anyhow!("Документ превышает 8 МБ."));
        }
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Некорректный документ."))?
            .to_string();
        Ok(crate::printer::PrintRequest {
            html,
            name,
            module_root: self.store.app_version(&id, &manifest.version)?,
            data_root: self.store.root().join("WebView"),
            to_pdf,
        })
    }

    async fn install(&self, id: &str) -> Result<Value> {
        if self.is_busy(id) {
            return Err(anyhow!("Установка уже выполняется."));
        }
        if self.active_module().as_deref() == Some(id) {
            return Err(anyhow!("Закройте программу перед обновлением."));
        }
        let app = self
            .catalog_app(id)
            .ok_or_else(|| anyhow!("Программа не найдена в каталоге."))?;
        let cts = CancellationToken::new();
        self.downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), cts.clone());
        let mut path: Option<PathBuf> = None;
        let result = async {
            self.dispose_worker(id).await;
            if self.store.installed(id).is_some() {
                StandaloneLauncher::stop_for_maintenance(&self.store, id).await?;
            }
            let downloaded = self
                .github
                .download(
                    &self.store,
                    &app,
                    |progress| {
                        self.emit(&json!({
                            "event": "install.progress",
                            "id": id,
                            "progress": progress,
                            "stage": "download",
                        }));
                    },
                    &cts,
                )
                .await?;
            path = Some(downloaded.clone());
            self.emit(&json!({
                "event": "install.progress",
                "id": id,
                "progress": 1.0,
                "stage": "verify",
            }));
            let manifest = self
                .installer
                .install(&self.store, &downloaded, &app, &cts)
                .await?;
            self.emit(&json!({
                "event": "install.progress",
                "id": id,
                "progress": 1.0,
                "stage": "integrate",
            }));
            if let Err(error) = StandaloneLauncher::integrate(&self.store, id).await {
                self.store.change(|settings| {
                    if let Some(installed) = settings.installed.iter_mut().find(|a| a.id == id) {
                        installed.integration_error = Some(format!("{error}"));
                    }
                })?;
                return Err(anyhow!(
                    "Версия {} установлена, но интеграция Windows не завершена. Повторите её в сведениях программы. {}",
                    manifest.version,
                    error
                ));
            }
            let version = self.store.manifest(id)?.version;
            let fresh = HubStore::new(self.store.root())?;
            if version != manifest.version || fresh.manifest(id)?.version != manifest.version {
                return Err(anyhow!(
                    "Версия не активировалась. Откройте сведения и повторите установку."
                ));
            }
            if let Err(error) = StandaloneLauncher::create_shortcut(&self.store, id, &app.name) {
                self.emit(&json!({
                    "event": "notice",
                    "text": format!("Программа установлена, но ярлык не создан: {error}"),
                }));
            }
            WindowsInstallation::register(&self.store, id, &app.name)?;
            Ok(self.snapshot())
        }
        .await;
        self.downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if let Some(path) = path {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
        result
    }

    async fn uninstall(&self, id: &str, params: &Value) -> Result<Value> {
        if self.active_module().as_deref() == Some(id)
            && !util::confirm_discard(self.module_dirty.load(Ordering::SeqCst))
        {
            return Err(anyhow!(
                "Сохраните проект перед удалением или откатом программы."
            ));
        }
        if self
            .downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
            || !self
                .removing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.to_string())
        {
            return Err(anyhow!("Дождитесь завершения операции."));
        }
        let result = async {
            self.dispose_worker(id).await;
            let app = self
                .store
                .installed(id)
                .ok_or_else(|| anyhow!("Программа не установлена."))?;
            let present = self.store.app_present(&app);
            StandaloneLauncher::stop_for_maintenance(&self.store, id).await?;
            if self.active_module().as_deref() == Some(id) {
                self.set_active_module(None);
                self.module_dirty.store(false, Ordering::SeqCst);
            }
            if params.get("backup").and_then(|v| v.as_bool()).unwrap_or(false) {
                self.create_backup(id, true).await?;
            }
            if present {
                StandaloneLauncher::cleanup(&self.store, id).await?;
            }
            let delete_data = params
                .get("deleteData")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            self.installer.uninstall(&self.store, id, delete_data).await?;
            for warning in crate::system::remove_registrations(&self.store, &app) {
                self.emit(&json!({ "event": "notice", "text": warning }));
            }
            Ok(self.snapshot())
        }
        .await;
        self.removing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        result
    }

    async fn rollback(&self, id: &str) -> Result<Value> {
        if self.active_module().as_deref() == Some(id)
            && !util::confirm_discard(self.module_dirty.load(Ordering::SeqCst))
        {
            return Err(anyhow!(
                "Сохраните проект перед удалением или откатом программы."
            ));
        }
        if self
            .downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
            || !self
                .removing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.to_string())
        {
            return Err(anyhow!("Дождитесь завершения операции."));
        }
        let result = async {
            self.dispose_worker(id).await;
            let app = self
                .store
                .installed(id)
                .ok_or_else(|| anyhow!("Программа не установлена."))?;
            if !self.store.app_present(&app) {
                return Err(anyhow!(
                    "Файлы программы не найдены. Скачайте программу заново."
                ));
            }
            StandaloneLauncher::stop_for_maintenance(&self.store, id).await?;
            if self.active_module().as_deref() == Some(id) {
                self.set_active_module(None);
                self.module_dirty.store(false, Ordering::SeqCst);
            }
            StandaloneLauncher::cleanup(&self.store, id).await?;
            self.installer.rollback(&self.store, id).await?;
            StandaloneLauncher::integrate(&self.store, id).await?;
            let name = self.catalog_name(id);
            WindowsInstallation::register(&self.store, id, &name)?;
            StandaloneLauncher::create_shortcut(&self.store, id, &name)?;
            Ok(self.snapshot())
        }
        .await;
        self.removing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        result
    }

    async fn dispose_worker(&self, id: &str) {
        let worker = self
            .workers
            .lock()
            .await
            .remove(id);
        if let Some(worker) = worker {
            worker.dispose().await;
        }
    }

    // -----------------------------------------------------------------------
    // Module bridge
    // -----------------------------------------------------------------------

    async fn module_call(&self, p: &Value) -> Result<Value> {
        let id = get_str(p, "appId")?;
        if self.active_module().as_deref() != Some(id.as_str()) || self.is_busy(&id) {
            return Err(anyhow!("Программа не открыта."));
        }
        let method = get_str(p, "method")?;
        let args = p.get("params").cloned().unwrap_or(Value::Null);
        let manifest = self.store.manifest(&id)?;
        let require = |permission: &str| -> Result<()> {
            if !manifest.permissions.iter().any(|p| p == permission) {
                return Err(anyhow!("Не объявлено разрешение: {permission}"));
            }
            Ok(())
        };
        match method.as_str() {
            "state.dirty" => {
                self.module_dirty.store(
                    args.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false),
                    Ordering::SeqCst,
                );
                Ok(Value::Bool(true))
            }
            "settings.get" => {
                require("settings")?;
                Ok(self
                    .store
                    .load_module_settings(&id)?
                    .unwrap_or(Value::Null))
            }
            "settings.set" => {
                require("settings")?;
                self.store.save_module_settings(&id, &args)?;
                Ok(Value::Bool(true))
            }
            "settings.path" => {
                require("settings")?;
                Ok(Value::String(
                    self.store.data_directory(&id)?.to_string_lossy().to_string(),
                ))
            }
            "clipboard.write" => {
                require("clipboard.write")?;
                util::copy_text(args.get("text").and_then(|v| v.as_str()).unwrap_or(""))?;
                Ok(Value::Bool(true))
            }
            "dialog.folder" => {
                require("dialog.folder")?;
                let folder = rfd::FileDialog::new()
                    .set_title("Выбрать папку")
                    .pick_folder();
                Ok(folder
                    .map(|p| Value::String(p.to_string_lossy().to_string()))
                    .unwrap_or(Value::Null))
            }
            "files.export" => {
                require("files.export")?;
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Path::new(v).file_name().and_then(|n| n.to_str()))
                    .unwrap_or("export.txt")
                    .to_string();
                let Some(target) = rfd::FileDialog::new()
                    .set_title("Сохранить файл")
                    .set_file_name(&name)
                    .save_file()
                else {
                    return Ok(json!({ "canceled": true }));
                };
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(args.get("base64").and_then(|v| v.as_str()).unwrap_or(""))
                    .map_err(|_| anyhow!("Некорректные данные файла."))?;
                if bytes.len() > 8 * 1024 * 1024 {
                    return Err(anyhow!("Файл превышает 8 МБ."));
                }
                std::fs::write(&target, bytes)?;
                Ok(json!({ "canceled": false, "filePath": target.to_string_lossy() }))
            }
            "document.pdf" | "document.print" => {
                require(&method)?;
                Err(anyhow!("Документ обрабатывается оболочкой хаба."))
            }
            _ => {
                if !method.starts_with("worker.") {
                    return Err(anyhow!("Неизвестная команда модуля."));
                }
                require("worker")?;
                let worker_definition = manifest
                    .worker
                    .as_ref()
                    .ok_or_else(|| anyhow!("У программы нет исполняемой части."))?
                    .clone();
                let gate = {
                    let mut gates = self.module_gates.lock().await;
                    gates
                        .entry(id.clone())
                        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                        .clone()
                };
                let _gate = gate.lock().await;
                if self.active_module().as_deref() != Some(id.as_str()) || self.is_busy(&id) {
                    return Err(anyhow!("Программа закрыта."));
                }
                let existing = self.workers.lock().await.get(&id).cloned();
                let process = match existing {
                    Some(process) if process.is_running() => process,
                    other => {
                        if let Some(process) = other {
                            process.dispose().await;
                        }
                        let root = self.store.app_version(&id, &manifest.version)?;
                        let executable = contract::inside(&root, &worker_definition.executable)?;
                        if !executable.is_file() {
                            return Err(anyhow!(
                                "Файл программы не найден. Переустановите программу."
                            ));
                        }
                        let process = ModuleProcess::start(
                            &executable,
                            &worker_definition.arguments,
                            &root,
                            &self.store.data_directory(&id)?,
                        )?;
                        self.workers
                            .lock()
                            .await
                            .insert(id.clone(), process.clone());
                        process
                    }
                };
                let result = process
                    .call(&method[7..], Some(args))
                    .await?;
                Ok(result)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Hub self-update
    // -----------------------------------------------------------------------

    async fn install_hub_update(&self) -> Result<Value> {
        let update = self
            .hub_update
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow!("Сведения об обновлении хаба ещё не получены."))?;
        if self.module_dirty.load(Ordering::SeqCst) {
            return Err(anyhow!("Сохраните проект перед обновлением хаба."));
        }
        if self.has_running_operations() {
            return Err(anyhow!("Дождитесь завершения установки программ."));
        }
        let cts = CancellationToken::new();
        let archive = self
            .github
            .download_hub_update(
                &self.store,
                &update,
                |progress| {
                    self.emit(&json!({
                        "event": "hub.progress",
                        "progress": progress,
                        "stage": "download",
                    }));
                },
                &cts,
            )
            .await?;
        let stage_root = std::env::temp_dir().join(format!(
            "thodhub-update-{}",
            util::random_token()
        ));
        let extract_archive = archive.clone();
        let extract_stage = stage_root.clone();
        let stage = tokio::task::spawn_blocking(move || {
            HubUpdater::extract(&extract_archive, &extract_stage)
        })
        .await
        .map_err(|e| anyhow!("Не удалось распаковать обновление. {e}"))?;
        let result = (|| -> Result<Value> {
            self.emit(&json!({
                "event": "hub.progress",
                "progress": 1.0,
                "stage": "extract",
            }));
            let stage = stage?;
            self.emit(&json!({
                "event": "hub.progress",
                "progress": 1.0,
                "stage": "ready",
            }));
            let target = hub_directory();
            let backup = PathBuf::from(format!(
                "{}.old-{}",
                target.to_string_lossy().trim_end_matches(['\\', '/']),
                Local::now().format("%Y%m%d-%H%M%S")
            ));
            let log = std::env::temp_dir().join("thodhub-update.log");
            let script = HubUpdater::write_script(&std::env::temp_dir())?;
            HubUpdater::launch(
                &script,
                std::process::id(),
                &target,
                &stage,
                &backup,
                &log,
            )?;
            Ok(json!({
                "restarting": true,
                "backup": backup.to_string_lossy(),
                "log": log.to_string_lossy(),
            }))
        })();
        if result.is_err() && stage_root.exists() {
            let _ = std::fs::remove_dir_all(&stage_root);
        }
        let _ = std::fs::remove_file(archive);
        result
    }

    // -----------------------------------------------------------------------
    // Backups
    // -----------------------------------------------------------------------

    async fn handle_backup(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let _gate = self.backup_gate.lock().await;
        let p = params.unwrap_or(Value::Null);
        match method {
            "backups.list" => self.backup_list().await,
            "backups.folder" => {
                let root = BackupStore::new(&self.store).root();
                std::fs::create_dir_all(&root)?;
                util::open_path(&root)?;
                Ok(Value::Bool(true))
            }
            "backups.tokenPage" => {
                util::open_url("https://github.com/settings/personal-access-tokens/new")?;
                Ok(Value::Bool(true))
            }
            "backups.disconnect" => {
                GitHubCredentials::new(self.store.root()).forget()?;
                self.backup_list().await
            }
            "backups.connect" | "backups.connectCli" => {
                let repository = get_str(&p, "repository")?.trim().to_string();
                contract::repository(&repository)?;
                let token = if method == "backups.connectCli" {
                    GitHubCredentials::read_cli().await?
                } else {
                    get_str(&p, "token")?
                };
                let cloud = GitHubBackups::new(&repository, &token)?;
                cloud.validate_private().await?;
                GitHubCredentials::new(self.store.root()).save(&token)?;
                self.store
                    .change(|settings| settings.backup_repository = repository.clone())?;
                self.backup_list().await
            }
            "backups.create" | "backups.exportFile" | "backups.importFile" | "backups.restore" => {
                let id = get_str(&p, "id")?;
                contract::id(&id)?;
                if self
                    .downloads
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&id)
                    || self.active_module().as_deref() == Some(id.as_str())
                    || !self
                        .removing
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(id.clone())
                {
                    return Err(anyhow!(
                        "Сначала закройте программу и дождитесь окончания её установки."
                    ));
                }
                let result = async {
                    self.dispose_worker(&id).await;
                    if self.store.installed(&id).is_some() {
                        StandaloneLauncher::stop_for_maintenance(&self.store, &id).await?;
                    }
                    if method == "backups.exportFile" {
                        let defaults = format!(
                            "{id}-{}.zip",
                            Local::now().format("%Y-%m-%d-%H%M")
                        );
                        let Some(target) = rfd::FileDialog::new()
                            .set_title("Экспорт базы и настроек")
                            .add_filter("Копия ThodHUB", &["zip"])
                            .set_file_name(&defaults)
                            .save_file()
                        else {
                            return Ok(json!({ "canceled": true }));
                        };
                        let entry = self.create_backup(&id, false).await?;
                        std::fs::copy(
                            BackupStore::new(&self.store).file_path(&entry.name)?,
                            &target,
                        )?;
                        return Ok(json!({
                            "canceled": false,
                            "path": target.to_string_lossy(),
                        }));
                    }
                    if method == "backups.importFile" {
                        let Some(source) = rfd::FileDialog::new()
                            .set_title("Импорт базы и настроек")
                            .add_filter("Копия ThodHUB", &["zip"])
                            .pick_file()
                        else {
                            return Ok(Value::Null);
                        };
                        let store = BackupStore::new(&self.store);
                        let entry = store
                            .import(&source, &id, &CancellationToken::new())
                            .await?;
                        return Ok(serde_json::to_value(entry)?);
                    }
                    if method == "backups.create" {
                        let entry = self.create_backup(&id, true).await?;
                        return Ok(serde_json::to_value(entry)?);
                    }
                    let name = get_str(&p, "name")?;
                    let store = BackupStore::new(&self.store);
                    let path = store.file_path(&name)?;
                    if !path.exists() {
                        let token = GitHubCredentials::new(self.store.root()).read()?;
                        let repository = self.store.settings().backup_repository;
                        let cloud = GitHubBackups::new(&repository, &token)?;
                        let entry = cloud
                            .list()
                            .await?
                            .into_iter()
                            .find(|e| e.name == name && e.app_id == id)
                            .ok_or_else(|| anyhow!("Копия не найдена в GitHub."))?;
                        cloud.download(&entry, &path).await?;
                    }
                    store
                        .restore(&name, &id, &CancellationToken::new())
                        .await?;
                    Ok(self.snapshot())
                }
                .await;
                self.removing
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                result
            }
            _ => Err(anyhow!("Неизвестная команда резервных копий.")),
        }
    }

    async fn backup_list(&self) -> Result<Value> {
        let store = BackupStore::new(&self.store);
        let mut local = store.list()?;
        let credentials = GitHubCredentials::new(self.store.root());
        let mut warning: Option<String> = None;
        if credentials.exists() {
            let repository = self.store.settings().backup_repository;
            let token = credentials.read()?;
            match GitHubBackups::new(&repository, &token) {
                Ok(cloud) => match cloud.list().await {
                    Ok(remote) => {
                        for entry in remote {
                            match local.iter().position(|e| e.name == entry.name) {
                                Some(index) => {
                                    let mut merged = entry;
                                    merged.local = true;
                                    local[index] = merged;
                                }
                                None => local.push(entry),
                            }
                        }
                    }
                    Err(error) => warning = Some(format!("{error}")),
                },
                Err(error) => warning = Some(format!("{error}")),
            }
        }
        local.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let mut ids: Vec<String> = self
            .store
            .settings()
            .installed
            .iter()
            .map(|a| a.id.clone())
            .chain(local.iter().map(|e| e.app_id.clone()))
            .collect();
        if let Ok(entries) = std::fs::read_dir(self.store.data_root()) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        if !name.starts_with('.') {
                            ids.push(name.to_string());
                        }
                    }
                }
            }
        }
        ids.push("thodhub-settings".into());
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        let apps: Vec<Value> = ids
            .iter()
            .map(|id| {
                json!({
                    "id": id,
                    "name": if id == "thodhub-settings" {
                        "ThodHUB · оформление и порядок".to_string()
                    } else {
                        self.catalog_name(id)
                    },
                })
            })
            .collect();
        Ok(json!({
            "entries": local,
            "apps": apps,
            "connected": credentials.exists(),
            "repository": self.store.settings().backup_repository,
            "localPath": store.root().to_string_lossy(),
            "warning": warning,
        }))
    }

    async fn create_backup(&self, id: &str, upload: bool) -> Result<BackupEntry> {
        let version = self.store.installed(id).map(|a| a.version);
        let store = BackupStore::new(&self.store);
        let mut entry = store
            .create(id, version.as_deref(), &CancellationToken::new())
            .await?;
        let credentials = GitHubCredentials::new(self.store.root());
        if upload && credentials.exists() {
            self.emit(&json!({
                "event": "notice",
                "text": "Локальная копия сохранена. Загружаем её в приватный GitHub…",
            }));
            let repository = self.store.settings().backup_repository;
            let token = credentials.read()?;
            let cloud = GitHubBackups::new(&repository, &token)?;
            cloud
                .upload(&store.file_path(&entry.name)?)
                .await
                .map_err(|e| {
                    anyhow!("Локальная копия сохранена, но отправить в GitHub не удалось. {e}")
                })?;
            entry.cloud = true;
        }
        self.emit(&json!({ "event": "notice", "text": "" }));
        Ok(entry)
    }

    // -----------------------------------------------------------------------
    // Shutdown
    // -----------------------------------------------------------------------

    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let tokens: Vec<CancellationToken> = self
            .downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for token in tokens {
            token.cancel();
        }
        let workers: Vec<Arc<ModuleProcess>> = {
            let mut workers = self.workers.lock().await;
            workers.drain().map(|(_, worker)| worker).collect()
        };
        for worker in workers {
            worker.dispose().await;
        }
    }

    /// The module guard is also useful to the UI thread for maintenance.
    pub fn assert_module_not_running(&self, id: &str) -> Result<()> {
        if self.store.is_installed_present(id) {
            ModuleProcessGuard::assert_not_running(&self.store, id)?;
        }
        Ok(())
    }

    pub fn module_dirty(&self) -> bool {
        self.module_dirty.load(Ordering::SeqCst)
    }

    pub fn clear_dirty(&self) {
        self.module_dirty.store(false, Ordering::SeqCst);
    }

    pub fn set_dirty(&self, value: bool) {
        self.module_dirty.store(value, Ordering::SeqCst);
    }

    pub fn installed_version(&self, id: &str) -> Option<String> {
        self.store.installed(id).map(|a| a.version)
    }

    pub fn is_missing(&self, id: &str) -> bool {
        self.store
            .installed(id)
            .map(|a| !self.store.app_present(&a))
            .unwrap_or(false)
    }

    pub fn mark_installed(&self, id: &str) -> Option<InstalledApp> {
        self.store.installed(id)
    }

    pub fn settings_snapshot(&self) -> HubSettings {
        self.store.settings_view()
    }

    pub fn utc_now() -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

fn hub_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn get_str(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| anyhow!("Некорректный запрос."))
}

fn get_bool(value: &Value, key: &str) -> Result<bool> {
    value
        .get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow!("Некорректный запрос."))
}

pub fn response_ok(id: &str, result: Value) -> String {
    serde_json::to_string(&json!({ "id": id, "result": result })).unwrap_or_default()
}

pub fn response_error(id: &str, error: String) -> String {
    serde_json::to_string(&json!({ "id": id, "error": error })).unwrap_or_default()
}
