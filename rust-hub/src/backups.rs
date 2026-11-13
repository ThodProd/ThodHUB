use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::contract;
use crate::models::HubSettings;
use crate::store::HubStore;
use crate::util;

pub const MAX_BACKUP_BYTES: u64 = 256 * 1024 * 1024;
const EXCLUDED: [&str; 5] = ["cache", "logs", "temp", "tmp", "runtime"];

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupEntry {
    pub name: String,
    pub app_id: String,
    pub created_at: DateTime<Utc>,
    pub size: i64,
    pub local: bool,
    pub cloud: bool,
    pub asset_id: Option<i64>,
    pub sha256: Option<String>,
}
impl Default for BackupEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            app_id: String::new(),
            created_at: DateTime::<Utc>::MIN_UTC,
            size: 0,
            local: false,
            cloud: false,
            asset_id: None,
            sha256: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupMetadata {
    pub schema_version: i32,
    pub app_id: String,
    pub app_version: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}
impl Default for BackupMetadata {
    fn default() -> Self {
        Self {
            schema_version: 1,
            app_id: String::new(),
            app_version: None,
            created_at: None,
        }
    }
}

pub struct BackupStore<'a> {
    store: &'a HubStore,
}

impl<'a> BackupStore<'a> {
    pub fn new(store: &'a HubStore) -> Self {
        Self { store }
    }
    pub fn root(&self) -> PathBuf {
        self.store.backups_root()
    }

    pub fn describe(
        name: &str,
        size: i64,
        local: bool,
        cloud: bool,
        asset_id: Option<i64>,
        sha256: Option<String>,
    ) -> Option<BackupEntry> {
        let parts: Vec<&str> = name.split("--").collect();
        if parts.len() != 3 || !name.ends_with(".zip") {
            return None;
        }
        contract::id(parts[0]).ok()?;
        let created_at = util::parse_backup_timestamp(parts[1])?;
        if parts[2].len() != 36
            || !parts[2].is_ascii()
            || !parts[2][..32].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        Some(BackupEntry {
            name: name.to_string(),
            app_id: parts[0].to_string(),
            created_at,
            size,
            local,
            cloud,
            asset_id,
            sha256,
        })
    }

    pub fn list(&self) -> Result<Vec<BackupEntry>> {
        let root = self.root();
        std::fs::create_dir_all(&root)?;
        let mut entries = Vec::new();
        for item in std::fs::read_dir(&root)? {
            let path = item?.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let size = std::fs::metadata(&path)?.len() as i64;
            if let Some(entry) = Self::describe(name, size, true, false, None, None) {
                entries.push(entry);
            }
        }
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries)
    }

    pub fn file_path(&self, name: &str) -> Result<PathBuf> {
        if Self::describe(name, 0, true, false, None, None).is_none()
            || Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name)
        {
            return Err(anyhow!("Некорректное имя резервной копии."));
        }
        contract::inside(&self.root(), name)
    }

    pub async fn create(
        &self,
        id: &str,
        version: Option<&str>,
        ct: &CancellationToken,
    ) -> Result<BackupEntry> {
        contract::id(id)?;
        let root = self.root();
        std::fs::create_dir_all(&root)?;
        let now = Utc::now();
        let name = format!(
            "{id}--{}--{}.zip",
            util::backup_timestamp(now),
            util::random_token()
        );
        let path = self.file_path(&name)?;
        let tmp = path.with_extension("zip.partial");
        let result = (|| -> Result<()> {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("backup.json", options)?;
            zip.write_all(util::json_write(&BackupMetadata {
                schema_version: 1,
                app_id: id.to_string(),
                app_version: version.map(|v| v.to_string()),
                created_at: Some(now),
            })?.as_bytes())?;
            if id == "thodhub-settings" {
                let settings = self.store.settings();
                zip.start_file("data/preferences.json", options)?;
                zip.write_all(util::json_write(&serde_json::json!({
                    "theme": settings.theme,
                    "checkOnStartup": settings.check_on_startup,
                    "autoInstallUpdates": settings.auto_install_updates,
                    "sidebarOrder": settings.sidebar_order,
                }))?.as_bytes())?;
            } else {
                let source = self.store.data_directory(id)?;
                let data_root = self.store.data_root();
                let mut total: u64 = 0;
                let mut count: u64 = 0;
                let mut stack = vec![source.clone()];
                while let Some(dir) = stack.pop() {
                    for item in std::fs::read_dir(&dir)? {
                        let item = item?.path();
                        if ct.is_cancelled() {
                            return Err(anyhow!("Действие отменено."));
                        }
                        util::managed_check(&data_root, &item)?;
                        let relative = item
                            .strip_prefix(&source)
                            .unwrap_or(&item)
                            .to_string_lossy()
                            .replace('\\', "/");
                        let first = relative.split('/').next().unwrap_or("");
                        if EXCLUDED.iter().any(|x| x.eq_ignore_ascii_case(first)) {
                            continue;
                        }
                        let attributes = std::fs::symlink_metadata(&item)?.file_attributes();
                        if attributes & 0x400 != 0 {
                            return Err(anyhow!(
                                "Резервная копия не обходит ссылки: {relative}"
                            ));
                        }
                        if item.is_dir() {
                            stack.push(item);
                            continue;
                        }
                        count += 1;
                        total += std::fs::metadata(&item)?.len();
                        if count > 20_000 || total > 4 * MAX_BACKUP_BYTES {
                            return Err(anyhow!(
                                "Конфиги превышают предел 1 ГБ / 20 000 файлов."
                            ));
                        }
                        zip.start_file(format!("data/{relative}"), options)?;
                        let mut input = std::fs::File::open(&item)?;
                        std::io::copy(&mut input, &mut zip)?;
                    }
                }
            }
            zip.finish()?;
            if std::fs::metadata(&tmp)?.len() > MAX_BACKUP_BYTES {
                return Err(anyhow!("Сжатая копия превышает 256 МБ."));
            }
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })();
        if result.is_err() && tmp.exists() {
            let _ = std::fs::remove_file(&tmp);
        }
        result?;
        let size = std::fs::metadata(&path)?.len() as i64;
        Self::describe(&name, size, true, false, None, None)
            .ok_or_else(|| anyhow!("Не удалось описать резервную копию."))
    }

    fn extract(
        source: &Path,
        staging: &Path,
        id: &str,
        ct: &CancellationToken,
    ) -> Result<BackupMetadata> {
        let file = std::fs::File::open(source)?;
        let mut zip = zip::ZipArchive::new(file)?;
        if zip.len() > 20_001 {
            return Err(anyhow!("Некорректный архив."));
        }
        let metadata_count = (0..zip.len())
            .filter(|i| {
                zip.by_index(*i)
                    .ok()
                    .and_then(|e| e.name().ok().map(|n| n.to_string()))
                    .as_deref()
                    == Some("backup.json")
            })
            .count();
        if metadata_count != 1 {
            if metadata_count == 0 {
                return Err(anyhow!("В архиве нет описания резервной копии."));
            }
            return Err(anyhow!("Повторяющееся описание копии."));
        }
        let metadata = {
            let mut entry = zip
                .by_name("backup.json")
                .map_err(|_| anyhow!("В архиве нет описания резервной копии."))?;
            if entry.size() > 65536 {
                return Err(anyhow!("Некорректный архив."));
            }
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            util::json_read_text::<BackupMetadata>(&text)?
        };
        if metadata.schema_version != 1
            || metadata.app_id != id
            || metadata.created_at.is_none()
        {
            return Err(anyhow!("Копия относится к другой программе."));
        }
        let mut seen = HashSet::new();
        let mut total: u64 = 0;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            let name = entry.name().unwrap_or_default().to_string();
            if name == "backup.json" {
                continue;
            }
            if ct.is_cancelled() {
                return Err(anyhow!("Действие отменено."));
            }
            let normalized = name.replace('\\', "/");
            if !normalized.starts_with("data/") || normalized.ends_with('/') {
                return Err(anyhow!("Некорректный путь в копии."));
            }
            let target = contract::inside(staging, &normalized[5..])?;
            let unix_kind = (entry.external_attributes() >> 16) & 0xF000;
            total += entry.size();
            if !seen.insert(target.to_string_lossy().to_lowercase())
                || unix_kind == 0xA000
                || entry.external_attributes() & 0x400 != 0
                || total > 4 * MAX_BACKUP_BYTES
            {
                return Err(anyhow!("Некорректный архив резервной копии."));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            std::io::copy(&mut entry, &mut output)?;
        }
        Ok(metadata)
    }

    pub async fn import(
        &self,
        source: &Path,
        id: &str,
        ct: &CancellationToken,
    ) -> Result<BackupEntry> {
        contract::id(id)?;
        let root = self.root();
        std::fs::create_dir_all(&root)?;
        if std::fs::metadata(source)?.len() > MAX_BACKUP_BYTES {
            return Err(anyhow!("Копия слишком большая."));
        }
        let staging = self
            .store
            .cache_root()
            .join(format!("import-{}", util::random_token()));
        std::fs::create_dir_all(&staging)?;
        let result = (|| -> Result<BackupEntry> {
            let incoming = staging.join("incoming.zip");
            let mut input = std::fs::File::open(source)?;
            if std::fs::metadata(source)?.len() > MAX_BACKUP_BYTES {
                return Err(anyhow!("Копия слишком большая."));
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&incoming)?;
            std::io::copy(&mut input, &mut output)?;
            drop(output);
            let validated = staging.join("validated");
            std::fs::create_dir_all(&validated)?;
            let metadata = Self::extract(&incoming, &validated, id, ct)?;
            let name = format!(
                "{id}--{}--{}.zip",
                util::backup_timestamp(metadata.created_at.unwrap()),
                util::random_token()
            );
            let dest = self.file_path(&name)?;
            std::fs::rename(&incoming, &dest)?;
            let size = std::fs::metadata(&dest)?.len() as i64;
            Self::describe(&name, size, true, false, None, None)
                .ok_or_else(|| anyhow!("Не удалось описать резервную копию."))
        })();
        let _ = util::delete_tree(&self.store.cache_root(), &staging);
        result
    }

    pub async fn restore(
        &self,
        name: &str,
        id: &str,
        ct: &CancellationToken,
    ) -> Result<()> {
        contract::id(id)?;
        let source = self.file_path(name)?;
        if std::fs::metadata(&source)?.len() > MAX_BACKUP_BYTES {
            return Err(anyhow!("Копия слишком большая."));
        }
        let staging = self
            .store
            .cache_root()
            .join(format!("restore-{}", util::random_token()));
        std::fs::create_dir_all(&staging)?;
        let result: Result<()> = async {
            Self::extract(&source, &staging, id, ct)?;
            if id == "thodhub-settings" {
                let prefs_path = staging.join("preferences.json");
                let prefs = util::json_read::<HubSettings>(&prefs_path)?;
                if !matches!(prefs.theme.as_str(), "light" | "dark" | "system") {
                    return Err(anyhow!("Некорректная тема в копии."));
                }
                self.create(id, None, ct).await?;
                self.store.change(|settings| {
                    settings.theme = prefs.theme.clone();
                    settings.check_on_startup = prefs.check_on_startup;
                    settings.auto_install_updates = prefs.auto_install_updates;
                    let mut order: Vec<String> = prefs
                        .sidebar_order
                        .iter()
                        .filter(|id| settings.installed.iter().any(|a| &a.id == *id))
                        .cloned()
                        .collect();
                    for app in &settings.installed {
                        if !order.contains(&app.id) {
                            order.push(app.id.clone());
                        }
                    }
                    settings.sidebar_order = order;
                })?;
                return Ok(());
            }
            let installed_version = self.store.installed(id).map(|a| a.version);
            self.create(id, installed_version.as_deref(), ct).await?;
            let data_root = self.store.data_root();
            let destination = util::managed_check(&data_root, &self.store.data_directory(id)?)?;
            util::assert_unlocked(&data_root, &destination)?;
            let old = data_root.join(format!(".restore-old-{}", util::random_token()));
            if destination.exists() {
                std::fs::rename(&destination, &old)?;
            }
            if let Err(error) = std::fs::rename(&staging, &destination) {
                if old.exists() {
                    let _ = std::fs::rename(&old, &destination);
                }
                return Err(error.into());
            }
            let _ = util::delete_tree(&data_root, &old);
            Ok(())
        }
        .await;
        if staging.exists() {
            let _ = util::delete_tree(&self.store.cache_root(), &staging);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// GitHub history of backups
// ---------------------------------------------------------------------------

pub struct GitHubBackups {
    repository: String,
    token: String,
    client: reqwest::Client,
}

impl GitHubBackups {
    pub fn new(repository: &str, token: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15 * 60))
            .build()?;
        Ok(Self {
            repository: repository.to_string(),
            token: token.to_string(),
            client,
        })
    }

    const TAG: &'static str = "thodhub-backups-v1";

    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<serde_json::Value>,
        binary: bool,
        bytes: Option<Vec<u8>>,
    ) -> Result<reqwest::Response> {
        let parsed = url::Url::parse(url)?;
        if parsed.scheme() != "https"
            || !matches!(
                parsed.host_str(),
                Some("api.github.com") | Some("uploads.github.com")
            )
        {
            return Err(anyhow!("Некорректный адрес GitHub API."));
        }
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, "ThodHUB/0.1.0")
            .header(
                reqwest::header::ACCEPT,
                if binary {
                    "application/octet-stream"
                } else {
                    "application/vnd.github+json"
                },
            );
        if let Some(value) = body {
            request = request.json(&value);
        }
        if let Some(value) = bytes {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/zip")
                .body(value);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success()
            && status != reqwest::StatusCode::FOUND
            && status != reqwest::StatusCode::NOT_FOUND
        {
            return Err(anyhow!(
                "GitHub: HTTP {}. Проверьте доступ к приватному репозиторию и сеть.",
                status.as_u16()
            ));
        }
        Ok(response)
    }

    async fn json_body(response: reqwest::Response) -> Result<serde_json::Value> {
        Ok(response.json::<serde_json::Value>().await?)
    }

    pub async fn validate_private(&self) -> Result<()> {
        contract::repository(&self.repository)?;
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("https://api.github.com/repos/{}", self.repository),
                None,
                false,
                None,
            )
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("Приватный репозиторий не найден или нет доступа."));
        }
        let response = response.error_for_status()?;
        let data = Self::json_body(response).await?;
        if data.get("private").and_then(|v| v.as_bool()) != Some(true) {
            return Err(anyhow!(
                "Конфиги разрешено отправлять только в приватный репозиторий."
            ));
        }
        Ok(())
    }

    async fn release_id(&self, create: bool) -> Result<Option<i64>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!(
                    "https://api.github.com/repos/{}/releases/tags/{}",
                    self.repository,
                    Self::TAG
                ),
                None,
                false,
                None,
            )
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            if !create {
                return Ok(None);
            }
            let posted = self
                .request(
                    reqwest::Method::POST,
                    &format!("https://api.github.com/repos/{}/releases", self.repository),
                    Some(serde_json::json!({
                        "tag_name": Self::TAG,
                        "name": "ThodHUB · история конфигураций",
                        "body": "Private immutable dated configuration snapshots.",
                        "draft": false,
                        "prerelease": false
                    })),
                    false,
                    None,
                )
                .await?
                .error_for_status()?;
            let data = Self::json_body(posted).await?;
            return Ok(data.get("id").and_then(|v| v.as_i64()));
        }
        let response = response.error_for_status()?;
        let data = Self::json_body(response).await?;
        Ok(data.get("id").and_then(|v| v.as_i64()))
    }

    pub async fn list(&self) -> Result<Vec<BackupEntry>> {
        self.validate_private().await?;
        let Some(id) = self.release_id(false).await? else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for page in 1..=1000 {
            let response = self
                .request(
                    reqwest::Method::GET,
                    &format!(
                        "https://api.github.com/repos/{}/releases/{id}/assets?per_page=100&page={page}",
                        self.repository
                    ),
                    None,
                    false,
                    None,
                )
                .await?
                .error_for_status()?;
            let assets = Self::json_body(response).await?;
            let Some(array) = assets.as_array() else {
                break;
            };
            for asset in array {
                let Some(name) = asset.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let size = asset.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
                let asset_id = asset.get("id").and_then(|v| v.as_i64());
                let digest = asset
                    .get("digest")
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.strip_prefix("sha256:"))
                    .map(|v| v.to_string());
                if let Some(entry) =
                    BackupStore::describe(name, size, false, true, asset_id, digest)
                {
                    result.push(entry);
                }
            }
            if array.len() < 100 {
                return Ok(result);
            }
        }
        Err(anyhow!("Слишком большая история резервных копий."))
    }

    pub async fn upload(&self, path: &Path) -> Result<()> {
        self.validate_private().await?;
        let Some(id) = self.release_id(true).await? else {
            return Err(anyhow!("Не удалось создать выпуск для резервных копий."));
        };
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > MAX_BACKUP_BYTES {
            return Err(anyhow!("Резервная копия превышает 256 МБ."));
        }
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("Некорректное имя резервной копии."))?;
        let response = self
            .request(
                reqwest::Method::POST,
                &format!(
                    "https://uploads.github.com/repos/{}/releases/{id}/assets?name={}",
                    self.repository,
                    util::escape_url_component(name)
                ),
                None,
                false,
                Some(bytes),
            )
            .await?
            .error_for_status()?;
        let _ = response;
        Ok(())
    }

    pub async fn download(&self, entry: &BackupEntry, path: &Path) -> Result<()> {
        self.validate_private().await?;
        let Some(asset_id) = entry.asset_id else {
            return Err(anyhow!("Некорректная копия."));
        };
        if entry.size as u64 > MAX_BACKUP_BYTES {
            return Err(anyhow!("Некорректная копия."));
        }
        let mut response = self
            .request(
                reqwest::Method::GET,
                &format!(
                    "https://api.github.com/repos/{}/releases/assets/{asset_id}",
                    self.repository
                ),
                None,
                true,
                None,
            )
            .await?;
        let _download_client: reqwest::Client;
        if response.status() == reqwest::StatusCode::FOUND {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("Некорректное перенаправление GitHub."))?;
            let url = url::Url::parse(location)?;
            let host = url.host_str().unwrap_or("");
            if url.scheme() != "https"
                || !(host.ends_with(".githubusercontent.com")
                    || host == "release-assets.githubusercontent.com")
            {
                return Err(anyhow!("Некорректное перенаправление GitHub."));
            }
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15 * 60))
                .build()?;
            response = client.get(url).send().await?.error_for_status()?;
            _download_client = client;
        } else {
            response = response.error_for_status()?;
            _download_client = reqwest::Client::new();
        }
        let temp = path.with_extension("zip.partial");
        let result: Result<()> = async {
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            let mut received: i64 = 0;
            let mut response = response;
            while let Some(chunk) = response.chunk().await? {
                received += chunk.len() as i64;
                if received > entry.size || received as u64 > MAX_BACKUP_BYTES {
                    return Err(anyhow!("Размер копии изменился."));
                }
                output.write_all(&chunk)?;
            }
            if received != entry.size {
                return Err(anyhow!("Копия скачана не полностью."));
            }
            drop(output);
            if let Some(expected) = &entry.sha256 {
                let mut file = std::fs::File::open(&temp)?;
                let mut hasher = Sha256::new();
                std::io::copy(&mut file, &mut hasher)?;
                if !hex::encode_upper(hasher.finalize()).eq_ignore_ascii_case(expected) {
                    return Err(anyhow!("SHA-256 резервной копии не совпадает."));
                }
            }
            std::fs::rename(&temp, path)?;
            Ok(())
        }
        .await;
        if result.is_err() && temp.exists() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}
