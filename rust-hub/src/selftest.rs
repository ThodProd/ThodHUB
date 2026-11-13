//! Development-only end-to-end check of the catalog download, install,
//! missing-app detection and uninstall paths.
//!
//! It is intentionally independent of the UI: the same `GitHubCatalog`,
//! `PackageInstaller` and `HubStore` types used by the hub are exercised
//! against an isolated home directory.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tokio_util::sync::CancellationToken;

use crate::github::GitHubCatalog;
use crate::installer::PackageInstaller;
use crate::store::HubStore;

pub fn run(home: PathBuf, bundled: PathBuf, only: Vec<String>, cached: Option<PathBuf>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let store = HubStore::new(home.clone())?;
        println!("selftest: home = {}", home.display());
        let github = GitHubCatalog::new()?;
        let catalog = if let Some(path) = cached {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("Не удалось прочитать каталог {}: {e}", path.display()))?;
            crate::util::json_read_text(&text)?
        } else {
            let previous = github.load_cached(&store, &bundled)?;
            println!("selftest: refreshing catalog from GitHub...");
            github.refresh(&store, &previous).await?
        };
        println!("selftest: catalog has {} apps", catalog.apps.len());

        let mut targets: Vec<&crate::models::CatalogApp> = catalog
            .apps
            .iter()
            .filter(|app| only.is_empty() || only.iter().any(|id| id == &app.id))
            .collect();
        targets.sort_by(|a, b| a.id.cmp(&b.id));

        if !only.is_empty() {
            for id in &only {
                if !catalog.apps.iter().any(|app| &app.id == id) {
                    return Err(anyhow!("В каталоге нет программы {id}."));
                }
            }
        }

        let installer = PackageInstaller::new();
        let mut installed = Vec::new();
        for app in targets {
            let Some(release) = app.release.as_ref() else {
                println!(
                    "SKIP {}: нет выпуска ({})",
                    app.id,
                    app.release_error.as_deref().unwrap_or("")
                );
                continue;
            };
            println!("== {} {} ==", app.id, release.version);
            let ct = CancellationToken::new();
            let mut last_bucket = i32::MIN;
            let archive = github
                .download(
                    &store,
                    app,
                    |progress| {
                        let bucket = (progress * 20.0).floor() as i32;
                        if bucket != last_bucket {
                            last_bucket = bucket;
                            print!(" {}%", (progress * 100.0).round() as i32);
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                        }
                    },
                    &ct,
                )
                .await?;
            println!();
            let manifest = installer.install(&store, &archive, app, &ct).await?;
            let _ = std::fs::remove_file(&archive);
            let record = store
                .installed(&app.id)
                .ok_or_else(|| anyhow!("{} не попала в hub.json.", app.id))?;
            let present = store.app_present(&record);
            println!(
                "OK {} {} present={}",
                app.id, manifest.version, present
            );
            if !present {
                return Err(anyhow!("{} установлена, но не найдена на диске.", app.id));
            }
            installed.push((app.id.clone(), manifest.version));
        }

        if let Some((id, version)) = installed.first() {
            let dir = store.app_version(id, version)?;
            let hidden = dir.with_extension("missing-test");
            std::fs::rename(&dir, &hidden)?;
            let missing = store
                .settings_view()
                .installed
                .iter()
                .find(|app| &app.id == id)
                .map(|app| app.missing)
                .unwrap_or(false);
            println!("MISSING {id} while hidden = {missing}");
            std::fs::rename(&hidden, &dir)?;
            let restored = store
                .settings_view()
                .installed
                .iter()
                .find(|app| &app.id == id)
                .map(|app| app.missing)
                .unwrap_or(true);
            println!("MISSING {id} after restore = {restored}");
            if !missing || restored {
                return Err(anyhow!("Определение отсутствующих файлов работает неверно."));
            }

            std::fs::rename(&dir, &hidden)?;
            installer.uninstall(&store, id, false).await?;
            let still = store.installed(id).is_some();
            println!("UNINSTALL-MISSING {id} record_left={still}");
            if still {
                return Err(anyhow!("Удаление отсутствующей программы не убрало запись."));
            }
            let _ = std::fs::remove_dir_all(&hidden);
        }

        println!(
            "SELFTEST OK: installed={} home={}",
            installed.len(),
            home.display()
        );
        Ok(())
    })
}
