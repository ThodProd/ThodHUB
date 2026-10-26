using System.IO;
using System.Windows;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.Wpf;
using Microsoft.Win32;

namespace ThodHub;

public sealed class DocumentPrinter
{
    private readonly SemaphoreSlim _gate = new(1, 1);
    public async Task<object> PrintAsync(Window owner, string html, string name, string moduleRoot, string webViewDataRoot, bool toPdf)
    {
        if(html.Length > 8 * 1024 * 1024) throw new InvalidDataException("Документ превышает 8 МБ.");
        string? output = null;
        if(toPdf)
        {
            var dialog = new SaveFileDialog { FileName = Path.GetFileName(name), Filter = "PDF|*.pdf", DefaultExt = ".pdf", Title = "Сохранить резюме" };
            if(dialog.ShowDialog(owner) != true) return new { canceled = true };
            output = dialog.FileName;
        }
        await _gate.WaitAsync();
        var web = new WebView2();
        var window = new Window { Width = 1000, Height = 1300, Left = -32000, Top = -32000, ShowActivated = false, ShowInTaskbar = false, WindowStyle = WindowStyle.None, Content = web };
        try
        {
            window.Show();
            var environment = await CoreWebView2Environment.CreateAsync(null, webViewDataRoot);
            await web.EnsureCoreWebView2Async(environment);
            var core = web.CoreWebView2;
            core.Settings.IsScriptEnabled = false;
            core.Settings.IsWebMessageEnabled = false;
            core.Settings.AreDevToolsEnabled = false;
            core.Settings.AreDefaultContextMenusEnabled = false;
            core.NewWindowRequested += (_, e) => e.Handled = true;
            core.PermissionRequested += (_, e) => e.State = CoreWebView2PermissionState.Deny;
            core.DownloadStarting += (_, e) => e.Cancel = true;
            core.SetVirtualHostNameToFolderMapping("print.thodhub.local",moduleRoot,CoreWebView2HostResourceAccessKind.DenyCors);
            core.AddWebResourceRequestedFilter("*",CoreWebView2WebResourceContext.All);
            core.WebResourceRequested += (_, e) =>
            {
                if(!e.Request.Uri.StartsWith("https://print.thodhub.local/",StringComparison.Ordinal) && !e.Request.Uri.StartsWith("data:",StringComparison.Ordinal) && e.Request.Uri != "about:blank")
                    e.Response=environment.CreateWebResourceResponse(null,403,"Forbidden","");
            };
            var loaded = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
            core.NavigationCompleted += (_, e) => { if(e.IsSuccess) loaded.TrySetResult(); else loaded.TrySetException(new IOException("Не удалось подготовить документ.")); };
            core.NavigateToString(html);
            await loaded.Task.WaitAsync(TimeSpan.FromSeconds(20));
            // Font/image loading is bounded and does not block the shell's renderer.
            await core.CallDevToolsProtocolMethodAsync("Runtime.evaluate", "{\"expression\":\"Promise.all([document.fonts.ready,...Array.from(document.images).map(i=>i.decode().catch(()=>{}))])\",\"awaitPromise\":true}").WaitAsync(TimeSpan.FromSeconds(15));
            if(toPdf)
            {
                var settings = environment.CreatePrintSettings();
                settings.PageWidth = 210.0 / 25.4; settings.PageHeight = 297.0 / 25.4;
                settings.MarginTop = settings.MarginBottom = settings.MarginLeft = settings.MarginRight = 0;
                settings.ShouldPrintBackgrounds = true; settings.ShouldPrintHeaderAndFooter = false;
                if(!await core.PrintToPdfAsync(output!,settings)) throw new IOException("Не удалось сохранить PDF.");
                return new { canceled=false,filePath=output };
            }
            // System print dialog is deliberately explicit; no silent printing.
            core.ShowPrintUI(CoreWebView2PrintDialogKind.System);
            return new { success=true };
        }
        finally { web.Dispose();window.Close();_gate.Release(); }
    }
}
