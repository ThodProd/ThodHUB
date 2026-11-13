use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::contract;
use crate::models::{HubSettings, InstalledApp, ModuleManifest};
use crate::util;

pub struct HubStore {
    root: PathBuf,
    settings: Mutex<HubSettings>,
}

impl HubStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = util::absolute(root)?;
        let store = Self {
            root,
            settings: Mutex::new(HubSettings::default()),
        };
        for path in [&store.root, &store.apps_root(), &store.data_root(), &store.cache_root()] {
            std::fs::create_dir_all(path)?;
        }
        let file = store.root.join("hub.json");
        let settings = if file.exists() {
            util::json_read::<HubSettings>(&file)?
        } else {
            HubSettings::default()
        };
        for app in &settings.installed {
            contract::id(&app.id)?;
            contract::version(&app.version)?;
            if let Some(previous) = &app.previous_version {
                contract::version(previous)?;
            }
        }
        *store.settings.lock().unwrap() = settings;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn apps_root(&self) -> PathBuf {
        self.root.join("Apps")
    }
    pub fn data_root(&self) -> PathBuf {
        self.root.join("Data")
    }
    pub fn cache_root(&self) -> PathBuf {
        self.root.join("Cache")
    }
    pub fn backups_root(&self) -> PathBuf {
        self.root.join("Backups")
    }
    pub fn secrets_file(&self) -> PathBuf {
        self.root.join("Secrets").join("github.bin")
    }

    fn lock_settings(&self) -> std::sync::MutexGuard<'_, HubSettings> {
        self.settings.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn settings(&self) -> HubSettings {
        self.lock_settings().clone()
    }

    pub fn installed(&self, id: &str) -> Option<InstalledApp> {
        self.lock_settings()
            .installed
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    pub fn app_version(&self, id: &str, version: &str) -> Result<PathBuf> {
        contract::id(id)?;
        contract::version(version)?;
        Ok(self.apps_root().join(id).join(version))
    }

    pub fn data_directory(&self, id: &str) -> Result<PathBuf> {
        contract::id(id)?;
        let path = self.data_root().join(id);
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    pub fn manifest(&self, id: &str) -> Result<ModuleManifest> {
        let app = self
            .installed(id)
            .ok_or_else(|| anyhow!("Программа не установлена."))?;
        let path = self
            .app_version(id, &app.version)?
            .join("manifest.json");
        let value = util::json_read::<ModuleManifest>(&path)
            .map_err(|e| anyhow!("Не удалось прочитать манифест установленной программы: {e}"))?;
        contract::manifest(&value)?;
        if value.id != id || value.version != app.version {
            return Err(anyhow!(
                "Манифест установленной программы не совпадает с учётом хаба."
            ));
        }
        Ok(value)
    }

    /// Reads the manifest only when the installed files are actually present.
    pub fn manifest_if_present(&self, id: &str) -> Option<ModuleManifest> {
        let app = self.installed(id)?;
        if !self.app_present(&app) {
            return None;
        }
        self.manifest(id).ok()
    }

    /// True when the recorded version folder and its entry point still exist.
    pub fn app_present(&self, app: &InstalledApp) -> bool {
        let Ok(directory) = self.app_version(&app.id, &app.version) else {
            return false;
        };
        if !directory.is_dir() {
            return false;
        }
        let Ok(manifest) = util::json_read::<ModuleManifest>(&directory.join("manifest.json")) else {
            return false;
        };
        if contract::manifest(&manifest).is_err()
            || manifest.id != app.id
            || manifest.version != app.version
        {
            return false;
        }
        let entry = if manifest.mode == "embedded" {
            manifest.ui.clone()
        } else {
            manifest.launch.as_ref().map(|l| l.executable.clone())
        };
        match entry {
            Some(relative) => contract::inside(&directory, &relative)
                .map(|p| p.is_file())
                .unwrap_or(false),
            None => false,
        }
    }

    pub fn is_installed_present(&self, id: &str) -> bool {
        self.installed(id)
            .map(|app| self.app_present(&app))
            .unwrap_or(false)
    }

    /// Returns a copy of settings where every installed entry carries the computed `missing` flag.
    pub fn settings_view(&self) -> HubSettings {
        let mut settings = self.settings();
        for app in &mut settings.installed {
            app.missing = !self.app_present(app);
        }
        settings
    }

    pub fn change<F>(&self, change: F) -> Result<()>
    where
        F: FnOnce(&mut HubSettings),
    {
        let mut guard = self.lock_settings();
        let mut next = guard.clone();
        change(&mut next);
        let text = util::json_write(&next)?;
        util::write_atomic(&self.root.join("hub.json"), &text)?;
        *guard = next;
        Ok(())
    }

    pub fn save_module_settings(&self, id: &str, settings: &Value) -> Result<()> {
        let text = serde_json::to_string(settings)?;
        if text.len() > 8 * 1024 * 1024 {
            return Err(anyhow!("Настройки превышают 8 МБ."));
        }
        util::write_atomic(&self.data_directory(id)?.join("settings.json"), &text)
    }

    pub fn load_module_settings(&self, id: &str) -> Result<Option<Value>> {
        let file = self.data_directory(id)?.join("settings.json");
        if !file.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(file)?;
        Ok(Some(serde_json::from_str(&text)?))
    }
}
