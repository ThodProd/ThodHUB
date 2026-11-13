use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Result};

use crate::contract;
use crate::models::{CatalogApp, InstalledApp, ModuleManifest};
use crate::store::HubStore;
use crate::util;

pub struct PackageInstaller {
    gate: tokio::sync::Mutex<()>,
}

impl PackageInstaller {
    pub fn new() -> Self {
        Self {
            gate: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn install(
        &self,
        store: &HubStore,
        archive_path: &Path,
        app: &CatalogApp,
        ct: &tokio_util::sync::CancellationToken,
    ) -> Result<ModuleManifest> {
        contract::id(&app.id)?;
        let release = app
            .release
            .as_ref()
            .ok_or_else(|| anyhow!("Совместимый выпуск пока не опубликован."))?;
        contract::release(release, &app.repository)?;
        let _guard = self.gate.lock().await;
        let apps_root = store.apps_root();
        let staging = apps_root.join(format!(".staging-{}", util::random_token()));
        let result = self
            .install_inner(store, archive_path, app, release, &staging, ct)
            .await;
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }

    async fn install_inner(
        &self,
        store: &HubStore,
        archive_path: &Path,
        app: &CatalogApp,
        release: &crate::models::PackageRelease,
        staging: &Path,
        ct: &tokio_util::sync::CancellationToken,
    ) -> Result<ModuleManifest> {
        if ct.is_cancelled() {
            return Err(anyhow!("Действие отменено."));
        }
        let metadata = std::fs::metadata(archive_path)?;
        if metadata.len() as i64 != release.size {
            return Err(anyhow!("Размер скачанного файла не совпадает с выпуском."));
        }
        let hash = util::sha256_file_async(archive_path).await?;
        if !hash.eq_ignore_ascii_case(&release.sha256) {
            return Err(anyhow!("Пакет повреждён: SHA-256 не совпадает."));
        }
        let file = std::fs::File::open(archive_path)?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| anyhow!("Повреждённый ZIP-пакет: {e}"))?;
        if zip.len() > 20_000 {
            return Err(anyhow!("Слишком много файлов в пакете."));
        }
        std::fs::create_dir_all(staging)?;
        let mut expanded: i64 = 0;
        let mut seen: HashSet<String> = HashSet::new();
        for index in 0..zip.len() {
            if ct.is_cancelled() {
                return Err(anyhow!("Действие отменено."));
            }
            let mut entry = zip.by_index(index)?;
            let full_name = entry.name().unwrap_or_default().to_string();
            let normalized = full_name.replace('\\', "/");
            let name = normalized.trim_end_matches('/').to_string();
            let target = contract::inside(staging, &name)
                .map_err(|e| anyhow!("Недопустимый путь в ZIP: {}. {e}", serde_json::to_string(&full_name).unwrap_or_default()))?;
            if !seen.insert(target.to_string_lossy().to_lowercase()) {
                return Err(anyhow!("Повторяющийся путь в архиве."));
            }
            let unix_kind = (entry.external_attributes() >> 16) & 0xF000;
            if unix_kind == 0xA000 || entry.external_attributes() & 0x400 != 0 {
                return Err(anyhow!("Ссылки внутри пакета запрещены."));
            }
            expanded = expanded
                .checked_add(entry.size() as i64)
                .ok_or_else(|| anyhow!("Распакованный пакет слишком большой."))?;
            if expanded > 2 * contract::MAX_PACKAGE_BYTES {
                return Err(anyhow!("Распакованный пакет слишком большой."));
            }
            if normalized.ends_with('/') {
                std::fs::create_dir_all(&target)?;
                continue;
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
        let manifest_path = staging.join("manifest.json");
        if !manifest_path.is_file() {
            return Err(anyhow!("В корне пакета нужен manifest.json до 64 КБ."));
        }
        if std::fs::metadata(&manifest_path)?.len() > 65536 {
            return Err(anyhow!("В корне пакета нужен manifest.json до 64 КБ."));
        }
        let manifest = util::json_read::<ModuleManifest>(&manifest_path)?;
        contract::manifest(&manifest)?;
        if manifest.id != app.id || manifest.version != release.version || manifest.mode != app.mode
        {
            return Err(anyhow!("Пакет не соответствует карточке каталога."));
        }
        let mut entries_to_check: Vec<(&str, Option<String>)> = vec![
            ("ui", manifest.ui.clone()),
            (
                "launch",
                manifest.launch.as_ref().map(|w| w.executable.clone()),
            ),
            (
                "cleanup",
                manifest.cleanup.as_ref().map(|w| w.executable.clone()),
            ),
            ("stop", manifest.stop.as_ref().map(|w| w.executable.clone())),
            (
                "integration",
                manifest.integration.as_ref().map(|w| w.executable.clone()),
            ),
            (
                "worker",
                manifest.worker.as_ref().map(|w| w.executable.clone()),
            ),
        ];
        if manifest.mode == "embedded" {
            entries_to_check.retain(|(name, _)| *name == "ui" || *name == "cleanup" || *name == "stop" || *name == "integration" || *name == "worker");
        }
        for (kind, value) in entries_to_check {
            if let Some(relative) = value {
                if kind == "ui" && manifest.mode != "embedded" {
                    continue;
                }
                if !contract::inside(staging, &relative)?.is_file() {
                    return Err(anyhow!("В пакете отсутствует точка входа."));
                }
            }
        }
        let dest = store.app_version(&app.id, &release.version)?;
        let apps_root = store.apps_root();
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let replaced = apps_root.join(format!(".replace-{}", util::random_token()));
        let mut moved_old = false;
        if dest.exists() {
            util::assert_unlocked(&apps_root, &dest)?;
            std::fs::rename(&dest, &replaced)?;
            moved_old = true;
        }
        let activate = (|| -> Result<()> {
            std::fs::rename(staging, &dest)?;
            store.change(|settings| {
                let old = settings
                    .installed
                    .iter()
                    .find(|a| a.id == app.id)
                    .cloned();
                settings.installed.retain(|a| a.id != app.id);
                let previous_version = match old {
                    Some(old) if old.version == release.version => old.previous_version,
                    Some(old) => Some(old.version),
                    None => None,
                };
                settings.installed.push(InstalledApp {
                    id: app.id.clone(),
                    version: release.version.clone(),
                    previous_version,
                    ..Default::default()
                });
                if !settings.sidebar_order.iter().any(|x| x == &app.id) {
                    settings.sidebar_order.push(app.id.clone());
                }
            })
        })();
        if let Err(error) = activate {
            if dest.exists() {
                let _ = util::delete_tree(&apps_root, &dest);
            }
            if moved_old && replaced.exists() {
                let _ = std::fs::rename(&replaced, &dest);
            }
            return Err(error);
        }
        if replaced.exists() {
            let _ = util::delete_tree(&apps_root, &replaced);
        }
        Ok(manifest)
    }

    pub async fn rollback(&self, store: &HubStore, id: &str) -> Result<()> {
        let _guard = self.gate.lock().await;
        let old = store
            .installed(id)
            .ok_or_else(|| anyhow!("Программа не установлена."))?;
        let previous = old
            .previous_version
            .clone()
            .ok_or_else(|| anyhow!("Нет предыдущей версии."))?;
        let path = store
            .app_version(id, &previous)?
            .join("manifest.json");
        let manifest = util::json_read::<ModuleManifest>(&path)?;
        contract::manifest(&manifest)?;
        if manifest.id != id || manifest.version != previous {
            return Err(anyhow!("Повреждена предыдущая версия."));
        }
        store.change(|settings| {
            settings.installed.retain(|a| a.id != id);
            settings.installed.push(InstalledApp {
                id: id.to_string(),
                version: previous.clone(),
                previous_version: Some(old.version.clone()),
                installed_at: old.installed_at,
                integration_error: old.integration_error.clone(),
                missing: false,
            });
        })
    }

    pub async fn uninstall(
        &self,
        store: &HubStore,
        id: &str,
        delete_data: bool,
    ) -> Result<()> {
        contract::id(id)?;
        let _guard = self.gate.lock().await;
        if store.installed(id).is_none() {
            return Err(anyhow!("Программа уже удалена."));
        }
        let apps_root = store.apps_root();
        let data_root = store.data_root();
        let app_path = util::managed_check(&apps_root, &apps_root.join(id))?;
        let data_path = util::managed_check(&data_root, &data_root.join(id))?;
        util::assert_unlocked(&apps_root, &app_path).map_err(|e| {
            anyhow!("Закройте программу и её файлы перед удалением. {e}")
        })?;
        if delete_data {
            util::assert_unlocked(&data_root, &data_path).map_err(|e| {
                anyhow!("Закройте программу и её файлы перед удалением. {e}")
            })?;
        }
        let trash = util::managed_check(
            &apps_root,
            &apps_root.join(format!(".remove-{}", util::random_token())),
        )?;
        let data_trash = util::managed_check(
            &data_root,
            &data_root.join(format!(".remove-{}", util::random_token())),
        )?;
        let move_result = (|| -> Result<()> {
            if app_path.exists() {
                std::fs::rename(&app_path, &trash)?;
            }
            if delete_data && data_path.exists() {
                std::fs::rename(&data_path, &data_trash)?;
            }
            Ok(())
        })();
        if let Err(error) = move_result {
            if trash.exists() {
                let _ = std::fs::rename(&trash, &app_path);
            }
            if data_trash.exists() {
                let _ = std::fs::rename(&data_trash, &data_path);
            }
            return Err(error);
        }
        let change = store.change(|settings| {
            settings.installed.retain(|a| a.id != id);
            settings.sidebar_order.retain(|x| x != id);
        });
        if let Err(error) = change {
            if trash.exists() {
                let _ = std::fs::rename(&trash, &app_path);
            }
            if data_trash.exists() {
                let _ = std::fs::rename(&data_trash, &data_path);
            }
            return Err(error);
        }
        let _ = util::delete_tree(&apps_root, &trash);
        if delete_data {
            let _ = util::delete_tree(&data_root, &data_trash);
        }
        Ok(())
    }
}

// Keeps `Read` imported for std::io::copy's generic bound on older toolchains.
#[allow(dead_code)]
fn _assert_read<T: Read>() {}
