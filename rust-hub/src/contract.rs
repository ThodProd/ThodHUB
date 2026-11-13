use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use regex::Regex;

use crate::models::{Catalog, ModuleManifest, PackageRelease};

pub const API_VERSION: i32 = 1;
pub const MAX_PACKAGE_BYTES: i64 = 1024 * 1024 * 1024;

fn id_regex() -> &'static Regex {
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| Regex::new(r"^[a-z][a-z0-9-]{1,63}$").unwrap())
}
fn version_regex() -> &'static Regex {
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| Regex::new(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$").unwrap())
}
fn repo_regex() -> &'static Regex {
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9-]{0,38}/[a-zA-Z0-9_.-]{1,100}$").unwrap())
}

pub fn id(value: &str) -> Result<()> {
    if id_regex().is_match(value) {
        Ok(())
    } else {
        Err(anyhow!("Некорректный идентификатор программы."))
    }
}

pub fn parse_version(value: &str) -> Result<(i32, i32, i32)> {
    if !version_regex().is_match(value) {
        return Err(anyhow!("Версия должна иметь формат 1.2.3."));
    }
    let mut parts = value.split('.');
    let mut next = || -> Result<i32> {
        let part = parts.next().unwrap_or("0");
        let number: i64 = part
            .parse()
            .map_err(|_| anyhow!("Версия должна иметь формат 1.2.3."))?;
        if number > i32::MAX as i64 {
            return Err(anyhow!("Версия должна иметь формат 1.2.3."));
        }
        Ok(number as i32)
    };
    let major = next()?;
    let minor = next()?;
    let build = next()?;
    Ok((major, minor, build))
}

pub fn version(value: &str) -> Result<()> {
    parse_version(value).map(|_| ())
}

pub fn repository(value: &str) -> Result<()> {
    if repo_regex().is_match(value) {
        Ok(())
    } else {
        Err(anyhow!("Репозиторий должен иметь формат owner/name."))
    }
}

pub fn is_newer(candidate: &str, installed: &str) -> Result<bool> {
    Ok(parse_version(candidate)? > parse_version(installed)?)
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validates a relative path taken from a package and resolves it inside `root`.
pub fn inside(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.trim().is_empty()
        || relative.contains(':')
        || relative.contains('\\')
        || Path::new(relative).is_absolute()
        || Path::new(relative).has_root()
    {
        return Err(anyhow!("Недопустимый путь внутри пакета."));
    }
    for part in relative.split('/') {
        if part == "." || part == ".." || part.ends_with(' ') || part.ends_with('.') {
            return Err(anyhow!("Недопустимый путь внутри пакета."));
        }
        if part
            .chars()
            .any(|c| (c as u32) < 32 || matches!(c, '<' | '>' | '|' | '?' | '*'))
        {
            return Err(anyhow!("Недопустимый путь внутри пакета."));
        }
        let base = part.split('.').next().unwrap_or("");
        if RESERVED.iter().any(|name| name.eq_ignore_ascii_case(base)) {
            return Err(anyhow!("Зарезервированное имя файла Windows."));
        }
    }
    let target = root.join(relative);
    // Defence in depth: even after component validation the resolved path must stay below root.
    let root_text = root.to_string_lossy().replace('/', "\\");
    let target_text = target.to_string_lossy().replace('/', "\\");
    let prefix = format!("{}\\", root_text.trim_end_matches('\\'));
    if !target_text.to_lowercase().starts_with(&prefix.to_lowercase()) {
        return Err(anyhow!("Путь выходит за пределы пакета."));
    }
    Ok(target)
}

fn require_exe(root: &Path, value: &str, message: &str) -> Result<()> {
    inside(root, value)?;
    if !value.to_lowercase().ends_with(".exe") {
        return Err(anyhow!("{message}"));
    }
    Ok(())
}

pub fn manifest(value: &ModuleManifest) -> Result<()> {
    id(&value.id)?;
    version(&value.version)?;
    if value.schema_version != 1 || value.hub_api != API_VERSION || value.platform != "win-x64" {
        return Err(anyhow!("Пакет несовместим с этим хабом."));
    }
    if value.mode != "embedded" && value.mode != "standalone" {
        return Err(anyhow!("Неизвестный режим программы."));
    }
    let temp = std::env::temp_dir();
    if value.mode == "embedded" {
        let ui = value
            .ui
            .as_deref()
            .ok_or_else(|| anyhow!("Не задан интерфейс программы."))?;
        inside(&temp, ui)?;
        if !ui.to_lowercase().ends_with(".html") {
            return Err(anyhow!("Точка входа UI должна быть HTML."));
        }
        if value.requires_elevation {
            return Err(anyhow!(
                "Встроенный модуль не может требовать повышения прав хаба."
            ));
        }
    } else {
        let launch = value
            .launch
            .as_ref()
            .ok_or_else(|| anyhow!("Не задан запуск самостоятельной программы."))?;
        require_exe(
            &temp,
            &launch.executable,
            "Самостоятельная программа должна содержать exe.",
        )?;
        if value.worker.is_some() {
            return Err(anyhow!("Самостоятельная программа не должна объявлять worker."));
        }
    }
    if let Some(worker) = &value.worker {
        require_exe(
            &temp,
            &worker.executable,
            "Пакет должен содержать готовый Windows exe.",
        )?;
    }
    for hook in [&value.cleanup, &value.integration, &value.stop]
        .into_iter()
        .flatten()
    {
        require_exe(
            &temp,
            &hook.executable,
            "Интеграция должна выполняться exe из пакета с корректными аргументами.",
        )?;
        if hook.arguments.len() > 16
            || hook
                .arguments
                .iter()
                .any(|a| a.len() > 2048 || a.contains('\0'))
        {
            return Err(anyhow!(
                "Интеграция должна выполняться exe из пакета с корректными аргументами."
            ));
        }
    }
    if value.integration.is_some() && value.cleanup.is_none() {
        return Err(anyhow!("Интеграция Windows требует cleanup для удаления."));
    }
    let mut action_ids = std::collections::HashSet::new();
    if value.actions.len() > 8
        || (value.mode != "standalone" && !value.actions.is_empty())
        || !value
            .actions
            .iter()
            .all(|a| action_ids.insert(a.id.clone()))
    {
        return Err(anyhow!("Некорректные действия программы."));
    }
    for action in &value.actions {
        id(&action.id)?;
        if action.label.trim().is_empty()
            || action.label.len() > 80
            || action.arguments.len() > 16
            || action
                .arguments
                .iter()
                .any(|a| a.len() > 2048 || a.contains('\0'))
        {
            return Err(anyhow!("Некорректное действие программы."));
        }
    }
    let allowed = [
        "settings",
        "clipboard.write",
        "dialog.folder",
        "dialog.save",
        "files.export",
        "document.pdf",
        "document.print",
        "worker",
    ];
    if value
        .permissions
        .iter()
        .any(|p| !allowed.contains(&p.as_str()))
    {
        return Err(anyhow!("Неизвестное разрешение пакета."));
    }
    if value.worker.is_some() && !value.permissions.iter().any(|p| p == "worker") {
        return Err(anyhow!("Не объявлено разрешение worker."));
    }
    Ok(())
}

pub fn release(value: &PackageRelease, repository_name: &str) -> Result<()> {
    version(&value.version)?;
    repository(repository_name)?;
    if value.platform != "win-x64" || value.hub_api != API_VERSION {
        return Err(anyhow!("Выпуск несовместим с хабом."));
    }
    if value.size <= 0 || value.size > MAX_PACKAGE_BYTES {
        return Err(anyhow!("Некорректный размер пакета."));
    }
    if value.sha256.len() != 64 || !value.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!("Не задана SHA-256 пакета."));
    }
    let url = url::Url::parse(&value.package_url)
        .map_err(|_| anyhow!("Пакет должен быть выпуском указанного GitHub-репозитория."))?;
    let default_port = url.port().is_none() || url.port() == Some(443);
    let prefix = format!("/{repository_name}/releases/download/");
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !default_port
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with(&prefix)
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(anyhow!(
            "Пакет должен быть выпуском указанного GitHub-репозитория."
        ));
    }
    Ok(())
}

pub fn validate_catalog(catalog: &Catalog) -> Result<()> {
    if catalog.schema_version != 1 || catalog.apps.len() > 200 {
        return Err(anyhow!("Неподдерживаемый каталог."));
    }
    let mut ids = std::collections::HashSet::new();
    for app in &catalog.apps {
        id(&app.id)?;
        repository(&app.repository)?;
        if app.mode != "embedded" && app.mode != "standalone" {
            return Err(anyhow!("Некорректный режим программы."));
        }
        if !ids.insert(app.id.to_lowercase()) {
            return Err(anyhow!("Повторяющийся идентификатор программы."));
        }
        let name_len = app.name.chars().count();
        if name_len == 0 || name_len > 100 || app.description.chars().count() > 1000 {
            return Err(anyhow!("Некорректное описание программы."));
        }
        if let Some(value) = &app.release {
            release(value, &app.repository)?;
        }
        if let Some(artwork) = &app.artwork_url {
            let parsed = url::Url::parse(artwork).ok();
            if parsed.as_ref().map(|u| u.scheme()) != Some("https")
                || parsed.as_ref().and_then(|u| u.host_str()) != Some("raw.githubusercontent.com")
            {
                return Err(anyhow!("Изображения каталога должны храниться на GitHub."));
            }
        }
        if let Some(icon) = &app.icon_url {
            let parsed = url::Url::parse(icon).ok();
            let valid = parsed.as_ref().is_some_and(|u| {
                u.scheme() == "https"
                    && u.host_str() == Some("raw.githubusercontent.com")
                    && (u.port().is_none() || u.port() == Some(443))
                    && u.username().is_empty()
            });
            if !valid {
                return Err(anyhow!("Иконки каталога должны храниться на GitHub."));
            }
        }
    }
    Ok(())
}

pub fn version_from_tag(tag: &str) -> Option<String> {
    let value = if tag.to_lowercase().starts_with("hub-v") {
        &tag[5..]
    } else if let Some(rest) = tag.strip_prefix('v') {
        rest
    } else {
        tag
    };
    version(value).ok()?;
    Some(value.to_string())
}
