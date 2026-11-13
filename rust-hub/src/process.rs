use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, GW_OWNER,
    WM_CLOSE,
};

use crate::models::{InstalledApp, ModuleManifest};
use crate::store::HubStore;
use crate::util;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct ModuleProcess {
    child: Mutex<tokio::process::Child>,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pending: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>>,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
    errors: Mutex<Option<tokio::task::JoinHandle<()>>>,
    lifetime: CancellationToken,
}

impl ModuleProcess {
    pub fn start(
        executable: &Path,
        arguments: &[String],
        working_directory: &Path,
        data_directory: &Path,
    ) -> Result<std::sync::Arc<Self>> {
        let mut command = tokio::process::Command::new(executable);
        command
            .args(arguments)
            .current_dir(working_directory)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env("THODHUB_DATA_DIR", data_directory)
            .env("THODHUB_API", "1")
            .env("THODHUB_LAUNCHER", util::process_path());
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| anyhow!("Не удалось запустить утилиту. {e}"))?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Не удалось подключить вывод утилиты."))?;
        let stderr = child.stderr.take();
        let pending: std::sync::Arc<
            std::sync::Mutex<
                std::collections::HashMap<
                    String,
                    tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, String>>,
                >,
            >,
        > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let lifetime = CancellationToken::new();

        let pending_reader = pending.clone();
        let alive_reader = alive.clone();
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.len() > 8 * 1024 * 1024 {
                            break;
                        }
                        let text = String::from_utf8_lossy(&line)
                            .trim_end_matches(['\r', '\n'])
                            .to_string();
                        if text.is_empty() {
                            continue;
                        }
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                            continue;
                        };
                        let Some(id) = value.get("id").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let sender = pending_reader
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(id);
                        let Some(sender) = sender else { continue };
                        if let Some(error) = value.get("error") {
                            let message = error
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Ошибка утилиты.")
                                .to_string();
                            let _ = sender.send(Err(message));
                        } else if let Some(result) = value.get("result") {
                            let _ = sender.send(Ok(result.clone()));
                        } else {
                            let _ = sender.send(Err("Некорректный ответ утилиты.".into()));
                        }
                    }
                    Err(_) => break,
                }
            }
            alive_reader.store(false, std::sync::atomic::Ordering::SeqCst);
            let mut pending = pending_reader.lock().unwrap_or_else(|e| e.into_inner());
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err("Связь с утилитой завершилась.".into()));
            }
        });

        let errors = stderr.map(|stderr| {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line).await {
                        Ok(0) => break,
                        Ok(_) => {
                            let text = String::from_utf8_lossy(&line);
                            let text = text.trim();
                            if !text.is_empty() {
                                eprintln!("[module] {text}");
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
        });

        Ok(std::sync::Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            alive,
            reader: Mutex::new(Some(reader)),
            errors: Mutex::new(errors),
            lifetime,
        }))
    }

    pub fn is_running(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        if !self.is_running() {
            return Err(anyhow!("Процесс утилиты завершён."));
        }
        let id = util::random_token();
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), sender);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or(serde_json::Value::Null),
        });
        let write = async {
            let mut guard = self.stdin.lock().await;
            let stdin = guard
                .as_mut()
                .ok_or_else(|| anyhow!("Ввод утилиты закрыт."))?;
            stdin.write_all(format!("{request}\n").as_bytes()).await?;
            stdin.flush().await?;
            Ok::<(), anyhow::Error>(())
        };
        if let Err(error) = write.await {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(Duration::from_secs(30), receiver).await {
            Ok(Ok(result)) => result.map_err(anyhow::Error::msg),
            Ok(Err(_)) => Err(anyhow!("Связь с утилитой завершилась.")),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(anyhow!("Утилита не ответила за 30 секунд."))
            }
        }
    }

    pub async fn dispose(&self) {
        self.lifetime.cancel();
        if let Some(stdin) = self.stdin.lock().await.take() {
            drop(stdin);
        }
        {
            let mut child = self.child.lock().await;
            match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                }
            }
        }
        self.alive
            .store(false, std::sync::atomic::Ordering::SeqCst);
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err("Связь с утилитой завершилась.".into()));
            }
        }
        if let Some(handle) = self.reader.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
        }
        if let Some(handle) = self.errors.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Process guard
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RunningModuleProcess {
    pub id: u32,
    pub name: String,
    pub path: Option<String>,
    pub background: bool,
    pub main_window: isize,
}

pub struct ModuleProcessGuard;

impl ModuleProcessGuard {
    pub fn find(store: &HubStore, id: &str) -> Result<Vec<RunningModuleProcess>> {
        let root = util::managed_check(&store.apps_root(), &store.apps_root().join(id))?;
        let manifest = store.manifest(id)?;
        Ok(find_entries(&root, &manifest))
    }

    pub fn assert_not_running(store: &HubStore, id: &str) -> Result<()> {
        let running = Self::find(store, id)?;
        if !running.is_empty() {
            return Err(anyhow!(running_message(&running)));
        }
        Ok(())
    }

    pub async fn wait_for_exit(
        store: &HubStore,
        id: &str,
        timeout: Duration,
    ) -> Result<Vec<RunningModuleProcess>> {
        let mut running = Self::find(store, id)?;
        if running.is_empty() {
            return Ok(running);
        }
        for item in &running {
            close_main_window(item.main_window);
        }
        let deadline = Instant::now() + timeout;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            running = Self::find(store, id)?;
            if running.is_empty() || Instant::now() >= deadline {
                return Ok(running);
            }
        }
    }

    pub async fn stop(
        store: &HubStore,
        id: &str,
        grace: Duration,
        terminate_if_needed: bool,
    ) -> Result<()> {
        let mut running = Self::wait_for_exit(store, id, grace).await?;
        if running.is_empty() {
            return Ok(());
        }
        if !terminate_if_needed {
            return Err(anyhow!(running_message(&running)));
        }
        for item in &running {
            // A visible application window still belongs to the user: leave it alone.
            if !item.background && item.main_window != 0 {
                continue;
            }
            kill_tree(item.id);
        }
        running = Self::wait_for_exit(store, id, Duration::from_secs(5)).await?;
        if !running.is_empty() {
            return Err(anyhow!(running_message(&running)));
        }
        Ok(())
    }

    /// Stops processes before uninstall/rollback, tolerating a missing manifest.
    pub async fn stop_if_present(
        store: &HubStore,
        app: &InstalledApp,
        id: &str,
        grace: Duration,
        terminate_if_needed: bool,
    ) -> Result<()> {
        if !store.app_present(app) {
            return Ok(());
        }
        Self::stop(store, id, grace, terminate_if_needed).await
    }
}

fn find_entries(root: &Path, manifest: &ModuleManifest) -> Vec<RunningModuleProcess> {
    let root_prefix = format!(
        "{}\\",
        root.to_string_lossy().trim_end_matches('\\').to_lowercase()
    );
    let mut names: HashSet<String> = HashSet::new();
    let mut background: HashSet<String> = HashSet::new();
    add_hook(&mut names, &mut background, manifest.launch.as_ref(), false);
    add_hook(&mut names, &mut background, manifest.worker.as_ref(), true);
    add_hook(&mut names, &mut background, manifest.stop.as_ref(), true);
    add_hook(&mut names, &mut background, manifest.cleanup.as_ref(), true);
    add_hook(
        &mut names,
        &mut background,
        manifest.integration.as_ref(),
        true,
    );
    if let Some(launch) = &manifest.launch {
        if let Some(stem) = Path::new(&launch.executable).file_stem() {
            background.remove(&stem.to_string_lossy().to_lowercase());
        }
    }
    let mut found = Vec::new();
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return found;
    };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let current = std::process::id();
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while ok {
        let pid = entry.th32ProcessID;
        if pid != 0 && pid != current {
            let raw_name = utf16z_to_string(&entry.szExeFile);
            let name = Path::new(&raw_name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| raw_name.clone());
            let path = process_path(pid);
            let matched = match &path {
                Some(path) => path.to_lowercase().starts_with(&root_prefix),
                None => names.contains(&name.to_lowercase()),
            };
            if matched {
                let background = background.contains(&name.to_lowercase());
                found.push(RunningModuleProcess {
                    id: pid,
                    name,
                    path,
                    background,
                    main_window: main_window(pid),
                });
            }
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    found
}

fn add_hook(
    names: &mut HashSet<String>,
    background: &mut HashSet<String>,
    hook: Option<&crate::models::WorkerDefinition>,
    helper: bool,
) {
    let Some(hook) = hook else { return };
    if hook.executable.trim().is_empty() {
        return;
    }
    let name = Path::new(&hook.executable)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    names.insert(name.clone());
    if helper {
        background.insert(name);
    }
}

fn utf16z_to_string(value: &[u16]) -> String {    let end = value.iter().position(|c| *c == 0).unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let result =
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size);
        let _ = CloseHandle(handle);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

struct EnumContext {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut EnumContext) };
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == context.pid
        && unsafe { IsWindowVisible(hwnd) }.as_bool()
        && unsafe { GetWindow(hwnd, GW_OWNER) }.is_err()
    {
        context.found = hwnd;
        return BOOL(0);
    }
    BOOL(1)
}

fn main_window(pid: u32) -> isize {
    let mut context = EnumContext {
        pid,
        found: HWND::default(),
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_window),
            LPARAM(&mut context as *mut EnumContext as isize),
        );
    }
    context.found.0 as isize
}

fn close_main_window(handle: isize) {
    if handle == 0 {
        return;
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(handle as *mut _)),
            WM_CLOSE,
            Default::default(),
            Default::default(),
        );
    }
}

fn kill_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn running_message(running: &[RunningModuleProcess]) -> String {
    let names = running
        .iter()
        .map(|item| format!("{} (PID {})", item.name, item.id))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Сначала закройте программу, в том числе её значок в трее. Работает: {names}.")
}

// Unused import guard for HANDLE on some windows crate versions.
#[allow(dead_code)]
fn _handle(_: HANDLE) {}
