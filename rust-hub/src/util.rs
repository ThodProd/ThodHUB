use std::ffi::OsStr;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_YESNO, SW_SHOWNORMAL,
};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub fn to_wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn absolute(path: impl AsRef<Path>) -> Result<PathBuf> {
    std::path::absolute(path.as_ref()).context("Некорректный путь.")
}

pub fn json_read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let mut bytes = std::fs::read(path)?;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(0..3);
    }
    serde_json::from_slice(&bytes).map_err(|e| anyhow!("Некорректный JSON: {e}"))
}

pub fn json_read_text<T: DeserializeOwned>(text: &str) -> Result<T> {
    let text = text.trim_start_matches('\u{FEFF}');
    serde_json::from_str(text).map_err(|e| anyhow!("Некорректный JSON: {e}"))
}

pub fn json_write<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

pub fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        uuid::Uuid::new_v4().simple()
    ));
    let write = || -> Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
        file.sync_all().ok();
        Ok(())
    };
    let result = write();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode_upper(hasher.finalize()))
}

pub async fn sha256_file_async(path: &Path) -> Result<String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

pub fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn same_prefix(path: &Path, root: &Path) -> bool {
    let path_text = path.to_string_lossy().replace('/', "\\");
    let root_text = root.to_string_lossy().replace('/', "\\");
    let prefix = format!("{}\\", root_text.trim_end_matches('\\'));
    path_text.to_lowercase().starts_with(&prefix.to_lowercase())
}

fn metadata_attributes(path: &Path) -> Result<Option<u32>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta.file_attributes())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Mirrors ManagedFiles.Check: confines a path to the hub folder and forbids reparse points.
pub fn managed_check(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = absolute(root)?;
    let full = absolute(path)?;
    if !same_prefix(&full, &root) {
        return Err(anyhow!("Операция выходит за пределы папки хаба."));
    }
    let mut current = full.parent().map(|p| p.to_path_buf());
    while let Some(dir) = current {
        if dir.as_os_str().len() >= root.as_os_str().len() {
            if let Some(attributes) = metadata_attributes(&dir)? {
                if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(anyhow!(
                        "Папка хаба не должна проходить через символическую ссылку."
                    ));
                }
            }
        }
        if dir == root {
            break;
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }
    Ok(full)
}

/// Mirrors ManagedFiles.AssertUnlocked: no reparse points and no files opened by other processes.
pub fn assert_unlocked(root: &Path, path: &Path) -> Result<()> {
    let path = managed_check(root, path)?;
    if !path.is_dir() {
        return Ok(());
    }
    let attributes = std::fs::symlink_metadata(&path)?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(anyhow!("Каталог программы заменён ссылкой. Удаление отменено."));
    }
    for item in std::fs::read_dir(&path)? {
        let item = item?.path();
        managed_check(root, &item)?;
        let attributes = std::fs::symlink_metadata(&item)?.file_attributes();
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            continue;
        }
        if item.is_dir() {
            assert_unlocked(root, &item)?;
        } else {
            open_exclusive(&item)?;
        }
    }
    Ok(())
}

fn open_exclusive(path: &Path) -> Result<()> {
    let wide = to_wide(&path.to_string_lossy());
    unsafe {
        let handle: Result<HANDLE, _> = CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        );
        match handle {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                Ok(())
            }
            Err(_) => Err(anyhow!("Файл занят другой программой.")),
        }
    }
}

/// Mirrors ManagedFiles.DeleteTree.
pub fn delete_tree(root: &Path, path: &Path) -> Result<()> {
    let path = managed_check(root, path)?;
    if !path.exists() {
        return Ok(());
    }
    let attributes = std::fs::symlink_metadata(&path)?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        if path.is_dir() {
            std::fs::remove_dir(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    }
    if path.is_dir() {
        for item in std::fs::read_dir(&path)? {
            let item = item?.path();
            managed_check(root, &item)?;
            let attributes = std::fs::symlink_metadata(&item)?.file_attributes();
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                if item.is_dir() {
                    let _ = std::fs::remove_dir(&item);
                } else {
                    let _ = std::fs::remove_file(&item);
                }
            } else if item.is_dir() {
                delete_tree(root, &item)?;
            } else {
                if attributes & 0x1 != 0 {
                    let mut permissions = std::fs::metadata(&item)?.permissions();
                    permissions.set_readonly(false);
                    let _ = std::fs::set_permissions(&item, permissions);
                }
                std::fs::remove_file(&item)?;
            }
        }
        std::fs::remove_dir(&path)?;
    } else {
        if attributes & 0x1 != 0 {
            let mut permissions = std::fs::metadata(&path)?.permissions();
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&path, permissions);
        }
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn open_path(path: &Path) -> Result<()> {
    shell_execute(&path.to_string_lossy(), false)
}

pub fn open_url(url: &str) -> Result<()> {
    shell_execute(url, true)
}

fn shell_execute(target: &str, is_url: bool) -> Result<()> {
    let verb = to_wide("open");
    let file = to_wide(target);
    let parameters = to_wide(if is_url { "" } else { "" });
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(parameters.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        return Err(anyhow!("Не удалось открыть: {target}"));
    }
    Ok(())
}

pub fn message_box(text: &str, title: &str, warning: bool, yes_no: bool) -> bool {
    let text = to_wide(text);
    let title = to_wide(title);
    let flags = if yes_no { MB_YESNO } else { MB_OK }
        | if warning { MB_ICONWARNING } else { MB_ICONERROR };
    let result = unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), flags) };
    result == windows::Win32::UI::WindowsAndMessaging::IDYES
}

pub fn show_error(text: &str) {
    message_box(text, "ThodHUB", false, false);
}

pub fn confirm_discard(dirty: bool) -> bool {
    !dirty
        || message_box(
            "В открытой программе есть несохранённые изменения. Сначала сохраните проект, чтобы продолжить работу позже.\n\nВсё равно закрыть программу?",
            "Несохранённые изменения",
            true,
            true,
        )
}

pub fn copy_text(text: &str) -> Result<()> {
    let mut last: Option<anyhow::Error> = None;
    for _ in 0..10 {
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string()).map_err(Into::into)) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error.into());
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("Не удалось скопировать текст в буфер обмена.")))
}

pub fn escape_url_component(value: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS};
    const SET: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'&')
        .add(b'\'')
        .add(b'(')
        .add(b')')
        .add(b'*')
        .add(b'+')
        .add(b',')
        .add(b'/')
        .add(b':')
        .add(b';')
        .add(b'<')
        .add(b'=')
        .add(b'>')
        .add(b'?')
        .add(b'@')
        .add(b'[')
        .add(b']')
        .add(b'\\')
        .add(b'^')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}');
    percent_encoding::utf8_percent_encode(value, SET).to_string()
}

pub fn backup_timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%Y%m%dT%H%M%S%.3fZ").to_string()
}

pub fn parse_backup_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S%.3fZ")
        .ok()
        .map(|d| d.and_utc())
}

pub fn host_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn host_name() -> String {
    format!("hub-v{}.thodhub.local", host_version().replace('.', "-"))
}

pub fn module_host(id: &str, version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{id}/{version}").as_bytes());
    let hash = hex::encode(hasher.finalize()).to_lowercase();
    format!("module-{}.thodhub.local", &hash[..24])
}

pub fn random_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn valid_file_name(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (ch as u32) < 32 {
            result.push('_');
        } else {
            result.push(ch);
        }
    }
    result.trim().trim_end_matches('.').to_string()
}

pub fn installed_home() -> PathBuf {
    if let Ok(value) = std::env::var("THODHUB_HOME") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
        std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into())
    });
    Path::new(&local).join("ThodHUB")
}

pub fn process_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn atomic_temp_path(directory: &Path, prefix: &str, suffix: &str) -> PathBuf {
    directory.join(format!("{prefix}{}{suffix}", random_token()))
}

#[allow(dead_code)]
pub fn os_error(context: &str, error: std::io::Error) -> anyhow::Error {
    anyhow!("{context} {}", error)
}

pub fn read_limited(path: &Path, limit: u64) -> Result<String> {
    let length = std::fs::metadata(path)?.len();
    if length > limit {
        return Err(anyhow!("Файл слишком большой."));
    }
    Ok(std::fs::read_to_string(path)?)
}
