use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tao::event_loop::EventLoopProxy;
use tao::window::Window;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Environment, ICoreWebView2Environment6, ICoreWebView2Settings4,
    ICoreWebView2_16, ICoreWebView2_3, ICoreWebView2_4, ICoreWebView2_7,
    ICoreWebView2DownloadStartingEventArgs, ICoreWebView2NavigationCompletedEventArgs,
    ICoreWebView2NewWindowRequestedEventArgs, ICoreWebView2PermissionRequestedEventArgs,
    ICoreWebView2PrintSettings, ICoreWebView2WebResourceRequestedEventArgs,
    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS, COREWEBVIEW2_PERMISSION_STATE_DENY,
    COREWEBVIEW2_PRINT_DIALOG_KIND_SYSTEM, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
};
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler, DownloadStartingEventHandler,
    NavigationCompletedEventHandler, NewWindowRequestedEventHandler,
    PermissionRequestedEventHandler, PrintToPdfCompletedHandler, WebResourceRequestedEventHandler,
};
use windows::core::{BOOL, HSTRING, Interface, PWSTR};
use windows::Win32::System::Com::IStream;
use wry::{NewWindowResponse, PermissionResponse, WebContext, WebView, WebViewBuilder, WebViewExtWindows};

use crate::hub::UserEvent;

pub const PRINT_HOST: &str = "print.thodhub.local";

#[derive(Clone)]
pub struct PrintRequest {
    pub html: String,
    pub name: String,
    pub module_root: PathBuf,
    pub data_root: PathBuf,
    pub to_pdf: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum PrintSignal {
    Navigated(bool),
    FontsDone(bool),
    PdfDone(bool),
    Timeout,
}

pub struct PrintJob {
    pub id: u64,
    pub window: Window,
    pub webview: WebView,
    pub to_pdf: bool,
    pub output: Option<PathBuf>,
    pub started: std::time::Instant,
}

/// Creates an offscreen WebView2 document surface and starts loading `html`.
/// Completion is reported through `UserEvent::PrintSignal`.
pub fn start(
    window: &Window,
    request: &PrintRequest,
    id: u64,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<WebView> {
    let mut context = WebContext::new(Some(request.data_root.clone()));
    let allowed = format!("https://{PRINT_HOST}/");
    let allowed_clone = allowed.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_devtools(false)
        .with_clipboard(false)
        .with_navigation_handler(move |url| {
            url.starts_with(&allowed_clone) || url.starts_with("data:") || url == "about:blank"
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_permission_handler(|_| PermissionResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .build(window)
        .map_err(|e| anyhow!("Не удалось подготовить документ. {e}"))?;

    let core = webview.webview();
    let environment = webview.environment();

    unsafe {
        if let Ok(settings) = core.Settings() {
            let _ = settings.SetIsScriptEnabled(false);
            let _ = settings.SetIsWebMessageEnabled(false);
            let _ = settings.SetAreDevToolsEnabled(false);
            let _ = settings.SetAreDefaultContextMenusEnabled(false);
            let _ = settings.SetIsStatusBarEnabled(false);
            if let Ok(settings4) = settings.cast::<ICoreWebView2Settings4>() {
                let _ = settings4.SetIsPasswordAutosaveEnabled(false);
            }
        }

        let core3: ICoreWebView2_3 = core
            .cast()
            .map_err(|e| anyhow!("WebView2 не поддерживает виртуальные узлы. {e}"))?;
        core3
            .SetVirtualHostNameToFolderMapping(
                &HSTRING::from(PRINT_HOST),
                &HSTRING::from(request.module_root.to_string_lossy().to_string()),
                COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
            )
            .map_err(|e| anyhow!("Не удалось подключить файлы программы. {e}"))?;

        let mut token = 0i64;
        core.AddWebResourceRequestedFilter(&HSTRING::from("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
            .map_err(|e| anyhow!("Не удалось подготовить документ. {e}"))?;
        let env_for_filter = environment.clone();
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(
                move |_sender, args: Option<ICoreWebView2WebResourceRequestedEventArgs>| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let Ok(request) = args.Request() else {
                        return Ok(());
                    };
                    let mut uri = PWSTR::null();
                    if request.Uri(&mut uri).is_err() {
                        return Ok(());
                    }
                    let uri = take_pwstr(uri);
                    if uri.starts_with("https://print.thodhub.local/")
                        || uri.starts_with("data:")
                        || uri == "about:blank"
                    {
                        return Ok(());
                    }
                    if let Ok(response) = env_for_filter.CreateWebResourceResponse(
                        None::<&IStream>,
                        403,
                        &HSTRING::from("Forbidden"),
                        &HSTRING::from(""),
                    ) {
                        let _ = args.SetResponse(&response);
                    }
                    Ok(())
                },
            )),
            &mut token,
        )?;

        let mut token = 0i64;
        core.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(
                |_sender, args: Option<ICoreWebView2NewWindowRequestedEventArgs>| {
                    if let Some(args) = args {
                        let _ = args.SetHandled(true);
                    }
                    Ok(())
                },
            )),
            &mut token,
        )?;

        let mut token = 0i64;
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(
                |_sender, args: Option<ICoreWebView2PermissionRequestedEventArgs>| {
                    if let Some(args) = args {
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                    }
                    Ok(())
                },
            )),
            &mut token,
        )?;

        let mut token = 0i64;
        core.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(
                |_sender, args: Option<ICoreWebView2DownloadStartingEventArgs>| {
                    if let Some(args) = args {
                        let _ = args.SetCancel(true);
                    }
                    Ok(())
                },
            )),
            &mut token,
        )?;

        let completion = proxy.clone();
        let mut token = 0i64;
        core.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(
                move |_sender, args: Option<ICoreWebView2NavigationCompletedEventArgs>| {
                    let success = args
                        .map(|args| {
                            let mut value = BOOL::default();
                            args.IsSuccess(&mut value)
                                .map(|_| value.as_bool())
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    let _ = completion.send_event(UserEvent::PrintSignal(
                        id,
                        PrintSignal::Navigated(success),
                    ));
                    Ok(())
                },
            )),
            &mut token,
        )?;

        core.NavigateToString(&HSTRING::from(request.html.clone()))
            .map_err(|e| anyhow!("Не удалось подготовить документ. {e}"))?;
    }

    Ok(webview)
}

/// Waits for fonts and images before printing, matching DocumentPrinter.cs.
pub fn wait_for_assets(
    core: &ICoreWebView2,
    id: u64,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<()> {
    const EXPRESSION: &str = "Promise.all([document.fonts.ready,...Array.from(document.images).map(i=>i.decode().catch(()=>{}))])";
    let parameters = serde_json::json!({ "expression": EXPRESSION, "awaitPromise": true }).to_string();
    let completion = proxy.clone();
    unsafe {
        core.CallDevToolsProtocolMethod(
            &HSTRING::from("Runtime.evaluate"),
            &HSTRING::from(parameters),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |error, _| {
                let _ = completion
                    .send_event(UserEvent::PrintSignal(id, PrintSignal::FontsDone(error.is_ok())));
                Ok(())
            })),
        )
        .map_err(|e| anyhow!("Не удалось подготовить документ. {e}"))?;
    }
    Ok(())
}

pub fn print_to_pdf(
    core: &ICoreWebView2,
    environment: &ICoreWebView2Environment,
    output: &Path,
    id: u64,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<()> {
    let settings: ICoreWebView2PrintSettings = unsafe {
        environment
            .cast::<ICoreWebView2Environment6>()
            .and_then(|environment| environment.CreatePrintSettings())
            .map_err(|e| anyhow!("Не удалось подготовить PDF. {e}"))?
    };
    unsafe {
        settings.SetPageWidth(210.0 / 25.4)?;
        settings.SetPageHeight(297.0 / 25.4)?;
        settings.SetMarginTop(0.0)?;
        settings.SetMarginBottom(0.0)?;
        settings.SetMarginLeft(0.0)?;
        settings.SetMarginRight(0.0)?;
        settings.SetShouldPrintBackgrounds(true)?;
        settings.SetShouldPrintHeaderAndFooter(false)?;
    }
    let core7: ICoreWebView2_7 = core
        .cast()
        .map_err(|e| anyhow!("WebView2 не поддерживает сохранение PDF. {e}"))?;
    let completion = proxy.clone();
    unsafe {
        core7
            .PrintToPdf(
                &HSTRING::from(output.to_string_lossy().to_string()),
                &settings,
                &PrintToPdfCompletedHandler::create(Box::new(move |error, success| {
                    let _ = completion.send_event(UserEvent::PrintSignal(
                        id,
                        PrintSignal::PdfDone(error.is_ok() && success),
                    ));
                    Ok(())
                })),
            )
            .map_err(|e| anyhow!("Не удалось сохранить PDF. {e}"))?;
    }
    Ok(())
}

pub fn show_print_ui(core: &ICoreWebView2) -> Result<()> {
    let core16: ICoreWebView2_16 = core
        .cast()
        .map_err(|e| anyhow!("WebView2 не поддерживает печать. {e}"))?;
    unsafe {
        core16
            .ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_SYSTEM)
            .map_err(|e| anyhow!("Не удалось открыть печать. {e}"))?;
    }
    Ok(())
}
