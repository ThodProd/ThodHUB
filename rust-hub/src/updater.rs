use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::contract;
use crate::util;

const MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SCRIPT: &str = r#"param(
    [Parameter(Mandatory=$true)][int]$HubPid,
    [Parameter(Mandatory=$true)][string]$Target,
    [Parameter(Mandatory=$true)][string]$Stage,
    [Parameter(Mandatory=$true)][string]$Backup,
    [Parameter(Mandatory=$true)][string]$Log
)
$ErrorActionPreference = 'Stop'
function Write-Log([string]$Message) {
    try { Add-Content -LiteralPath $Log -Value ((Get-Date).ToString('o') + ' ' + $Message) -Encoding UTF8 } catch {}
}
$started = $false
try {
    Write-Log "waiting for hub process $HubPid"
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Process -Id $HubPid -ErrorAction SilentlyContinue) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Seconds 2
    $exe = Join-Path $Target 'ThodHUB.exe'
    if (-not (Test-Path -LiteralPath $Target)) { throw "target folder is missing: $Target" }
    if (-not (Test-Path -LiteralPath (Join-Path $Stage 'ThodHUB.exe'))) { throw "staged ThodHUB.exe is missing" }
    Write-Log "replacing $Target"
    if (Test-Path -LiteralPath $Backup) { Remove-Item -LiteralPath $Backup -Recurse -Force }
    $moved = $false
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        try { Move-Item -LiteralPath $Target -Destination $Backup -Force; $moved = $true; break }
        catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $moved) { throw "could not move the target folder" }
    try {
        New-Item -ItemType Directory -Path $Target -Force | Out-Null
        Copy-Item -Path (Join-Path $Stage '*') -Destination $Target -Recurse -Force
        Write-Log "starting updated hub"
        $new = Start-Process -FilePath $exe -WorkingDirectory $Target -PassThru
        $started = $true
        Start-Sleep -Seconds 8
        if ($new.HasExited) { throw "updated hub exited with code $($new.ExitCode)" }
        Write-Log "update completed"
        if (Test-Path -LiteralPath $Backup) { Remove-Item -LiteralPath $Backup -Recurse -Force -ErrorAction SilentlyContinue }
    } catch {
        Write-Log "rolling back: $($_.Exception.Message)"
        if (Test-Path -LiteralPath $Target) { Remove-Item -LiteralPath $Target -Recurse -Force -ErrorAction SilentlyContinue }
        if (Test-Path -LiteralPath $Backup) { Move-Item -LiteralPath $Backup -Destination $Target -Force }
        if (Test-Path -LiteralPath (Join-Path $Target 'ThodHUB.exe')) {
            Start-Process -FilePath (Join-Path $Target 'ThodHUB.exe') -WorkingDirectory $Target | Out-Null
            $started = $true
        }
        throw
    }
} catch {
    Write-Log "update failed: $($_.Exception.Message)"
    if (-not $started -and (Test-Path -LiteralPath (Join-Path $Target 'ThodHUB.exe'))) {
        try { Start-Process -FilePath (Join-Path $Target 'ThodHUB.exe') -WorkingDirectory $Target | Out-Null } catch {}
    }
} finally {
    if (Test-Path -LiteralPath $Stage) { Remove-Item -LiteralPath $Stage -Recurse -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}
"#;

pub struct HubUpdater;

impl HubUpdater {
    pub fn extract(archive_path: &Path, destination: &Path) -> Result<PathBuf> {
        let root = util::absolute(destination)?;
        let parent = root
            .parent()
            .ok_or_else(|| anyhow!("Некорректная папка распаковки."))?;
        let parent_prefix = format!(
            "{}\\",
            util::absolute(parent)?
                .to_string_lossy()
                .trim_end_matches('\\')
        );
        if !root
            .to_string_lossy()
            .to_lowercase()
            .starts_with(&parent_prefix.to_lowercase())
            || root
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.chars().count() < 8)
                .unwrap_or(true)
        {
            return Err(anyhow!("Некорректная папка распаковки."));
        }
        if root.exists() {
            return Err(anyhow!("Папка распаковки уже существует."));
        }
        std::fs::create_dir_all(&root)?;
        let extract = (|| -> Result<()> {
            let file = std::fs::File::open(archive_path)?;
            let mut zip = zip::ZipArchive::new(file)
                .map_err(|_| anyhow!("Некорректный архив обновления."))?;
            if zip.len() == 0 || zip.len() > 20_000 {
                return Err(anyhow!("Некорректный архив обновления."));
            }
            let mut expanded: u64 = 0;
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index)?;
                let name = entry.name().unwrap_or_default().to_string();
                let normalized = name.replace('\\', "/");
                let directory = normalized.ends_with('/');
                let relative = normalized.trim_end_matches('/');
                if relative.is_empty() {
                    continue;
                }
                let target = contract::inside(&root, relative)
                    .map_err(|_| anyhow!("Некорректный архив обновления."))?;
                if directory {
                    std::fs::create_dir_all(&target)?;
                    continue;
                }
                expanded = expanded
                    .checked_add(entry.size())
                    .ok_or_else(|| anyhow!("Архив обновления слишком большой."))?;
                if expanded > MAX_EXPANDED_BYTES {
                    return Err(anyhow!("Архив обновления слишком большой."));
                }
                if entry.external_attributes() & 0x400 != 0 {
                    return Err(anyhow!("Архив содержит недопустимую ссылку."));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                let mut buffer = [0u8; 65536];
                loop {
                    let read = entry.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    output.write_all(&buffer[..read])?;
                }
            }
            if !root.join("ThodHUB.exe").is_file() {
                return Err(anyhow!("В архиве нет ThodHUB.exe."));
            }
            Ok(())
        })();
        if let Err(error) = extract {
            let _ = std::fs::remove_dir_all(&root);
            return Err(error);
        }
        Ok(root)
    }

    pub fn write_script(directory: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("thodhub-update-{}.ps1", util::random_token()));
        util::write_atomic(&path, SCRIPT)?;
        Ok(path)
    }

    pub fn launch(
        script_path: &Path,
        hub_pid: u32,
        target: &Path,
        stage: &Path,
        backup: &Path,
        log: &Path,
    ) -> Result<()> {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let powershell = Path::new(&system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let powershell = if powershell.is_file() {
            powershell
        } else {
            PathBuf::from("powershell.exe")
        };
        let mut command = std::process::Command::new(&powershell);
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
                &script_path.to_string_lossy(),
                "-HubPid",
                &hub_pid.to_string(),
                "-Target",
                &target.to_string_lossy(),
                "-Stage",
                &stage.to_string_lossy(),
                "-Backup",
                &backup.to_string_lossy(),
                "-Log",
                &log.to_string_lossy(),
            ])
            .creation_flags(CREATE_NO_WINDOW);
        command
            .spawn()
            .map_err(|_| anyhow!("Не удалось запустить установщик обновления."))?;
        Ok(())
    }
}
