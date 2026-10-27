using System.Diagnostics;
using System.IO;
using System.Text.Json;
using System.Windows;
using Microsoft.Web.WebView2.Core;
using Microsoft.Win32;
using ThodHub.Core;

namespace ThodHub;

public partial class MainWindow : Window
{
    private static readonly string Host = "hub-v"+typeof(MainWindow).Assembly.GetName().Version!.ToString(3).Replace('.','-')+".thodhub.local";
    private static readonly string Origin = "https://"+Host;
    private readonly HubStore _store;
    private readonly GitHubCatalog _github;
    private readonly PackageInstaller _installer;
    private readonly Dictionary<string, ModuleProcess> _workers = [];
    private readonly Dictionary<string, SemaphoreSlim> _moduleGates = [];
    private readonly Dictionary<string, CancellationTokenSource> _downloads = [];
    private readonly HashSet<string> _removing = [];
    private Catalog _catalog;
    private string? _activeModule;
    private bool _closing;
    private readonly SemaphoreSlim _refreshGate = new(1, 1);
    private readonly DocumentPrinter _printer = new();
    private WindowState _previousWindowState;
    private bool _fullscreen;
    private readonly TaskCompletionSource _uiReady = new();
    public async Task OpenRequestedAsync(string id)
    {
        try
        {
            if(WindowState==WindowState.Minimized)WindowState=WindowState.Normal;
            Show();Activate();
            await _uiReady.Task.WaitAsync(TimeSpan.FromSeconds(30));
            if(id.StartsWith("remove:",StringComparison.Ordinal))
            {
                var appId=id[7..];Contract.Id(appId);
                await Browser.CoreWebView2.ExecuteScriptAsync("confirmRemoval("+JsonSerializer.Serialize(appId)+")");
            }
            else if(id.Length>0)await Browser.CoreWebView2.ExecuteScriptAsync("openApp("+JsonSerializer.Serialize(id)+")");
        }
        catch(Exception ex){MessageBox.Show(this,ex.Message,"ThodHUB");}
    }

    public MainWindow()
    {
        InitializeComponent();
        var root = StandaloneLauncher.DefaultHome;
        _store = new HubStore(root);
        _github = new GitHubCatalog(_store);
        _installer = new PackageInstaller(_store);
        _catalog = _github.LoadCached(Path.Combine(AppContext.BaseDirectory, "catalog.json"));
        Loaded += async (_, _) => await InitializeAsync();
        Closing += async (_, e) =>
        {
            if(_closing) return;
            if(!ConfirmDiscard()){e.Cancel=true;return;}
            e.Cancel = true; _closing = true;
            foreach(var c in _downloads.Values) await c.CancelAsync();
            foreach(var worker in _workers.Values) await worker.DisposeAsync();
            _github.Dispose(); Browser.Dispose(); Close();
        };
    }
    private async Task InitializeAsync()
    {
        try
        {
            var environment = await CoreWebView2Environment.CreateAsync(null, Path.Combine(_store.Root, "WebView"));
            await Browser.EnsureCoreWebView2Async(environment);
            var core = Browser.CoreWebView2;
            core.Settings.AreDevToolsEnabled = Environment.GetEnvironmentVariable("THODHUB_DEV") == "1";
            core.Settings.AreDefaultContextMenusEnabled = false;
            core.Settings.IsStatusBarEnabled = false;
            core.Settings.IsPasswordAutosaveEnabled = false;
            core.SetVirtualHostNameToFolderMapping(Host, Path.Combine(AppContext.BaseDirectory, "wwwroot"), CoreWebView2HostResourceAccessKind.DenyCors);
            core.NavigationStarting += (_, e) => { if(!e.Uri.StartsWith(Origin + "/", StringComparison.Ordinal)) e.Cancel = true; };
            core.NewWindowRequested += (_, e) => e.Handled = true;
            core.PermissionRequested += (_, e) => e.State = CoreWebView2PermissionState.Deny;
            core.DownloadStarting += (_, e) => e.Cancel = true;
            Browser.PreviewKeyDown += (_, e) =>
            {
                if(e.Key==System.Windows.Input.Key.Escape&&_fullscreen)
                {
                    e.Handled=true;SetFullscreen(false);Send(new { @event="fullscreen",enabled=false });
                }
            };
            core.WebMessageReceived += OnMessage;
            core.Navigate(Origin + "/index.html");
        }
        catch(Exception ex)
        {
            Browser.Visibility=Visibility.Collapsed;StartupHelp.Visibility=Visibility.Visible;StartupError.Text=ex.Message;
        }
    }
    private void WebViewDownload_Click(object sender,RoutedEventArgs e) => Process.Start(new ProcessStartInfo("https://developer.microsoft.com/microsoft-edge/webview2/#download-section"){UseShellExecute=true});
    private async void OnMessage(object? sender, CoreWebView2WebMessageReceivedEventArgs e)
    {
        if(!e.Source.StartsWith(Origin + "/", StringComparison.Ordinal)) return;
        string? id = null;
        try
        {
            if(e.WebMessageAsJson.Length > 12 * 1024 * 1024) throw new InvalidDataException("Слишком большой запрос.");
            var message = HubJson.Read<JsonElement>(e.WebMessageAsJson);
            id = message.GetProperty("id").GetString();
            var method = message.GetProperty("method").GetString()!;
            var p = message.TryGetProperty("params", out var param) ? param : default;
            var result = await HandleAsync(method, p);
            Send(new { id, result });
        }
        catch(Exception ex) { Send(new { id, error = ex is OperationCanceledException ? "Действие отменено." : ex.Message }); }
    }
    private void Send(object data)
    {
        if(_closing || Browser.CoreWebView2 is null) return;
        Browser.CoreWebView2.PostWebMessageAsJson(JsonSerializer.Serialize(data, HubJson.Options));
    }
    private bool _moduleDirty;
    private bool ConfirmDiscard() => !_moduleDirty || MessageBox.Show(this,"В открытой программе есть несохранённые изменения. Сначала сохраните проект, чтобы продолжить работу позже.\n\nВсё равно закрыть программу?","Несохранённые изменения",MessageBoxButton.YesNo,MessageBoxImage.Warning,MessageBoxResult.No)==MessageBoxResult.Yes;
    private object Snapshot() => new { catalog = _catalog, settings = _store.Settings, dataRoot = _store.DataRoot, hubVersion = typeof(MainWindow).Assembly.GetName().Version!.ToString(3), apiVersion = 1, native = true };
    private async Task<object?> HandleAsync(string method, JsonElement p)
    {
        if(method.StartsWith("backups.",StringComparison.Ordinal))return await HandleBackupAsync(method,p);
        switch(method)
        {
            case "hub.ready":
                foreach(var installed in _store.Settings.Installed)
                {
                    try {WindowsInstallation.Register(_store,installed.Id,_catalog.Apps.FirstOrDefault(a=>a.Id==installed.Id)?.Name??installed.Id);}
                    catch(Exception ex){Send(new{@event="notice",text="Не удалось зарегистрировать приложение Windows: "+ex.Message});}
                }
                _uiReady.TrySetResult(); return true;
            case "hub.state": return Snapshot();
            case "catalog.refresh":
                await _refreshGate.WaitAsync();
                try { _catalog = await _github.RefreshAsync(_catalog); return Snapshot(); }
                finally { _refreshGate.Release(); }
            case "settings.save":
                var theme = p.GetProperty("theme").GetString()!;
                if(theme is not ("light" or "dark" or "system")) throw new InvalidDataException("Неизвестная тема.");
                await _store.ChangeAsync(s => { s.Theme = theme; s.CheckOnStartup = p.GetProperty("checkOnStartup").GetBoolean(); });
                return Snapshot();
            case "sidebar.order":
                var order = p.GetProperty("order").EnumerateArray().Select(x => x.GetString()!).ToList();
                if(order.Count != _store.Settings.Installed.Count || order.Distinct().Count() != order.Count || order.Any(x => !_store.Settings.Installed.Any(a => a.Id == x))) throw new InvalidDataException("Порядок должен содержать все установленные программы.");
                await _store.ChangeAsync(s => s.SidebarOrder = order); return true;
            case "hub.openData": Process.Start(new ProcessStartInfo(_store.DataRoot) { UseShellExecute = true }); return true;
            case "hub.releases": Process.Start(new ProcessStartInfo("https://github.com/ThodProd/ThodHUB/releases/latest") { UseShellExecute = true }); return true;
            case "hub.prompt": return await File.ReadAllTextAsync(Path.Combine(AppContext.BaseDirectory, "agent-prompt.md"));
            case "clipboard.copy": Clipboard.SetText(p.GetProperty("text").GetString() ?? ""); return true;
            case "app.install": return await InstallAsync(p.GetProperty("id").GetString()!);
            case "app.cancel":
                if(_downloads.TryGetValue(p.GetProperty("id").GetString()!, out var cancel)) await cancel.CancelAsync(); return true;
            case "app.open": return await OpenModuleAsync(p.GetProperty("id").GetString()!);
            case "app.info":
                var infoManifest=_store.Manifest(p.GetProperty("id").GetString()!);
                return new {actions=infoManifest.Actions,integration=infoManifest.Integration is not null};
            case "app.launch":
                var launchId=p.GetProperty("id").GetString()!;
                if(_downloads.ContainsKey(launchId)||_removing.Contains(launchId)) throw new InvalidOperationException("Программа сейчас обновляется.");
                StandaloneLauncher.Launch(_store,launchId);return true;
            case "app.action":
                var actionId=p.GetProperty("id").GetString()!;
                if(_downloads.ContainsKey(actionId)||_removing.Contains(actionId))throw new InvalidOperationException("Программа сейчас обновляется.");
                var action=_store.Manifest(actionId).Actions.Single(a=>a.Id==p.GetProperty("action").GetString());
                if(action.WaitForExit)
                {
                    var launch=_store.Manifest(actionId).Launch!;
                    await StandaloneLauncher.RunHookAsync(_store,actionId,launch with {Arguments=[..launch.Arguments,..action.Arguments]});
                }
                else StandaloneLauncher.Launch(_store,actionId,action.Arguments);
                return true;
            case "app.integrate":
                var integrateId=p.GetProperty("id").GetString()!;
                if(_downloads.ContainsKey(integrateId)||_removing.Contains(integrateId))throw new InvalidOperationException("Дождитесь завершения установки.");
                await StandaloneLauncher.IntegrateAsync(_store,integrateId);
                await _store.ChangeAsync(s=>{var i=s.Installed.FindIndex(a=>a.Id==integrateId);s.Installed[i]=s.Installed[i] with {IntegrationError=null};});return Snapshot();
            case "app.shortcut":
                var shortcutId=p.GetProperty("id").GetString()!;
                StandaloneLauncher.CreateShortcut(_store,shortcutId,_catalog.Apps.Single(x=>x.Id==shortcutId).Name);return true;
            case "app.openData":
                var dataId=p.GetProperty("id").GetString()!;
                _store.Manifest(dataId);
                Process.Start(new ProcessStartInfo(_store.DataDirectory(dataId)) {UseShellExecute=true});return true;
            case "hub.fullscreen":
                var enabled=p.GetProperty("enabled").GetBoolean();
                SetFullscreen(enabled);return enabled;
            case "app.close":
                if(!ConfirmDiscard())throw new OperationCanceledException("Сохраните проект перед закрытием программы.");
                _moduleDirty=false;_activeModule = null; return true;
            case "app.uninstall":
            case "app.rollback":
                var appId = p.GetProperty("id").GetString()!;
                if(_activeModule==appId&&!ConfirmDiscard())throw new OperationCanceledException("Сохраните проект перед удалением или откатом программы.");
                if(_downloads.ContainsKey(appId)||!_removing.Add(appId)) throw new InvalidOperationException("Дождитесь завершения операции.");
                try
                {
                if(_workers.Remove(appId, out var worker)) await worker.DisposeAsync();
                await Task.Run(()=>StandaloneLauncher.AssertNotRunning(_store,appId));
                if(_activeModule == appId) { _activeModule = null;_moduleDirty=false; }
                if(method == "app.uninstall")
                {
                    if(p.TryGetProperty("backup",out var backup)&&backup.GetBoolean())await CreateBackupAsync(appId,true);
                    await StandaloneLauncher.CleanupAsync(_store,appId);
                    var deleteData=p.TryGetProperty("deleteData",out var delete)&&delete.GetBoolean();
                    await Task.Run(()=>_installer.UninstallAsync(appId,deleteData));
                    StandaloneLauncher.RemoveShortcut(appId);
                    WindowsInstallation.Remove(_store,appId);
                }
                else
                {
                    await StandaloneLauncher.CleanupAsync(_store,appId);
                    await _installer.RollbackAsync(appId);
                    await StandaloneLauncher.IntegrateAsync(_store,appId);
                    WindowsInstallation.Register(_store,appId,_catalog.Apps.Single(x=>x.Id==appId).Name);
                    StandaloneLauncher.CreateShortcut(_store,appId,_catalog.Apps.Single(x=>x.Id==appId).Name);
                }
                return Snapshot();
                }
                finally { _removing.Remove(appId); }
            case "module.call": return await ModuleCallAsync(p);
            default: throw new InvalidOperationException("Неизвестная команда хаба.");
        }
    }
    private void SetFullscreen(bool enabled)
    {
        if(enabled&&!_fullscreen) { _previousWindowState=WindowState;WindowState=WindowState.Normal;WindowStyle=WindowStyle.None;WindowState=WindowState.Maximized; }
        else if(!enabled&&_fullscreen) { WindowState=WindowState.Normal;WindowStyle=WindowStyle.SingleBorderWindow;WindowState=_previousWindowState; }
        _fullscreen=enabled;
    }
    private async Task<object> InstallAsync(string id)
    {
        if(_downloads.ContainsKey(id)||_removing.Contains(id)) throw new InvalidOperationException("Установка уже выполняется.");
        if(_activeModule == id) throw new InvalidOperationException("Закройте программу перед обновлением.");
        var app = _catalog.Apps.Single(x => x.Id == id);
        using var cts = new CancellationTokenSource(); _downloads[id] = cts;
        string? path = null;
        try
        {
            if(_workers.Remove(id, out var worker)) await worker.DisposeAsync();
            if(_store.Settings.Installed.Any(x=>x.Id==id)) await Task.Run(()=>StandaloneLauncher.AssertNotRunning(_store,id));
            var progress = new Progress<double>(v => Send(new { @event = "install.progress", id, progress = v, stage = "download" }));
            path = await _github.DownloadAsync(app, progress, cts.Token);
            Send(new { @event = "install.progress", id, progress = 1, stage = "verify" });
            var manifest=await Task.Run(() => _installer.InstallAsync(path, app, cts.Token), cts.Token);
            Send(new { @event="install.progress",id,progress=1,stage="integrate" });
            try { await StandaloneLauncher.IntegrateAsync(_store,id); }
            catch(Exception e)
            {
                await _store.ChangeAsync(s=>{var i=s.Installed.FindIndex(a=>a.Id==id);s.Installed[i]=s.Installed[i] with {IntegrationError=e.Message};});
                throw new IOException($"Версия {manifest.Version} установлена, но интеграция Windows не завершена. Повторите её в сведениях программы. "+e.Message,e);
            }
            if(_store.Manifest(id).Version!=manifest.Version || new HubStore(_store.Root).Manifest(id).Version!=manifest.Version)
                throw new IOException("Версия не активировалась. Откройте сведения и повторите установку.");
            try { StandaloneLauncher.CreateShortcut(_store,id,app.Name); }
            catch(Exception e) { Send(new { @event="notice",text="Программа установлена, но ярлык не создан: "+e.Message }); }
            WindowsInstallation.Register(_store,id,app.Name);
            return Snapshot();
        }
        finally { _downloads.Remove(id); if(path is not null && File.Exists(path)) File.Delete(path); }
    }
    private Task<object> OpenModuleAsync(string id)
    {
        if(_downloads.ContainsKey(id)||_removing.Contains(id)) throw new InvalidOperationException("Программа обновляется.");
        var m = _store.Manifest(id);
        if(!ConfirmDiscard())throw new OperationCanceledException("Сохраните проект перед переключением программы.");
        _moduleDirty=false;
        if(m.Mode=="standalone")
        {
            _activeModule=null;
            return Task.FromResult<object>(new { mode="standalone",id,version=m.Version,requiresElevation=m.RequiresElevation,actions=m.Actions });
        }
        var root = _store.AppVersion(id, m.Version);
        // A versioned origin invalidates HTML, scripts and styles together after an update.
        var host = "module-"+Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(id+"/"+m.Version)))[..24].ToLowerInvariant()+".thodhub.local";
        Browser.CoreWebView2.SetVirtualHostNameToFolderMapping(host, root, CoreWebView2HostResourceAccessKind.DenyCors);
        _activeModule = id;
        return Task.FromResult<object>(new { mode="embedded",url = $"https://{host}/{m.Ui}", id, version = m.Version, permissions = m.Permissions });
    }
    private async Task<object?> ModuleCallAsync(JsonElement p)
    {
        var id = p.GetProperty("appId").GetString()!;
        if(id != _activeModule||_removing.Contains(id)||_downloads.ContainsKey(id)) throw new UnauthorizedAccessException("Программа не открыта.");
        var method = p.GetProperty("method").GetString()!;
        var args = p.TryGetProperty("params", out var value) ? value : default;
        var m = _store.Manifest(id);
        void Require(string permission) { if(!m.Permissions.Contains(permission)) throw new UnauthorizedAccessException("Не объявлено разрешение: " + permission); }
        switch(method)
        {
            case "state.dirty": _moduleDirty=args.GetProperty("dirty").GetBoolean();return true;
            case "settings.get": Require("settings"); return _store.LoadModuleSettings(id);
            case "settings.set": Require("settings"); await _store.SaveModuleSettingsAsync(id, args); return true;
            case "settings.path": Require("settings"); return _store.DataDirectory(id);
            case "clipboard.write": Require("clipboard.write"); Clipboard.SetText(args.GetProperty("text").GetString() ?? ""); return true;
            case "dialog.folder": Require("dialog.folder"); var dialog = new OpenFolderDialog { Title = "Выбрать папку" }; return dialog.ShowDialog(this) == true ? dialog.FolderName : null;
            case "document.pdf":
            case "document.print":
                Require(method);
                return await _printer.PrintAsync(this,args.GetProperty("html").GetString()!,args.GetProperty("name").GetString()!,_store.AppVersion(id,m.Version),Path.Combine(_store.Root,"WebView"),method=="document.pdf");
            case "files.export":
                Require("files.export");
                var name = Path.GetFileName(args.GetProperty("name").GetString() ?? "export.txt");
                var save = new SaveFileDialog { FileName = name, Title = "Сохранить файл" };
                if(save.ShowDialog(this) != true) return new { canceled = true };
                var bytes = Convert.FromBase64String(args.GetProperty("base64").GetString()!);
                if(bytes.Length > 8 * 1024 * 1024) throw new InvalidDataException("Файл превышает 8 МБ.");
                await File.WriteAllBytesAsync(save.FileName, bytes); return new { canceled = false, filePath = save.FileName };
            default:
                if(!method.StartsWith("worker.", StringComparison.Ordinal)) throw new InvalidOperationException("Неизвестная команда модуля.");
                Require("worker");
                if(m.Worker is null) throw new InvalidOperationException("У программы нет исполняемой части.");
                if(!_moduleGates.TryGetValue(id, out var gate)) _moduleGates[id] = gate = new SemaphoreSlim(1, 1);
                await gate.WaitAsync();
                ModuleProcess process;
                try
                {
                    if(id != _activeModule||_removing.Contains(id)||_downloads.ContainsKey(id)) throw new InvalidOperationException("Программа закрыта.");
                    if(!_workers.TryGetValue(id, out process!) || !process.IsRunning)
                    {
                        if(process is not null) await process.DisposeAsync();
                        var root = _store.AppVersion(id, m.Version);
                        process = new ModuleProcess(Contract.Inside(root, m.Worker.Executable), m.Worker.Arguments, root, _store.DataDirectory(id));
                        _workers[id] = process;
                    }
                }
                finally { gate.Release(); }
                return await process.CallAsync(method[7..], args.ValueKind == JsonValueKind.Undefined ? null : args);
        }
    }
}
