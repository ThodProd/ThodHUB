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
    public static async Task CleanupAsync(HubStore store,string id)
    {
        var m=store.Manifest(id);
        if(m.Cleanup is null)return;
        var root=store.AppVersion(id,m.Version);
        var info=new ProcessStartInfo(Contract.Inside(root,m.Cleanup.Executable)) { WorkingDirectory=root,UseShellExecute=false,CreateNoWindow=true,RedirectStandardError=true,RedirectStandardOutput=true };
        foreach(var arg in m.Cleanup.Arguments)info.ArgumentList.Add(arg.Replace("{dataDir}",store.DataDirectory(id),StringComparison.Ordinal));
        info.Environment["THODHUB_DATA_DIR"]=store.DataDirectory(id);info.Environment["THODHUB_LAUNCHER"]=Environment.ProcessPath;
        using var process=Process.Start(info) ?? throw new IOException("Не удалось убрать интеграцию программы.");
        var stderr=process.StandardError.ReadToEndAsync();var stdout=process.StandardOutput.ReadToEndAsync();
        try { await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(15)); }
        catch(TimeoutException) { process.Kill(true);throw new IOException("Очистка интеграции не завершилась. Программа не удалена."); }
        await Task.WhenAll(stderr,stdout);
        if(process.ExitCode!=0)throw new IOException("Не удалось убрать контекстное меню программы. "+await stderr);
    }
    public static void AssertNotRunning(HubStore store,string id)
    {
        var root=ManagedFiles.Check(store.AppsRoot,Path.Combine(store.AppsRoot,id))+Path.DirectorySeparatorChar;
        var manifest=store.Manifest(id);
        var expectedName=manifest.Launch is null?null:Path.GetFileNameWithoutExtension(manifest.Launch.Executable);
        foreach(var process in Process.GetProcesses())
        {
            using(process)
            {
                string? path=null;
                try { path=process.MainModule?.FileName; }
                catch(Exception e) when(e is System.ComponentModel.Win32Exception or InvalidOperationException or NotSupportedException)
                {
                    try { if(expectedName is not null && process.ProcessName.Equals(expectedName,StringComparison.OrdinalIgnoreCase)) throw new IOException("Сначала закройте программу, в том числе её значок в трее."); }
                    catch(InvalidOperationException) { }
                }
                if(path is not null && Path.GetFullPath(path).StartsWith(root,StringComparison.OrdinalIgnoreCase)) throw new IOException("Сначала закройте программу, в том числе её значок в трее.");
            }
        }
    }
    private static string ShortcutPath(string id)
    {
        Contract.Id(id);
        return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),$"ThodHUB - {id}.lnk");
    }
    public static void CreateShortcut(HubStore store,string id,string name)
    {
        var manifest=store.Manifest(id);
        if(manifest.Mode!="standalone") return;
        var type=Type.GetTypeFromProgID("WScript.Shell") ?? throw new InvalidOperationException("Windows Script Host недоступен: ярлык не создан.");
        object shell=Activator.CreateInstance(type)!;object? shortcut=null;
        try
        {
            dynamic automation=shell;
            var path=ShortcutPath(id);
            shortcut=automation.CreateShortcut(path);
            dynamic link=shortcut;
            if(File.Exists(path) && !string.Equals((string)link.Arguments,"--launch "+id,StringComparison.Ordinal)) throw new IOException("На рабочем столе уже существует чужой ярлык с таким именем.");
            link.TargetPath=Environment.ProcessPath!;
            link.Arguments="--launch "+id;
            link.WorkingDirectory=AppContext.BaseDirectory;
            link.Description=name+" · ThodHUB";
            link.IconLocation=Contract.Inside(store.AppVersion(id,manifest.Version),manifest.Launch!.Executable)+",0";
            link.Save();
        }
        finally { if(shortcut is not null) Marshal.FinalReleaseComObject(shortcut);Marshal.FinalReleaseComObject(shell); }
    }
    public static void RemoveShortcut(string id)
    {
        var path=ShortcutPath(id);
        if(!File.Exists(path)) return;
        var type=Type.GetTypeFromProgID("WScript.Shell") ?? throw new InvalidOperationException("Не удалось проверить ярлык.");
        object shell=Activator.CreateInstance(type)!;object? shortcut=null;
        try
        {
            dynamic automation=shell;shortcut=automation.CreateShortcut(path);dynamic link=shortcut;
            if(string.Equals((string)link.Arguments,"--launch "+id,StringComparison.Ordinal)) File.Delete(path);
        }
        finally { if(shortcut is not null) Marshal.FinalReleaseComObject(shortcut);Marshal.FinalReleaseComObject(shell); }
    }
}
