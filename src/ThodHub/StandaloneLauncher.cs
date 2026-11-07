using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using ThodHub.Core;

namespace ThodHub;

public static class StandaloneLauncher
{
    public static string DefaultHome => Environment.GetEnvironmentVariable("THODHUB_HOME") ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),"ThodHUB");
    public static void Launch(HubStore store,string id,IEnumerable<string>? extraArguments=null)
    {
        var manifest=store.Manifest(id);
        if(manifest.Mode != "standalone" || manifest.Launch is null) throw new InvalidOperationException("Эта программа открывается внутри хаба.");
        var root=store.AppVersion(id,manifest.Version);
        var exe=Contract.Inside(root,manifest.Launch.Executable);
        if(!File.Exists(exe)) throw new FileNotFoundException("Файл программы не найден. Переустановите программу.");
        var info=new ProcessStartInfo(exe) { WorkingDirectory=Path.GetDirectoryName(exe)!,UseShellExecute=manifest.RequiresElevation };
        foreach(var arg in manifest.Launch.Arguments) info.ArgumentList.Add(arg.Replace("{dataDir}",store.DataDirectory(id),StringComparison.Ordinal));
        foreach(var arg in extraArguments ?? []) info.ArgumentList.Add(arg);
        if(manifest.RequiresElevation) info.Verb="runas";
        else { info.Environment["THODHUB_DATA_DIR"]=store.DataDirectory(id);info.Environment["THODHUB_LAUNCHER"]=Environment.ProcessPath; }
        Process.Start(info)?.Dispose();
    }
    public static Task CleanupAsync(HubStore store,string id) => RunHookAsync(store,id,store.Manifest(id).Cleanup);
    public static Task StopAsync(HubStore store,string id) => RunHookAsync(store,id,store.Manifest(id).Stop);
    public static Task IntegrateAsync(HubStore store,string id) => RunHookAsync(store,id,store.Manifest(id).Integration);

    /// <summary>Runs the optional stop hook, then makes sure older versions without one no longer block maintenance.</summary>
    public static Task StopForMaintenanceAsync(HubStore store,string id,bool terminateIfNeeded=true) =>
        ModuleProcessGuard.StopAsync(store,id,()=>StopAsync(store,id),
            store.Manifest(id).Stop is null?TimeSpan.FromSeconds(2):TimeSpan.FromSeconds(6),
            terminateIfNeeded);
    public static void AssertNotRunning(HubStore store,string id) => ModuleProcessGuard.AssertNotRunning(store,id);
    public static async Task RunHookAsync(HubStore store,string id,WorkerDefinition? hook)
    {
        var m=store.Manifest(id);
        if(hook is null)return;
        var root=store.AppVersion(id,m.Version);
        var info=new ProcessStartInfo(Contract.Inside(root,hook.Executable)) { WorkingDirectory=root,UseShellExecute=false,CreateNoWindow=true,RedirectStandardError=true,RedirectStandardOutput=true };
        foreach(var arg in hook.Arguments)info.ArgumentList.Add(arg.Replace("{dataDir}",store.DataDirectory(id),StringComparison.Ordinal));
        info.Environment["THODHUB_DATA_DIR"]=store.DataDirectory(id);info.Environment["THODHUB_LAUNCHER"]=Environment.ProcessPath;
        using var process=Process.Start(info) ?? throw new IOException("Не удалось запустить настройку интеграции Windows.");
        var stderr=process.StandardError.ReadToEndAsync();var stdout=process.StandardOutput.ReadToEndAsync();
        try { await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(45)); }
        catch(TimeoutException) { process.Kill(true);throw new IOException("Настройка интеграции Windows не завершилась за 45 секунд."); }
        await Task.WhenAll(stderr,stdout);
        if(process.ExitCode!=0)throw new IOException("Ошибка интеграции Windows (код "+process.ExitCode+"). "+await stderr);
    }
    private static string ShortcutPath(string id,string name)
    {
        Contract.Id(id);
        var file=string.Concat(name.Select(c=>Path.GetInvalidFileNameChars().Contains(c)?'_':c)).Trim().TrimEnd('.');
        if(file.Length==0||file.Length>100)file=id;
        return Contract.Inside(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),file+".lnk");
    }
    private static bool OwnShortcut(dynamic link,string id) =>
        string.Equals((string)link.Arguments,"--launch "+id,StringComparison.Ordinal) &&
        string.Equals(Path.GetFileName((string)link.TargetPath),"ThodHUB.exe",StringComparison.OrdinalIgnoreCase) &&
        ((string)link.Description).EndsWith(" · ThodHUB",StringComparison.Ordinal);
    public static void CreateShortcut(HubStore store,string id,string name)
    {
        var manifest=store.Manifest(id);
        var type=Type.GetTypeFromProgID("WScript.Shell") ?? throw new InvalidOperationException("Windows Script Host недоступен: ярлык не создан.");
        object shell=Activator.CreateInstance(type)!;object? shortcut=null;
        try
        {
            dynamic automation=shell;
            var path=ShortcutPath(id,name);
            shortcut=automation.CreateShortcut(path);
            dynamic link=shortcut;
            if(File.Exists(path) && !OwnShortcut(link,id)) throw new IOException("На рабочем столе уже существует чужой ярлык «"+name+"». Переименуйте его перед созданием ярлыка из хаба.");
            link.TargetPath=Environment.ProcessPath!;
            link.Arguments="--launch "+id;
            link.WorkingDirectory=AppContext.BaseDirectory;
            link.Description=name+" · ThodHUB";
            var entry=manifest.Launch??manifest.Worker;
            link.IconLocation=(entry is null?Environment.ProcessPath!:Contract.Inside(store.AppVersion(id,manifest.Version),entry.Executable))+",0";
            link.Save();
            RemoveShortcut(id,path);
        }
        finally { if(shortcut is not null) Marshal.FinalReleaseComObject(shortcut);Marshal.FinalReleaseComObject(shell); }
    }
    public static void RemoveShortcut(string id,string? except=null)
    {
        Contract.Id(id);
        var type=Type.GetTypeFromProgID("WScript.Shell") ?? throw new InvalidOperationException("Не удалось проверить ярлык.");
        object shell=Activator.CreateInstance(type)!;object? shortcut=null;
        try
        {
            dynamic automation=shell;
            foreach(var path in Directory.EnumerateFiles(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),"*.lnk"))
            {
                if(string.Equals(path,except,StringComparison.OrdinalIgnoreCase))continue;
                try { shortcut=automation.CreateShortcut(path);dynamic link=shortcut;if(OwnShortcut(link,id))File.Delete(path); }
                finally {if(shortcut is not null){Marshal.FinalReleaseComObject(shortcut);shortcut=null;}}
            }
        }
        finally { if(shortcut is not null) Marshal.FinalReleaseComObject(shortcut);Marshal.FinalReleaseComObject(shell); }
    }
}
