//! Port of the key `tests/ThodHub.Tests` scenarios to Rust.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::backups::BackupStore;
use crate::contract;
use crate::github::hash_bytes;
use crate::installer::PackageInstaller;
use crate::models::{CatalogApp, ModuleManifest, PackageRelease};
use crate::store::HubStore;
use crate::updater::HubUpdater;
use crate::util;

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "thodhub-tests-{name}-{}",
            util::random_token()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn make_package(
    dir: &Path,
    version: &str,
    extra_path: Option<&str>,
    hub_api: i32,
) -> PathBuf {
    let path = dir.join(format!("package-{}.zip", util::random_token()));
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    let manifest = ModuleManifest {
        id: "test-app".into(),
        version: version.into(),
        ui: Some("ui/index.html".into()),
        hub_api,
        ..Default::default()
    };
    zip.start_file("manifest.json", options).unwrap();
    zip.write_all(serde_json::to_string(&manifest).unwrap().as_bytes())
        .unwrap();
    zip.start_file("ui/index.html", options).unwrap();
    zip.write_all(b"<!doctype html><title>Test</title>").unwrap();
    if let Some(extra) = extra_path {
        zip.start_file(extra, options).unwrap();
        zip.write_all(b"outside").unwrap();
    }
    zip.finish().unwrap();
    path
}

fn describe(path: &Path, version: &str, repository: &str) -> PackageRelease {
    let bytes = std::fs::read(path).unwrap();
    PackageRelease {
        version: version.into(),
        package_url: format!(
            "https://github.com/{repository}/releases/download/v{version}/module.zip"
        ),
        sha256: hash_bytes(&bytes),
        size: bytes.len() as i64,
        notes: "test".into(),
        platform: "win-x64".into(),
        hub_api: 1,
        published_at: None,
    }
}

fn catalog_app(release: PackageRelease) -> CatalogApp {
    CatalogApp {
        id: "test-app".into(),
        name: "Test".into(),
        description: "Test".into(),
        repository: "ThodProd/Test".into(),
        release: Some(release),
        ..Default::default()
    }
}

#[test]
fn paths_reject_traversal_streams_and_reserved_names() {
    let root = TestDir::new("paths");
    for bad in [
        "../out.exe",
        "a/../../out.exe",
        "C:/out",
        "a:stream",
        "a\\b",
        "nul.txt",
        "a./x",
        "a /x",
        "/root",
    ] {
        assert!(
            contract::inside(root.path(), bad).is_err(),
            "path accepted: {bad}"
        );
    }
}

#[test]
fn semantic_versions_are_compared_strictly() {
    assert!(contract::is_newer("1.10.0", "1.9.0").unwrap());
    assert!(!contract::is_newer("1.0.0", "2.0.0").unwrap());
    assert!(contract::version("1.02.0").is_err());
}

#[test]
fn json_accepts_utf8_bom() {
    let catalog: crate::models::Catalog =
        util::json_read_text("\u{feff}{\"schemaVersion\":1,\"apps\":[]}").unwrap();
    assert!(catalog.apps.is_empty());
}

#[test]
fn manifests_require_integration_cleanup_and_safe_paths() {
    let mut manifest = ModuleManifest {
        id: "integrated-app".into(),
        version: "1.0.0".into(),
        mode: "standalone".into(),
        launch: Some(crate::models::WorkerDefinition {
            executable: "app/App.exe".into(),
            arguments: Vec::new(),
        }),
        integration: Some(crate::models::WorkerDefinition {
            executable: "app/App.exe".into(),
            arguments: vec!["--register-integrations".into()],
        }),
        cleanup: Some(crate::models::WorkerDefinition {
            executable: "app/App.exe".into(),
            arguments: vec!["--uninstall-context".into()],
        }),
        ..Default::default()
    };
    contract::manifest(&manifest).unwrap();
    manifest.stop = Some(crate::models::WorkerDefinition {
        executable: "app/App.exe".into(),
        arguments: vec!["--stop".into()],
    });
    contract::manifest(&manifest).unwrap();

    let mut bad = manifest.clone();
    bad.stop = Some(crate::models::WorkerDefinition {
        executable: "../outside.exe".into(),
        arguments: Vec::new(),
    });
    assert!(contract::manifest(&bad).is_err());

    bad = manifest.clone();
    bad.cleanup = None;
    assert!(contract::manifest(&bad).is_err());

    bad = manifest.clone();
    bad.integration = Some(crate::models::WorkerDefinition {
        executable: "app/App.exe".into(),
        arguments: vec!["invalid\0argument".into()],
    });
    assert!(contract::manifest(&bad).is_err());
}

#[tokio::test]
async fn install_update_rollback_and_uninstall_preserve_data() {
    let dir = TestDir::new("lifecycle");
    let store = HubStore::new(dir.path().join("home")).unwrap();
    let installer = PackageInstaller::new();
    let ct = CancellationToken::new();

    let v1 = make_package(dir.path(), "1.0.0", None, 1);
    let app = catalog_app(describe(&v1, "1.0.0", "ThodProd/Test"));
    installer.install(&store, &v1, &app, &ct).await.unwrap();
    assert_eq!(store.manifest("test-app").unwrap().version, "1.0.0");

    store
        .save_module_settings("test-app", &json!({ "value": "preserve" }))
        .unwrap();

    let v2 = make_package(dir.path(), "1.1.0", None, 1);
    let app2 = catalog_app(describe(&v2, "1.1.0", "ThodProd/Test"));
    installer.install(&store, &v2, &app2, &ct).await.unwrap();
    assert_eq!(store.manifest("test-app").unwrap().version, "1.1.0");

    installer.rollback(&store, "test-app").await.unwrap();
    assert_eq!(store.manifest("test-app").unwrap().version, "1.0.0");
    assert_eq!(
        store
            .load_module_settings("test-app")
            .unwrap()
            .unwrap()
            .get("value")
            .and_then(|v| v.as_str()),
        Some("preserve")
    );

    installer.uninstall(&store, "test-app", false).await.unwrap();
    assert!(store.installed("test-app").is_none());
    assert!(!store.apps_root().join("test-app").exists());
    assert_eq!(
        store
            .load_module_settings("test-app")
            .unwrap()
            .unwrap()
            .get("value")
            .and_then(|v| v.as_str()),
        Some("preserve")
    );
}

#[tokio::test]
async fn checksum_mismatch_and_bad_api_keep_working_version() {
    let dir = TestDir::new("verify");
    let store = HubStore::new(dir.path().join("home")).unwrap();
    let installer = PackageInstaller::new();
    let ct = CancellationToken::new();

    let v1 = make_package(dir.path(), "1.0.0", None, 1);
    let app = catalog_app(describe(&v1, "1.0.0", "ThodProd/Test"));
    installer.install(&store, &v1, &app, &ct).await.unwrap();

    let mut bad = app.clone();
    bad.release = Some(PackageRelease {
        sha256: "0".repeat(64),
        ..app.release.clone().unwrap()
    });
    assert!(installer.install(&store, &v1, &bad, &ct).await.is_err());
    assert_eq!(store.manifest("test-app").unwrap().version, "1.0.0");

    let incompatible = make_package(dir.path(), "1.1.0", None, 999);
    let app2 = catalog_app(describe(&incompatible, "1.1.0", "ThodProd/Test"));
    assert!(
        installer
            .install(&store, &incompatible, &app2, &ct)
            .await
            .is_err()
    );
    assert_eq!(store.manifest("test-app").unwrap().version, "1.0.0");
}

#[tokio::test]
async fn traversal_archive_is_rejected_before_activation() {
    let dir = TestDir::new("traversal");
    let store = HubStore::new(dir.path().join("home")).unwrap();
    let installer = PackageInstaller::new();
    let ct = CancellationToken::new();

    let v1 = make_package(dir.path(), "1.0.0", None, 1);
    let app = catalog_app(describe(&v1, "1.0.0", "ThodProd/Test"));
    installer.install(&store, &v1, &app, &ct).await.unwrap();

    let evil = make_package(dir.path(), "1.1.0", Some("../escaped.txt"), 1);
    let evil_app = catalog_app(describe(&evil, "1.1.0", "ThodProd/Test"));
    assert!(
        installer
            .install(&store, &evil, &evil_app, &ct)
            .await
            .is_err()
    );
    assert!(!store.apps_root().join("escaped.txt").exists());
    assert_eq!(store.manifest("test-app").unwrap().version, "1.0.0");
}

#[tokio::test]
async fn missing_app_is_marked_and_can_be_uninstalled() {
    let dir = TestDir::new("missing");
    let store = HubStore::new(dir.path().join("home")).unwrap();
    let installer = PackageInstaller::new();
    let ct = CancellationToken::new();

    let v1 = make_package(dir.path(), "1.0.0", None, 1);
    let app = catalog_app(describe(&v1, "1.0.0", "ThodProd/Test"));
    installer.install(&store, &v1, &app, &ct).await.unwrap();

    let version_dir = store.app_version("test-app", "1.0.0").unwrap();
    let hidden = version_dir.with_extension("hidden");
    std::fs::rename(&version_dir, &hidden).unwrap();

    let record = store.installed("test-app").unwrap();
    assert!(!store.app_present(&record));
    let view = store.settings_view();
    assert!(view.installed.iter().find(|a| a.id == "test-app").unwrap().missing);

    installer.uninstall(&store, "test-app", false).await.unwrap();
    assert!(store.installed("test-app").is_none());
    let _ = std::fs::remove_dir_all(&hidden);
}

#[tokio::test]
async fn backups_create_restore_and_reject_traversal() {
    let dir = TestDir::new("backups");
    let store = HubStore::new(dir.path().join("home")).unwrap();
    let ct = CancellationToken::new();
    store
        .save_module_settings("backup-app", &json!({ "value": "first" }))
        .unwrap();
    let backups = BackupStore::new(&store);
    let first = backups.create("backup-app", None, &ct).await.unwrap();

    store
        .save_module_settings("backup-app", &json!({ "value": "second" }))
        .unwrap();
    let second = backups.create("backup-app", None, &ct).await.unwrap();
    assert_ne!(first.name, second.name);
    assert_eq!(backups.list().unwrap().len(), 2);

    backups
        .restore(&first.name, "backup-app", &ct)
        .await
        .unwrap();
    assert_eq!(
        store
            .load_module_settings("backup-app")
            .unwrap()
            .unwrap()
            .get("value")
            .and_then(|v| v.as_str()),
        Some("first")
    );
    assert_eq!(backups.list().unwrap().len(), 3);

    let source = backups.file_path(&first.name).unwrap();
    // Append a traversal entry by rebuilding the archive.
    let rebuilt = dir.path().join("evil.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&rebuilt).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&source).unwrap()).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().to_string();
        if name == "data/preferences.json" {
            continue;
        }
        writer.start_file(name, options).unwrap();
        std::io::copy(&mut entry, &mut writer).unwrap();
    }
    writer.start_file("data/../../escape.txt", options).unwrap();
    writer.write_all(b"bad").unwrap();
    writer.finish().unwrap();

    assert!(
        backups
            .import(&rebuilt, "backup-app", &ct)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .load_module_settings("backup-app")
            .unwrap()
            .unwrap()
            .get("value")
            .and_then(|v| v.as_str()),
        Some("first")
    );
}

#[test]
fn hub_update_archive_extracts_and_rejects_traversal() {
    let dir = TestDir::new("updater");
    let good = dir.path().join("good.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&good).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("ThodHUB.exe", options).unwrap();
    writer.write_all(b"hub").unwrap();
    writer.start_file("wwwroot/index.html", options).unwrap();
    writer.write_all(b"<!doctype html>").unwrap();
    writer.finish().unwrap();

    let stage = HubUpdater::extract(&good, &dir.path().join("hub-update-stage")).unwrap();
    assert!(stage.join("ThodHUB.exe").is_file());
    assert!(stage.join("wwwroot/index.html").is_file());

    let bad = dir.path().join("bad.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&bad).unwrap());
    writer.start_file("../escape.txt", options).unwrap();
    writer.write_all(b"bad").unwrap();
    writer.finish().unwrap();
    assert!(HubUpdater::extract(&bad, &dir.path().join("hub-update-bad-stage")).is_err());
    assert!(!dir.path().join("escape.txt").exists());
}
