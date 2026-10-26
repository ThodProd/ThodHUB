using System.Windows;
using ThodHub.Core;

namespace ThodHub;
public partial class App : Application
{
    private Mutex? _singleInstance;
    protected override async void OnStartup(StartupEventArgs e)
    {
        if(e.Args.Length>=2 && e.Args[0]=="--launch")
        {
            try { StandaloneLauncher.Launch(new HubStore(StandaloneLauncher.DefaultHome),e.Args[1],e.Args.Skip(2)); }
            catch(Exception ex) { MessageBox.Show(ex.Message,"ThodHUB",MessageBoxButton.OK,MessageBoxImage.Error); }
            Shutdown();return;
        }
        if(e.Args.Length==4 && e.Args[0]=="--invoke")
        {
            StartupUri=null;ShutdownMode=ShutdownMode.OnExplicitShutdown;
            try
            {
                var store=new HubStore(StandaloneLauncher.DefaultHome);var m=store.Manifest(e.Args[1]);
                if(m.Worker is null||!m.Permissions.Contains("clipboard.write")) throw new InvalidOperationException("Программа не поддерживает копирование из меню.");
                var root=store.AppVersion(m.Id,m.Version);
                await using var process=new ModuleProcess(Contract.Inside(root,m.Worker.Executable),m.Worker.Arguments,root,store.DataDirectory(m.Id));
                var result=await process.CallAsync(e.Args[2],System.Text.Json.JsonSerializer.SerializeToElement(new {path=e.Args[3]}));
                for(var attempt=0;;attempt++)
                {
                    try { Clipboard.SetDataObject(result.GetProperty("clipboardText").GetString()??"",true);break; }
                    catch(System.Runtime.InteropServices.COMException) when(attempt<9) { await Task.Delay(100); }
                }
                if(result.TryGetProperty("skipped",out var skipped)&&skipped.GetInt32()>0)MessageBox.Show($"Пути скопированы. Пропущено недоступных папок или ссылок: {skipped}.","CopyAll");
            }
            catch(Exception ex) { MessageBox.Show(ex.Message,"ThodHUB",MessageBoxButton.OK,MessageBoxImage.Error); }
            Shutdown();return;
        }
        _singleInstance = new Mutex(true, "Local\\ThodHUB.Desktop", out bool created);
        if(!created) { MessageBox.Show("ThodHUB уже запущен.", "ThodHUB"); Shutdown(); return; }
        base.OnStartup(e);
    }
    protected override void OnExit(ExitEventArgs e) { _singleInstance?.Dispose(); base.OnExit(e); }
}
