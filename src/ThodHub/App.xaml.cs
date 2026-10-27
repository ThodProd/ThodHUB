using System.Windows;
using System.IO;
using System.IO.Pipes;
using System.Security.Cryptography;
using System.Text;
using ThodHub.Core;

namespace ThodHub;
public partial class App : Application
{
    private Mutex? _singleInstance;
    private readonly CancellationTokenSource _lifetime = new();
    private static string InstanceName => "ThodHUB." + Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(Environment.UserName + Path.GetFullPath(StandaloneLauncher.DefaultHome).ToUpperInvariant())))[..24];
    protected override async void OnStartup(StartupEventArgs e)
    {
        if(e.Args.Length>=2 && e.Args[0]=="--launch" && new HubStore(StandaloneLauncher.DefaultHome).Settings.Installed.Any(a=>a.Id==e.Args[1]) && new HubStore(StandaloneLauncher.DefaultHome).Manifest(e.Args[1]).Mode=="standalone")
        {
            try { StandaloneLauncher.Launch(new HubStore(StandaloneLauncher.DefaultHome),e.Args[1],e.Args.Skip(2)); }
            catch(Exception ex) { MessageBox.Show(ex.Message,"ThodHUB",MessageBoxButton.OK,MessageBoxImage.Error); }
            Shutdown();return;
        }
        if(e.Args.Length==4 && e.Args[0]=="--invoke")
        {
            ShutdownMode=ShutdownMode.OnExplicitShutdown;
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
        _singleInstance = new Mutex(true, "Local\\"+InstanceName, out bool created);
        var requested = e.Args.Length>=2 && e.Args[0] is "--launch" or "--open" ? e.Args[1] : "";
        if(!created)
        {
            try
            {
                using var pipe=new NamedPipeClientStream(".",InstanceName,PipeDirection.Out,PipeOptions.Asynchronous);
                await pipe.ConnectAsync(10000);
                using var writer=new StreamWriter(pipe,new UTF8Encoding(false)){AutoFlush=true};
                await writer.WriteLineAsync(requested);
            }
            catch(Exception ex){MessageBox.Show("Не удалось передать команду открытому хабу: "+ex.Message,"ThodHUB");}
            Shutdown();return;
        }
        base.OnStartup(e);
        var window=new MainWindow();MainWindow=window;ShutdownMode=ShutdownMode.OnMainWindowClose;window.Show();
        _=ListenAsync(window);
        if(requested.Length>0)await window.OpenRequestedAsync(requested);
    }
    private async Task ListenAsync(MainWindow window)
    {
        while(!_lifetime.IsCancellationRequested)
        {
            try
            {
                using var pipe=new NamedPipeServerStream(InstanceName,PipeDirection.In,1,PipeTransmissionMode.Byte,PipeOptions.Asynchronous|PipeOptions.CurrentUserOnly);
                await pipe.WaitForConnectionAsync(_lifetime.Token);
                using var reader=new StreamReader(pipe);
                using var timeout=CancellationTokenSource.CreateLinkedTokenSource(_lifetime.Token);timeout.CancelAfter(5000);
                var buffer=new char[65];var length=0;
                while(length<buffer.Length){var n=await reader.ReadAsync(buffer.AsMemory(length,1),timeout.Token);if(n==0||buffer[length]=='\n')break;length++;}
                var id=new string(buffer,0,length).TrimEnd('\r');
                if(id.Length>0)Contract.Id(id);
                await window.OpenRequestedAsync(id);
            }
            catch(OperationCanceledException) when(_lifetime.IsCancellationRequested){break;}
            catch(Exception ex){System.Diagnostics.Debug.WriteLine(ex.Message);}
        }
    }
    protected override void OnExit(ExitEventArgs e) { _lifetime.Cancel();_singleInstance?.Dispose(); base.OnExit(e); }
}
