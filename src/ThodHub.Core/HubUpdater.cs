using System.Diagnostics;
using System.IO.Compression;
using System.Text;

namespace ThodHub.Core;

public static class HubUpdater
{
    private const long MaxExpandedBytes = 512L * 1024 * 1024;

    private const string Script = """
param(
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
""";

    public static string Extract(string archivePath, string destination)
    {
        var root = Path.GetFullPath(destination);
        var parent = Path.GetDirectoryName(root) ?? throw new InvalidDataException("Некорректная папка распаковки.");
        var parentPrefix = Path.GetFullPath(parent).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (!root.StartsWith(parentPrefix, StringComparison.OrdinalIgnoreCase) || Path.GetFileName(root).Length < 8)
            throw new InvalidDataException("Некорректная папка распаковки.");
        if (Directory.Exists(root) || File.Exists(root)) throw new InvalidDataException("Папка распаковки уже существует.");
        Directory.CreateDirectory(root);
        try
        {
            using var archive = ZipFile.OpenRead(archivePath);
            if (archive.Entries.Count is 0 or > 20000) throw new InvalidDataException("Некорректный архив обновления.");
            long expanded = 0;
            foreach (var entry in archive.Entries)
            {
                var name = entry.FullName.Replace('\\', '/');
                var directory = name.EndsWith('/');
                var relative = name.TrimEnd('/');
                if (relative.Length == 0) continue;
                var target = Contract.Inside(root, relative);
                if (directory) { Directory.CreateDirectory(target); continue; }
                expanded = checked(expanded + entry.Length);
                if (expanded > MaxExpandedBytes) throw new InvalidDataException("Архив обновления слишком большой.");
                if ((entry.ExternalAttributes & (int)FileAttributes.ReparsePoint) != 0) throw new InvalidDataException("Архив содержит недопустимую ссылку.");
                Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                using var input = entry.Open();
                using var output = new FileStream(target, FileMode.CreateNew, FileAccess.Write, FileShare.None);
                input.CopyTo(output);
            }
            if (!File.Exists(Path.Combine(root, "ThodHUB.exe"))) throw new InvalidDataException("В архиве нет ThodHUB.exe.");
            return root;
        }
        catch
        {
            TryDelete(root);
            throw;
        }
    }

    public static string WriteScript(string directory, string target, string stage, string backup, string log)
    {
        Directory.CreateDirectory(directory);
        var path = Path.Combine(directory, "thodhub-update-" + Guid.NewGuid().ToString("N") + ".ps1");
        File.WriteAllText(path, Script, new UTF8Encoding(false));
        return path;
    }

    public static void Launch(string scriptPath, int hubPid, string target, string stage, string backup, string log)
    {
        var powershell = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "WindowsPowerShell", "v1.0", "powershell.exe");
        if (!File.Exists(powershell)) powershell = "powershell.exe";
        var info = new ProcessStartInfo(powershell)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WindowStyle = ProcessWindowStyle.Hidden
        };
        info.ArgumentList.Add("-NoProfile");
        info.ArgumentList.Add("-NonInteractive");
        info.ArgumentList.Add("-ExecutionPolicy");
        info.ArgumentList.Add("Bypass");
        info.ArgumentList.Add("-WindowStyle");
        info.ArgumentList.Add("Hidden");
        info.ArgumentList.Add("-File");
        info.ArgumentList.Add(scriptPath);
        info.ArgumentList.Add("-HubPid");
        info.ArgumentList.Add(hubPid.ToString(System.Globalization.CultureInfo.InvariantCulture));
        info.ArgumentList.Add("-Target");
        info.ArgumentList.Add(target);
        info.ArgumentList.Add("-Stage");
        info.ArgumentList.Add(stage);
        info.ArgumentList.Add("-Backup");
        info.ArgumentList.Add(backup);
        info.ArgumentList.Add("-Log");
        info.ArgumentList.Add(log);
        if (Process.Start(info) is null) throw new InvalidOperationException("Не удалось запустить установщик обновления.");
    }

    private static void TryDelete(string path)
    {
        try { if (Directory.Exists(path)) Directory.Delete(path, true); }
        catch { }
    }
}
