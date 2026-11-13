use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SHGetKnownFolderPath, ShellExecuteW, ShellLink, FOLDERID_Desktop,
    KF_FLAG_DEFAULT,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

use crate::contract;
use crate::models::{InstalledApp, ModuleManifest, WorkerDefinition};
use crate::process::ModuleProcessGuard;
use crate::store::HubStore;
use crate::util;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BASE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\";

// ---------------------------------------------------------------------------
// Standalone programs and module hooks
// ---------------------------------------------------------------------------

pub struct StandaloneLauncher;

impl StandaloneLauncher {
    pub fn launch(store: &HubStore, id: &str, extra_arguments: &[String]) -> Result<()> {
        let manifest = store.manifest(id)?;
        Self::launch_manifest(store, id, &manifest, extra_arguments, true)
    }

    fn launch_manifest(
        store: &HubStore,
        id: &str,
        manifest: &ModuleManifest,
        extra_arguments: &[String],
        detached: bool,
    ) -> Result<()> {
        if manifest.mode != "standalone" || manifest.launch.is_none() {
            return Err(anyhow!("Эта программа открывается внутри хаба."));
        }
        let root = store.app_version(id, &manifest.version)?;
        let launch = manifest.launch.as_ref().unwrap();
        let exe = contract::inside(&root, &launch.executable)?;
        if !exe.is_file() {
            return Err(anyhow!(
                "Файл программы не найден. Переустановите программу."
            ));
        }
        let data = store.data_directory(id)?;
        let mut arguments: Vec<String> = launch
            .arguments
            .iter()
            .map(|a| a.replace("{dataDir}", &data.to_string_lossy()))
            .collect();
        arguments.extend(extra_arguments.iter().cloned());
        if manifest.requires_elevation {
            let parameters = arguments
                .iter()
                .map(|a| quote_argument(a))
                .collect::<Vec<_>>()
                .join(" ");
            let file = util::to_wide(&exe.to_string_lossy());
            let params = util::to_wide(&parameters);
            let directory = util::to_wide(
                &exe.parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            let verb = util::to_wide("runas");
            let result = unsafe {
                ShellExecuteW(
                    None,
                    PCWSTR(verb.as_ptr()),
                    PCWSTR(file.as_ptr()),
                    PCWSTR(params.as_ptr()),
                    PCWSTR(directory.as_ptr()),
                    SW_SHOWNORMAL,
                )
            };
            if result.0 as isize <= 32 {
                return Err(anyhow!("Не удалось запустить программу с правами администратора."));
            }
            return Ok(());
        }
        let mut command = std::process::Command::new(&exe);
        command
            .args(&arguments)
            .current_dir(exe.parent().unwrap_or(&root))
            .env("THODHUB_DATA_DIR", &data)
            .env("THODHUB_API", "1")
            .env("THODHUB_LAUNCHER", util::process_path());
        command.creation_flags(CREATE_NO_WINDOW);
        if detached {
            command
                .spawn()
                .map_err(|e| anyhow!("Не удалось запустить программу. {e}"))?;
        } else {
            command
                .status()
                .map_err(|e| anyhow!("Не удалось запустить программу. {e}"))?;
        }
        Ok(())
    }

    pub async fn cleanup(store: &HubStore, id: &str) -> Result<()> {
        let manifest = store.manifest(id)?;
        Self::run_hook(store, id, &manifest, manifest.cleanup.as_ref()).await
    }

    pub async fn stop(store: &HubStore, id: &str) -> Result<()> {
        let manifest = store.manifest(id)?;
        Self::run_hook(store, id, &manifest, manifest.stop.as_ref()).await
    }

    pub async fn integrate(store: &HubStore, id: &str) -> Result<()> {
        let manifest = store.manifest(id)?;
        Self::run_hook(store, id, &manifest, manifest.integration.as_ref()).await
    }

    pub async fn stop_for_maintenance(store: &HubStore, id: &str) -> Result<()> {
        let app = store.installed(id);
        // Missing files cannot host a running module: skip manifest-dependent cleanup.
        let manifest = match &app {
            Some(app) if store.app_present(app) => store.manifest(id).ok(),
            _ => None,
        };
        let Some(manifest) = manifest else {
            return Ok(());
        };
        if let Err(error) = Self::run_hook(store, id, &manifest, manifest.stop.as_ref()).await {
            eprintln!("[hub] stop hook failed for {id}: {error}");
        }
        let grace = if manifest.stop.is_none() {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(6)
        };
        ModuleProcessGuard::stop(store, id, grace, true).await
    }

    pub fn assert_not_running(store: &HubStore, id: &str) -> Result<()> {
        ModuleProcessGuard::assert_not_running(store, id)
    }

    pub async fn run_hook(
        store: &HubStore,
        id: &str,
        manifest: &ModuleManifest,
        hook: Option<&WorkerDefinition>,
    ) -> Result<()> {
        let Some(hook) = hook else {
            return Ok(());
        };
        let root = store.app_version(id, &manifest.version)?;
        let exe = contract::inside(&root, &hook.executable)?;
        if !exe.is_file() {
            return Ok(());
        }
        let data = store.data_directory(id)?;
        let arguments: Vec<String> = hook
            .arguments
            .iter()
            .map(|a| a.replace("{dataDir}", &data.to_string_lossy()))
            .collect();
        let mut command = tokio::process::Command::new(&exe);
        command
            .args(&arguments)
            .current_dir(&root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env("THODHUB_DATA_DIR", &data)
            .env("THODHUB_API", "1")
            .env("THODHUB_LAUNCHER", util::process_path());
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| anyhow!("Не удалось запустить настройку интеграции Windows. {e}"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let read_stdout = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buffer = String::new();
            if let Some(mut stdout) = stdout {
                let _ = stdout.read_to_string(&mut buffer).await;
            }
            buffer
        });
        let read_stderr = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buffer = String::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_string(&mut buffer).await;
            }
            buffer
        });
        let status = match tokio::time::timeout(Duration::from_secs(45), child.wait()).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                return Err(anyhow!(
                    "Настройка интеграции Windows не завершилась за 45 секунд."
                ));
            }
        };
        let _stdout = read_stdout.await.unwrap_or_default();
        let stderr = read_stderr.await.unwrap_or_default();
        if !status.success() {
            return Err(anyhow!(
                "Ошибка интеграции Windows (код {}). {}",
                status.code().unwrap_or(-1),
                stderr
            ));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Desktop shortcuts (native IShellLink instead of WScript.Shell)
    // -----------------------------------------------------------------------

    fn shortcut_path(id: &str, name: &str) -> Result<PathBuf> {
        contract::id(id)?;
        let mut file: String = name
            .chars()
            .map(|c| if invalid_file_char(c) { '_' } else { c })
            .collect();
        file = file.trim().trim_end_matches('.').to_string();
        if file.is_empty() || file.chars().count() > 100 {
            file = id.to_string();
        }
        let desktop = desktop_directory()?;
        contract::inside(&desktop, &format!("{file}.lnk"))
    }

    fn own_shortcut(link: &IShellLinkW, id: &str) -> bool {
        let arguments = read_link_arguments(link);
        let target = read_link_target(link);
        let description = read_link_description(link);
        arguments == format!("--launch {id}")
            && Path::new(&target)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("ThodHUB.exe"))
            && description.ends_with(" · ThodHUB")
    }

    pub fn create_shortcut(store: &HubStore, id: &str, name: &str) -> Result<()> {
        let manifest = store.manifest(id)?;
        let path = Self::shortcut_path(id, name)?;
        let _com = ComScope::new();
        let link = create_shell_link()?;
        if path.exists() {
            let persist: windows::Win32::System::Com::IPersistFile = link.cast()?;
            let wide = util::to_wide(&path.to_string_lossy());
            unsafe {
                persist.Load(PCWSTR(wide.as_ptr()), STGM_READ)?;
            }
            if !Self::own_shortcut(&link, id) {
                return Err(anyhow!(
                    "На рабочем столе уже существует чужой ярлык «{name}». Переименуйте его перед созданием ярлыка из хаба."
                ));
            }
        }
        let exe = util::process_path();
        let working = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_default();
        let version_root = store.app_version(id, &manifest.version)?;
        let icon = manifest
            .launch
            .as_ref()
            .or(manifest.worker.as_ref())
            .map(|entry| {
                contract::inside(&version_root, &entry.executable)
                    .map(|p| p.to_string_lossy().to_string())
            })
            .transpose()?
            .unwrap_or_else(|| exe.clone());
        let exe_w = util::to_wide(&exe);
        let args_w = util::to_wide(&format!("--launch {id}"));
        let working_w = util::to_wide(&working);
        let description_w = util::to_wide(&format!("{name} · ThodHUB"));
        let icon_w = util::to_wide(&icon);
        let path_w = util::to_wide(&path.to_string_lossy());
        unsafe {
            link.SetPath(PCWSTR(exe_w.as_ptr()))?;
            link.SetArguments(PCWSTR(args_w.as_ptr()))?;
            link.SetWorkingDirectory(PCWSTR(working_w.as_ptr()))?;
            link.SetDescription(PCWSTR(description_w.as_ptr()))?;
            link.SetIconLocation(PCWSTR(icon_w.as_ptr()), 0)?;
            let persist: windows::Win32::System::Com::IPersistFile = link.cast()?;
            persist.Save(PCWSTR(path_w.as_ptr()), true)?;
        }
        if let Err(error) = Self::remove_shortcut(id, Some(&path)) {
            eprintln!("[hub] duplicate shortcut cleanup failed: {error}");
        }
        Ok(())
    }

    pub fn remove_shortcut(id: &str, except: Option<&Path>) -> Result<()> {
        contract::id(id)?;
        let desktop = desktop_directory()?;
        let _com = ComScope::new();
        let entries = match std::fs::read_dir(&desktop) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| !e.eq_ignore_ascii_case("lnk"))
                .unwrap_or(true)
            {
                continue;
            }
            if let Some(except) = except {
                if path.to_string_lossy().eq_ignore_ascii_case(&except.to_string_lossy()) {
                    continue;
                }
            }
            let link = match create_shell_link() {
                Ok(link) => link,
                Err(_) => continue,
            };
            let Ok(persist) =
                link.cast::<windows::Win32::System::Com::IPersistFile>()
            else {
                continue;
            };
            let wide = util::to_wide(&path.to_string_lossy());
            if unsafe { persist.Load(PCWSTR(wide.as_ptr()), STGM_READ) }.is_err() {
                continue;
            }
            if Self::own_shortcut(&link, id) {
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(())
    }
}

fn invalid_file_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32
}

fn quote_argument(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return value.to_string();
    }
    let mut result = String::from("\"");
    let mut backslashes = 0usize;
    for ch in value.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                result.push_str(&"\\".repeat(backslashes * 2 + 1));
                result.push('"');
                backslashes = 0;
            }
            _ => {
                result.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                result.push(ch);
            }
        }
    }
    result.push_str(&"\\".repeat(backslashes * 2));
    result.push('"');
    result
}

// ---------------------------------------------------------------------------
// Windows "Apps & features" entries
// ---------------------------------------------------------------------------

pub struct WindowsInstallation;

impl WindowsInstallation {
    pub fn register(store: &HubStore, id: &str, name: &str) -> Result<()> {
        let manifest = store.manifest(id)?;
        let key_name = format!("{BASE_KEY}ThodHUB.{id}");
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(existing) = hkcu.open_subkey_with_flags(&key_name, KEY_READ) {
            let owner: String = existing.get_value("ThodHUB.Owner").unwrap_or_default();
            if !owner.eq_ignore_ascii_case(&store.root().to_string_lossy()) {
                return Err(anyhow!(
                    "Программа уже зарегистрирована другим экземпляром хаба."
                ));
            }
        }
        let (key, _) = hkcu.create_subkey(&key_name)?;
        let directory = store.app_version(id, &manifest.version)?;
        let uninstall = format!("\"{}\" --uninstall {id}", util::process_path());
        let exe = manifest.launch.as_ref().or(manifest.worker.as_ref());
        let icon = match exe {
            Some(entry) => contract::inside(&directory, &entry.executable)?
                .to_string_lossy()
                .to_string(),
            None => util::process_path(),
        };
        key.set_value("ThodHUB.Owner", &store.root().to_string_lossy().to_string())?;
        key.set_value("DisplayName", &name)?;
        key.set_value("DisplayVersion", &manifest.version)?;
        key.set_value("Publisher", &"ThodProd")?;
        key.set_value("InstallLocation", &directory.to_string_lossy().to_string())?;
        key.set_value("UninstallString", &uninstall)?;
        key.set_value("NoModify", &1u32)?;
        key.set_value("NoRepair", &1u32)?;
        key.set_value("DisplayIcon", &format!("\"{icon}\",0"))?;
        Ok(())
    }

    pub fn remove(store: &HubStore, id: &str) -> Result<()> {
        contract::id(id)?;
        let key_name = format!("{BASE_KEY}ThodHUB.{id}");
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(key) = hkcu.open_subkey_with_flags(&key_name, KEY_READ) else {
            return Ok(());
        };
        let owner: String = key.get_value("ThodHUB.Owner").unwrap_or_default();
        if owner.eq_ignore_ascii_case(&store.root().to_string_lossy()) {
            hkcu.delete_subkey_all(&key_name)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DPAPI-protected GitHub token
// ---------------------------------------------------------------------------

pub struct GitHubCredentials {
    root: PathBuf,
}

impl GitHubCredentials {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn path(&self) -> PathBuf {
        self.root.join("Secrets").join("github.bin")
    }

    pub fn exists(&self) -> bool {
        self.path().is_file()
    }

    pub fn save(&self, token: &str) -> Result<()> {
        let token = token.trim();
        if token.chars().count() < 20
            || token.chars().count() > 500
            || token.chars().any(char::is_whitespace)
        {
            return Err(anyhow!("Некорректный токен GitHub."));
        }
        if let Some(parent) = self.path().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let protected = dpapi_transform(token.as_bytes(), true)?;
        std::fs::write(self.path(), protected)?;
        Ok(())
    }

    pub fn read(&self) -> Result<String> {
        let bytes = std::fs::read(self.path())?;
        let clear = dpapi_transform(&bytes, false)?;
        String::from_utf8(clear).map_err(|_| anyhow!("Некорректный токен GitHub."))
    }

    pub fn forget(&self) -> Result<()> {
        if self.exists() {
            std::fs::remove_file(self.path())?;
        }
        Ok(())
    }

    pub async fn read_cli() -> Result<String> {
        let mut candidates: Vec<PathBuf> = std::env::var("PATH")
            .unwrap_or_default()
            .split(';')
            .filter(|p| !p.is_empty())
            .map(|p| Path::new(p).join("gh.exe"))
            .collect();
        if let Ok(exe) = std::env::current_exe() {
            let mut current = exe.parent().map(|p| p.to_path_buf());
            while let Some(dir) = current {
                candidates.push(dir.join(".tools").join("gh").join("bin").join("gh.exe"));
                current = dir.parent().map(|p| p.to_path_buf());
            }
        }
        let exe = candidates
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| anyhow!("GitHub CLI не найден. Введите токен только для приватного репозитория в поле ниже."))?;
        let mut child = tokio::process::Command::new(&exe)
            .args(["auth", "token", "--hostname", "github.com"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|_| anyhow!("GitHub CLI не найден. Введите токен только для приватного репозитория в поле ниже."))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buffer = String::new();
            if let Some(mut out) = stdout {
                let _ = out.read_to_string(&mut buffer).await;
            }
            buffer
        });
        let err_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buffer = String::new();
            if let Some(mut err) = stderr {
                let _ = err.read_to_string(&mut buffer).await;
            }
            buffer
        });
        match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
            Ok(Ok(status)) => {
                let output = out_task.await.unwrap_or_default();
                let _ = err_task.await;
                if !status.success() {
                    return Err(anyhow!(
                        "В GitHub CLI нет сохранённого входа. Выполните gh auth login или укажите токен."
                    ));
                }
                Ok(output.trim().to_string())
            }
            Ok(Err(error)) => Err(anyhow!("GitHub CLI не ответил. {error}")),
            Err(_) => {
                let _ = child.start_kill();
                Err(anyhow!("GitHub CLI не ответил."))
            }
        }
    }
}

fn dpapi_transform(bytes: &[u8], protect: bool) -> Result<Vec<u8>> {
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        if protect {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(&input, None, None, None, None, 1, &mut output)
        }
    };
    input.cbData = 0;
    input.pbData = std::ptr::null_mut();
    result.map_err(|_| {
        anyhow!("Не удалось открыть сохранённый вход GitHub. Подключите его заново.")
    })?;
    let value = if output.pbData.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    unsafe {
        if !output.pbData.is_null() {
            let _ = LocalFree(Some(HLOCAL(output.pbData as *mut c_void)));
        }
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Shell helpers
// ---------------------------------------------------------------------------

struct ComScope {
    initialized: bool,
}

impl ComScope {
    fn new() -> Self {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self {
            initialized: result.is_ok(),
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn create_shell_link() -> Result<IShellLinkW> {
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    Ok(link)
}

fn read_link_target(link: &IShellLinkW) -> String {
    let mut buffer = vec![0u16; 1024];
    if unsafe { link.GetPath(&mut buffer, std::ptr::null_mut(), 0) }.is_err() {
        return String::new();
    }
    utf16z(&buffer)
}

fn read_link_arguments(link: &IShellLinkW) -> String {
    let mut buffer = vec![0u16; 1024];
    if unsafe { link.GetArguments(&mut buffer) }.is_err() {
        return String::new();
    }
    utf16z(&buffer)
}

fn read_link_description(link: &IShellLinkW) -> String {
    let mut buffer = vec![0u16; 1024];
    if unsafe { link.GetDescription(&mut buffer) }.is_err() {
        return String::new();
    }
    utf16z(&buffer)
}

fn utf16z(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn desktop_directory() -> Result<PathBuf> {
    unsafe {
        if let Ok(path) = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) {
            let result = path.to_string().map(PathBuf::from);
            CoTaskMemFree(Some(path.0 as *const c_void));
            if let Ok(result) = result {
                return Ok(result);
            }
        }
    }
    let profile = std::env::var("USERPROFILE")?;
    Ok(Path::new(&profile).join("Desktop"))
}

/// Removes the install record and Windows registrations even when files vanished.
pub fn remove_registrations(store: &HubStore, app: &InstalledApp) -> Vec<String> {
    let mut warnings = Vec::new();
    if WindowsInstallation::remove(store, &app.id).is_err() {
        warnings.push(format!(
            "Не удалось удалить запись «{}» из списка установленных программ Windows.",
            app.id
        ));
    }
    if StandaloneLauncher::remove_shortcut(&app.id, None).is_err() {
        warnings.push("Не удалось удалить ярлык с рабочего стола.".to_string());
    }
    warnings
}
