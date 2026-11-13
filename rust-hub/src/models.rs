use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Catalog {
    pub schema_version: i32,
    pub apps: Vec<CatalogApp>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self { schema_version: 1, apps: Vec::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub repository: String,
    pub category: String,
    pub icon: String,
    pub mode: String,
    pub artwork_url: Option<String>,
    pub icon_url: Option<String>,
    pub release: Option<PackageRelease>,
    pub release_error: Option<String>,
}
impl Default for CatalogApp {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            kind: "Программа".into(),
            repository: String::new(),
            category: "Утилиты".into(),
            icon: "app".into(),
            mode: "embedded".into(),
            artwork_url: None,
            icon_url: None,
            release: None,
            release_error: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct HubReleaseInfo {
    pub version: String,
    pub notes: String,
    pub size: i64,
    pub sha256: String,
    pub download_url: Option<String>,
    pub release_url: String,
    pub published_at: Option<DateTime<Utc>>,
}
impl Default for HubReleaseInfo {
    fn default() -> Self {
        Self {
            version: String::new(),
            notes: String::new(),
            size: 0,
            sha256: String::new(),
            download_url: None,
            release_url: String::new(),
            published_at: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PackageRelease {
    pub version: String,
    pub package_url: String,
    pub sha256: String,
    pub size: i64,
    pub notes: String,
    pub platform: String,
    pub hub_api: i32,
    pub published_at: Option<DateTime<Utc>>,
}
impl Default for PackageRelease {
    fn default() -> Self {
        Self {
            version: String::new(),
            package_url: String::new(),
            sha256: String::new(),
            size: 0,
            notes: String::new(),
            platform: "win-x64".into(),
            hub_api: 1,
            published_at: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct ModuleManifest {
    pub schema_version: i32,
    pub hub_api: i32,
    pub id: String,
    pub version: String,
    pub platform: String,
    pub mode: String,
    pub ui: Option<String>,
    pub launch: Option<WorkerDefinition>,
    pub requires_elevation: bool,
    pub worker: Option<WorkerDefinition>,
    pub cleanup: Option<WorkerDefinition>,
    pub stop: Option<WorkerDefinition>,
    pub integration: Option<WorkerDefinition>,
    pub actions: Vec<ModuleAction>,
    pub permissions: Vec<String>,
}
impl Default for ModuleManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            hub_api: 1,
            id: String::new(),
            version: String::new(),
            platform: "win-x64".into(),
            mode: "embedded".into(),
            ui: None,
            launch: None,
            requires_elevation: false,
            worker: None,
            cleanup: None,
            stop: None,
            integration: None,
            actions: Vec::new(),
            permissions: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkerDefinition {
    pub executable: String,
    pub arguments: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ModuleAction {
    pub wait_for_exit: bool,
    pub id: String,
    pub label: String,
    pub arguments: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct InstalledApp {
    pub integration_error: Option<String>,
    pub id: String,
    pub version: String,
    pub previous_version: Option<String>,
    pub installed_at: DateTime<Utc>,
    /// Computed for the UI snapshot only. Never written to hub.json.
    #[serde(default, skip_serializing_if = "is_false")]
    pub missing: bool,
}
impl Default for InstalledApp {
    fn default() -> Self {
        Self {
            integration_error: None,
            id: String::new(),
            version: String::new(),
            previous_version: None,
            installed_at: Utc::now(),
            missing: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct HubSettings {
    pub theme: String,
    pub check_on_startup: bool,
    pub auto_install_updates: bool,
    pub catalog_repository: String,
    pub catalog_ref: String,
    pub backup_repository: String,
    pub sidebar_order: Vec<String>,
    pub installed: Vec<InstalledApp>,
}
impl Default for HubSettings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            check_on_startup: true,
            auto_install_updates: false,
            catalog_repository: "ThodProd/ThodHUB-Registry".into(),
            catalog_ref: "main".into(),
            backup_repository: "ThodProd/ThodHUB-Backups".into(),
            sidebar_order: Vec::new(),
            installed: Vec::new(),
        }
    }
}
