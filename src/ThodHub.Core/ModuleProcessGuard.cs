using System.Diagnostics;

namespace ThodHub.Core;

public sealed record RunningModuleProcess(int Id,string Name,string? Path,bool Background=false);

/// <summary>Finds and stops processes that belong to one installed module.</summary>
public static class ModuleProcessGuard
{
    public static IReadOnlyList<RunningModuleProcess> Find(HubStore store,string id) => FindEntries(store,id);
    public static void AssertNotRunning(HubStore store,string id)
    {
        var running=FindEntries(store,id);
        if(running.Count>0)throw new IOException(RunningMessage(running));
    }

    /// <summary>A module stop hook is preferred. Older installed versions may not declare one, so the hub then closes or terminates only processes whose executable is inside that module's install folder.</summary>
    public static async Task StopAsync(HubStore store,string id,Func<Task>? stopHook=null,TimeSpan? grace=null,bool terminateIfNeeded=true)
    {
        if(stopHook is not null)await stopHook();
        var running=await WaitForExitAsync(store,id,grace??TimeSpan.FromSeconds(6));
        if(running.Count==0)return;
        if(!terminateIfNeeded)throw new IOException(RunningMessage(running));
        foreach(var item in running)
        {
            try
            {
                using var process=Process.GetProcessById(item.Id);
                // A visible application window still belongs to the user: close it politely and let the error explain what remains.
                if(!item.Background&&process.MainWindowHandle!=IntPtr.Zero)continue;
                process.Kill(true);
            }
            catch(Exception) { }
        }
        running=await WaitForExitAsync(store,id,TimeSpan.FromSeconds(5));
        if(running.Count>0)throw new IOException(RunningMessage(running));
    }

    private static async Task<List<RunningModuleProcess>> WaitForExitAsync(HubStore store,string id,TimeSpan timeout)
    {
        var running=FindEntries(store,id);
        if(running.Count==0)return running;
        foreach(var item in running)
        {
            try { using var process=Process.GetProcessById(item.Id);if(process.MainWindowHandle!=IntPtr.Zero)process.CloseMainWindow(); }
            catch(Exception) { }
        }
        var deadline=DateTime.UtcNow+timeout;
        do
        {
            await Task.Delay(250);
            running=FindEntries(store,id);
        }
        while(running.Count>0&&DateTime.UtcNow<deadline);
        return running;
    }

    private static List<RunningModuleProcess> FindEntries(HubStore store,string id)
    {
        var root=ManagedFiles.Check(store.AppsRoot,Path.Combine(store.AppsRoot,id)).TrimEnd(Path.DirectorySeparatorChar)+Path.DirectorySeparatorChar;
        var manifest=store.Manifest(id);
        var names=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var background=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        void Add(WorkerDefinition? hook,bool helper)
        {
            if(hook is null||string.IsNullOrWhiteSpace(hook.Executable))return;
            var name=Path.GetFileNameWithoutExtension(hook.Executable);
            names.Add(name);
            if(helper)background.Add(name);
        }
        Add(manifest.Launch,false);Add(manifest.Worker,true);Add(manifest.Stop,true);Add(manifest.Cleanup,true);Add(manifest.Integration,true);
        if(manifest.Launch is not null)background.Remove(Path.GetFileNameWithoutExtension(manifest.Launch.Executable));
        var found=new List<RunningModuleProcess>();
        foreach(var process in Process.GetProcesses())
        {
            try
            {
                if(process.Id==Environment.ProcessId)continue;
                var name=process.ProcessName;
                string? path=null;
                bool matched;
                try { path=process.MainModule?.FileName;matched=string.IsNullOrEmpty(path)?names.Contains(name):IsInside(root,path); }
                catch(Exception e) when(e is System.ComponentModel.Win32Exception or InvalidOperationException or NotSupportedException) { matched=names.Contains(name); }
                if(matched)found.Add(new RunningModuleProcess(process.Id,name,path,background.Contains(name)));
            }
            catch(Exception) { }
            finally { process.Dispose(); }
        }
        return found;
    }

    private static bool IsInside(string root,string path)
    {
        try { return Path.GetFullPath(path).StartsWith(root,StringComparison.OrdinalIgnoreCase); }
        catch(Exception) { return false; }
    }

    private static string RunningMessage(IReadOnlyList<RunningModuleProcess> running)
    {
        var names=string.Join(", ",running.Select(x=>x.Name+" (PID "+x.Id.ToString(System.Globalization.CultureInfo.InvariantCulture)+")"));
        return "Сначала закройте программу, в том числе её значок в трее. Работает: "+names+".";
    }
}
