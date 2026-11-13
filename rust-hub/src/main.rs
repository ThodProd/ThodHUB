#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backups;
mod contract;
mod github;
mod hub;
mod installer;
mod models;
mod printer;
mod process;
mod selftest;
mod store;
mod system;
mod updater;
mod util;

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tao::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Fullscreen, Window, WindowBuilder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, PipeMode, ServerOptions};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Settings4, ICoreWebView2_3,
    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
};
use webview2_com::{take_pwstr, WebMessageReceivedEventHandler};
use windows::core::{HSTRING, Interface, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, HANDLE,
};
use windows::Win32::System::Threading::CreateMutexW;
use wry::dpi::PhysicalSize as WryPhysicalSize;
use wry::{NewWindowResponse, PermissionResponse, Rect, WebContext, WebView, WebViewBuilder};
use wry::WebViewExtWindows;

use crate::hub::{response_error, response_ok, Hub, UserEvent};
use crate::printer::{PrintRequest, PrintSignal};
use crate::store::HubStore;

fn main() {
    trace("main start");
    if let Err(error) = run() {
        trace(&format!("fatal: {error:#}"));
        util::show_error(&format!("{error:#}"));
        std::process::exit(1);
    }
    trace("main end");
}

#[cfg(debug_assertions)]
fn trace(message: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("thodhub-rust-trace.log"))
    {
        let _ = writeln!(file, "{} {message}", std::process::id());
    }
}

#[cfg(not(debug_assertions))]
fn trace(_message: &str) {}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // One-shot CLI modes are handled before the single-instance check.
    if args.len() >= 2 && args[0] == "--launch" {
        let store = HubStore::new(util::installed_home())?;
        let id = &args[1];
        if store.installed(id).is_some() {
            if let Ok(manifest) = store.manifest(id) {
                if manifest.mode == "standalone" {
                    match system::StandaloneLauncher::launch(&store, id, &args[2..]) {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            util::show_error(&format!("{error:#}"));
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    if args.len() == 4 && args[0] == "--invoke" {
        invoke(&args[1], &args[2], &args[3]);
        return Ok(());
    }

    if args.first().map(String::as_str) == Some("--selftest") {
        let home = args
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(util::installed_home);
        let mut only = Vec::new();
        let mut cached = None;
        let mut index = 2;
        while index < args.len() {
            if args[index] == "--catalog" && index + 1 < args.len() {
                cached = Some(PathBuf::from(&args[index + 1]));
                index += 2;
            } else {
                only.push(args[index].clone());
                index += 1;
            }
        }
        return selftest::run(home, hub_directory().join("catalog.json"), only, cached);
    }

    let instance = instance_name(&util::installed_home())?;
    trace(&format!("instance {instance}"));
    let requested =
        if args.len() >= 2 && matches!(args[0].as_str(), "--launch" | "--open" | "--uninstall") {
            if args[0] == "--uninstall" {
                format!("remove:{}", args[1])
            } else {
                args[1].clone()
            }
        } else {
            String::new()
        };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Не удалось запустить фоновые задачи.")?;
    let runtime_handle = runtime.handle().clone();

    let mutex = SingleInstance::acquire(&instance)?;
    trace(&format!("mutex already={}", mutex.already_running));
    if mutex.already_running {
        if let Err(error) = runtime_handle.block_on(send_instance_request(&instance, &requested)) {
            util::show_error(&format!(
                "Не удалось передать команду открытому хабу: {error}"
            ));
        }
        return Ok(());
    }

    let store = HubStore::new(util::installed_home())?;
    let bundled = hub_directory().join("catalog.json");
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let hub = {
        // reqwest's resolver expects a Tokio runtime context while the client is built.
        let _enter = runtime.enter();
        Arc::new(Hub::new(store, proxy.clone(), bundled)?)
    };

    {
        let listener = proxy.clone();
        let pipe = instance.clone();
        runtime_handle.spawn(async move {
            listen_instance(pipe, listener).await;
        });
    }

    let mut state = AppState {
        hub,
        window: None,
        webview: None,
        tray: None,
        fullscreen: false,
        fullscreen_restore_maximized: false,
        exit_requested: false,
        closing: false,
        ready: Rc::new(Cell::new(false)),
        pending_open: Rc::new(RefCell::new(Vec::new())),
        print: None,
        next_print: 1,
    };
    if !requested.is_empty() {
        state.pending_open.borrow_mut().push(requested);
    }

    event_loop.run(move |event, target, control_flow| match event {        Event::NewEvents(StartCause::Init) => {
            trace("NewEvents Init");
            if state.window.is_none() && !state.closing {
                match create_main_window(&mut state, target, &runtime_handle, &proxy) {
                    Ok(()) => {}
                    Err(error) => {
                        trace(&format!("window error: {error:#}"));
                        util::show_error(&format!(
                            "Не удалось запустить интерфейс\n\n{error:#}\n\nДля ThodHUB нужен Microsoft Edge WebView2 Runtime. Если компонент отсутствует, скачайте Evergreen Runtime с сайта Microsoft, установите его и откройте хаб снова."
                        ));
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
        }
        Event::WindowEvent { window_id, event, .. } => {
            if state
                .window
                .as_ref()
                .map(|window| window.id() == window_id)
                .unwrap_or(false)
            {
                match event {
                    WindowEvent::CloseRequested => {
                        if state.exit_requested {
                            begin_exit(&mut state, &runtime_handle, control_flow);
                        } else if let Some(window) = state.window.as_ref() {
                            window.set_visible(false);
                        }
                    }
                    WindowEvent::Resized(size) => {
                        if let Some(webview) = state.webview.as_ref() {
                            let _ = webview.set_bounds(Rect {
                                position: LogicalPosition::new(0.0, 0.0).into(),
                                size: WryPhysicalSize::new(size.width, size.height).into(),
                            });
                        }
                    }
                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        if let Some(webview) = state.webview.as_ref() {
                            let _ = webview.set_bounds(Rect {
                                position: LogicalPosition::new(0.0, 0.0).into(),
                                size: WryPhysicalSize::new(
                                    new_inner_size.width,
                                    new_inner_size.height,
                                )
                                .into(),
                            });
                        }
                    }
                    WindowEvent::Destroyed => {
                        if state.exit_requested {
                            *control_flow = ControlFlow::Exit;
                        }
                    }
                    _ => {}
                }
            }
        }
        Event::UserEvent(event) => {
            handle_user_event(event, &mut state, target, control_flow, &runtime_handle, &proxy);
        }
        _ => {}
    });
}

// ---------------------------------------------------------------------------
// Window, bridge and print state
// ---------------------------------------------------------------------------

struct AppState {
    hub: Arc<Hub>,
    window: Option<Window>,
    webview: Option<WebView>,
    tray: Option<tray_icon::TrayIcon>,
    fullscreen: bool,
    fullscreen_restore_maximized: bool,
    exit_requested: bool,
    closing: bool,
    ready: Rc<Cell<bool>>,
    pending_open: Rc<RefCell<Vec<String>>>,
    print: Option<PrintState>,
    next_print: u64,
}

struct PrintState {
    job: u64,
    bridge_id: String,
    window: Window,
    webview: WebView,
    to_pdf: bool,
    output: Option<PathBuf>,
}

fn create_main_window(
    state: &mut AppState,
    target: &EventLoopWindowTarget<UserEvent>,
    runtime: &tokio::runtime::Handle,
    proxy: &EventLoopProxy<UserEvent>,
) -> Result<()> {
    let icons = load_icons();
    let mut builder = WindowBuilder::new()
        .with_title("ThodHUB")
        .with_inner_size(LogicalSize::new(1240.0, 820.0))
        .with_min_inner_size(LogicalSize::new(820.0, 560.0));
    if let Some((window_icon, _)) = &icons {
        builder = builder.with_window_icon(Some(window_icon.clone()));
    }
    let window = builder.build(target)?;

    if let Some((_, tray_icon)) = &icons {
        state.tray = create_tray(tray_icon.clone(), proxy);
    }

    let host = util::host_name();
    let origin = format!("https://{host}");
    let wwwroot = hub_directory().join("wwwroot");
    let devtools = std::env::var("THODHUB_DEV").ok().as_deref() == Some("1");
    let navigation_prefix = format!("{origin}/");
    let navigation_prefix_handler = navigation_prefix.clone();

    let mut context = WebContext::new(Some(state.hub.store.root().join("WebView")));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_background_color((247, 248, 250, 255))
        .with_devtools(devtools)
        .with_navigation_handler(move |url| url.starts_with(navigation_prefix_handler.as_str()))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_permission_handler(|_| PermissionResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .build(&window)?;

    let core = webview.webview();
    unsafe {
        if let Ok(settings) = core.Settings() {
            let _ = settings.SetAreDevToolsEnabled(devtools);
            let _ = settings.SetAreDefaultContextMenusEnabled(false);
            let _ = settings.SetIsStatusBarEnabled(false);
            if let Ok(settings4) = settings.cast::<ICoreWebView2Settings4>() {
                let _ = settings4.SetIsPasswordAutosaveEnabled(false);
            }
        }
        let core3: ICoreWebView2_3 = core
            .cast()
            .map_err(|error| anyhow!("WebView2 не поддерживает виртуальные узлы. {error}"))?;
        core3.SetVirtualHostNameToFolderMapping(
            &HSTRING::from(host.clone()),
            &HSTRING::from(wwwroot.to_string_lossy().to_string()),
            COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
        )?;
        attach_bridge(
            &core,
            state.hub.clone(),
            runtime.clone(),
            proxy.clone(),
            state.ready.clone(),
            state.pending_open.clone(),
            navigation_prefix,
        )?;
        core.Navigate(&HSTRING::from(format!("{origin}/index.html")))?;
    }

    state.window = Some(window);
    state.webview = Some(webview);
    trace("window created");
    Ok(())
}

fn attach_bridge(
    core: &ICoreWebView2,
    hub: Arc<Hub>,
    runtime: tokio::runtime::Handle,
    proxy: EventLoopProxy<UserEvent>,
    ready: Rc<Cell<bool>>,
    pending: Rc<RefCell<Vec<String>>>,
    origin_prefix: String,
) -> Result<()> {
    let core_for_messages = core.clone();
    let mut token = 0i64;
    unsafe {
        core.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut source = PWSTR::null();
                if args.Source(&mut source).is_err() {
                    return Ok(());
                }
                if !take_pwstr(source).starts_with(&origin_prefix) {
                    return Ok(());
                }
                let mut json = PWSTR::null();
                if args.WebMessageAsJson(&mut json).is_err() {
                    return Ok(());
                }
                let text = take_pwstr(json);
                if text.len() > 12 * 1024 * 1024 {
                    return Ok(());
                }
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    return Ok(());
                };
                let Some(id) = message.get("id").and_then(|value| value.as_str()) else {
                    return Ok(());
                };
                let Some(method) = message.get("method").and_then(|value| value.as_str()) else {
                    return Ok(());
                };
                let id = id.to_string();
                let method = method.to_string();
                let params = message.get("params").cloned();

                if method == "hub.ready" {
                    ready.set(true);
                    let requests: Vec<String> = pending.borrow_mut().drain(..).collect();
                    for request in requests {
                        execute_open(&core_for_messages, &request);
                    }
                }

                if method == "module.call" {
                    if let Some(params) = params.as_ref() {
                        let inner = params
                            .get("method")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        if inner == "document.pdf" || inner == "document.print" {
                            match hub.document_request(params) {
                                Ok(request) => {
                                    let _ = proxy.send_event(UserEvent::Print {
                                        request,
                                        reply: id.clone(),
                                    });
                                }
                                Err(error) => {
                                    let _ = core_for_messages.PostWebMessageAsJson(
                                        &HSTRING::from(response_error(&id, format!("{error}"))),
                                    );
                                }
                            }
                            return Ok(());
                        }
                    }
                }

                let hub = hub.clone();
                let runtime = runtime.clone();
                let proxy = proxy.clone();
                runtime.spawn(async move {
                    let result = hub.handle(&method, params).await;
                    let text = match result {
                        Ok(value) => response_ok(&id, value),
                        Err(error) => response_error(&id, error),
                    };
                    let _ = proxy.send_event(UserEvent::Deliver(text));
                });
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

fn execute_open(core: &ICoreWebView2, request: &str) {
    let script = if let Some(app) = request.strip_prefix("remove:") {
        if contract::id(app).is_err() {
            return;
        }
        format!(
            "confirmRemoval({})",
            serde_json::to_string(app).unwrap_or_default()
        )
    } else if request.is_empty() {
        return;
    } else {
        format!(
            "openApp({})",
            serde_json::to_string(request).unwrap_or_default()
        )
    };
    unsafe {
        let _ = core.ExecuteScript(&HSTRING::from(script), None);
    }
}

fn handle_user_event(
    event: UserEvent,
    state: &mut AppState,
    target: &EventLoopWindowTarget<UserEvent>,
    control_flow: &mut ControlFlow,
    runtime: &tokio::runtime::Handle,
    proxy: &EventLoopProxy<UserEvent>,
) {
    match event {
        UserEvent::Message(text) | UserEvent::Deliver(text) => post_message(state, &text),
        UserEvent::Execute(script) => {
            if let Some(webview) = state.webview.as_ref() {
                let _ = webview.evaluate_script(&script);
            }
        }
        UserEvent::Open(request) => {
            if state.ready.get() {
                if let Some(webview) = state.webview.as_ref() {
                    execute_open(&webview.webview(), &request);
                }
            } else {
                state.pending_open.borrow_mut().push(request);
            }
        }
        UserEvent::TrayRestore => restore_from_tray(state),
        UserEvent::TrayExit => {
            state.exit_requested = true;
            begin_exit(state, runtime, control_flow);
        }
        UserEvent::Exit => begin_exit(state, runtime, control_flow),
        UserEvent::MapHost { host, root, reply } => {
            let result = match state.webview.as_ref() {
                Some(webview) => unsafe {
                    webview
                        .webview()
                        .cast::<ICoreWebView2_3>()
                        .map_err(|error| error.to_string())
                        .and_then(|core| {
                            core.SetVirtualHostNameToFolderMapping(
                                &HSTRING::from(host),
                                &HSTRING::from(root.to_string_lossy().to_string()),
                                COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
                            )
                            .map_err(|error| error.to_string())
                        })
                },
                None => Err("Окно хаба закрыто.".to_string()),
            };
            let _ = reply.send(result);
        }
        UserEvent::Fullscreen { enabled, reply } => {
            set_fullscreen(state, enabled);
            let _ = reply.send(enabled);
        }
        UserEvent::Print { request, reply } => {
            begin_print(state, target, request, reply, runtime, proxy);
        }
        UserEvent::PrintSignal(job, signal) => {
            handle_print_signal(state, job, signal, proxy);
        }
    }
}

fn post_message(state: &AppState, text: &str) {
    if let Some(webview) = state.webview.as_ref() {
        unsafe {
            let _ = webview
                .webview()
                .PostWebMessageAsJson(&HSTRING::from(text.to_string()));
        }
    }
}

fn restore_from_tray(state: &AppState) {
    if let Some(window) = state.window.as_ref() {
        window.set_visible(true);
        window.set_minimized(false);
        window.set_focus();
    }
}

fn set_fullscreen(state: &mut AppState, enabled: bool) {
    let Some(window) = state.window.as_ref() else {
        state.fullscreen = enabled;
        return;
    };
    if enabled && !state.fullscreen {
        state.fullscreen_restore_maximized = window.is_maximized();
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    } else if !enabled && state.fullscreen {
        window.set_fullscreen(None);
        if state.fullscreen_restore_maximized {
            window.set_maximized(true);
        }
    }
    state.fullscreen = enabled;
}

fn begin_exit(state: &mut AppState, runtime: &tokio::runtime::Handle, control_flow: &mut ControlFlow) {
    if state.closing {
        *control_flow = ControlFlow::Exit;
        return;
    }
    if !util::confirm_discard(state.hub.module_dirty()) {
        state.exit_requested = false;
        return;
    }
    state.closing = true;
    runtime.block_on(state.hub.shutdown());
    if let Some(webview) = state.webview.take() {
        drop(webview);
    }
    if let Some(tray) = state.tray.take() {
        drop(tray);
    }
    if let Some(window) = state.window.take() {
        window.set_visible(false);
        drop(window);
    }
    *control_flow = ControlFlow::Exit;
}

// ---------------------------------------------------------------------------
// Document printing
// ---------------------------------------------------------------------------

fn begin_print(
    state: &mut AppState,
    target: &EventLoopWindowTarget<UserEvent>,
    request: PrintRequest,
    bridge_id: String,
    runtime: &tokio::runtime::Handle,
    proxy: &EventLoopProxy<UserEvent>,
) {
    let output = if request.to_pdf {
        let name = Path::new(&request.name)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("document.pdf")
            .to_string();
        let Some(mut path) = rfd::FileDialog::new()
            .set_title("Сохранить резюме")
            .add_filter("PDF", &["pdf"])
            .set_file_name(&name)
            .save_file()
        else {
            post_message(
                state,
                &response_ok(&bridge_id, json!({ "canceled": true })),
            );
            return;
        };
        if path.extension().and_then(|value| value.to_str()).is_none() {
            path.set_extension("pdf");
        }
        Some(path)
    } else {
        None
    };

    let window = match WindowBuilder::new()
        .with_title("ThodHUB")
        .with_inner_size(LogicalSize::new(1000.0, 1300.0))
        .with_position(PhysicalPosition::new(-32000, -32000))
        .with_resizable(false)
        .with_decorations(false)
        .with_focused(false)
        .with_visible(true)
        .build(target)
    {
        Ok(window) => window,
        Err(error) => {
            post_message(
                state,
                &response_error(&bridge_id, format!("Не удалось подготовить документ. {error}")),
            );
            return;
        }
    };

    let job = state.next_print;
    state.next_print += 1;
    let webview = match printer::start(&window, &request, job, proxy.clone()) {
        Ok(webview) => webview,
        Err(error) => {
            window.set_visible(false);
            drop(window);
            post_message(state, &response_error(&bridge_id, format!("{error:#}")));
            return;
        }
    };

    state.print = Some(PrintState {
        job,
        bridge_id,
        window,
        webview,
        to_pdf: request.to_pdf,
        output,
    });

    let timeout = proxy.clone();
    runtime.spawn(async move {
        tokio::time::sleep(Duration::from_secs(45)).await;
        let _ = timeout.send_event(UserEvent::PrintSignal(job, PrintSignal::Timeout));
    });
}

fn handle_print_signal(
    state: &mut AppState,
    job: u64,
    signal: PrintSignal,
    proxy: &EventLoopProxy<UserEvent>,
) {
    if state.print.as_ref().map(|print| print.job) != Some(job) {
        return;
    }
    match signal {
        PrintSignal::Navigated(true) => {
            let Some(print) = state.print.as_ref() else {
                return;
            };
            match printer::wait_for_assets(&print.webview.webview(), job, proxy.clone()) {
                Ok(()) => {}
                Err(error) => finish_print(state, Err(format!("{error:#}"))),
            }
        }
        PrintSignal::FontsDone(true) => {
            let Some(print) = state.print.as_ref() else {
                return;
            };
            if print.to_pdf {
                let output = print.output.clone().unwrap_or_default();
                match printer::print_to_pdf(
                    &print.webview.webview(),
                    &print.webview.environment(),
                    &output,
                    job,
                    proxy.clone(),
                ) {
                    Ok(()) => {}
                    Err(error) => finish_print(state, Err(format!("{error:#}"))),
                }
            } else {
                match printer::show_print_ui(&print.webview.webview()) {
                    Ok(()) => finish_print(state, Ok(json!({ "success": true }))),
                    Err(error) => finish_print(state, Err(format!("{error:#}"))),
                }
            }
        }
        PrintSignal::PdfDone(true) => {
            let path = state
                .print
                .as_ref()
                .and_then(|print| print.output.as_ref())
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default();
            finish_print(
                state,
                Ok(json!({ "canceled": false, "filePath": path })),
            );
        }
        PrintSignal::PdfDone(false) => {
            finish_print(state, Err("Не удалось сохранить PDF.".into()))
        }
        PrintSignal::Navigated(false)
        | PrintSignal::FontsDone(false)
        | PrintSignal::Timeout => {
            finish_print(state, Err("Не удалось подготовить документ.".into()))
        }
    }
}

fn finish_print(
    state: &mut AppState,
    result: std::result::Result<Value, String>,
) {
    let Some(print) = state.print.take() else {
        return;
    };
    print.window.set_visible(false);
    drop(print.webview);
    drop(print.window);
    let text = match result {
        Ok(value) => response_ok(&print.bridge_id, value),
        Err(error) => response_error(&print.bridge_id, error),
    };
    post_message(state, &text);
}

// ---------------------------------------------------------------------------
// Tray icon
// ---------------------------------------------------------------------------

fn load_icons() -> Option<(tao::window::Icon, tray_icon::Icon)> {
    let bytes = include_bytes!("../assets/thodhub.ico");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Ico)
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    let window_icon = tao::window::Icon::from_rgba(rgba.clone(), width, height).ok()?;
    let tray_icon = tray_icon::Icon::from_rgba(rgba, width, height).ok()?;
    Some((window_icon, tray_icon))
}

fn create_tray(
    icon: tray_icon::Icon,
    proxy: &EventLoopProxy<UserEvent>,
) -> Option<tray_icon::TrayIcon> {
    use muda::{Menu, MenuEvent, MenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let menu = Menu::new();
    let open = MenuItem::new("Открыть", true, None);
    let close = MenuItem::new("Закрыть", true, None);
    menu.append(&open).ok()?;
    menu.append(&close).ok()?;
    let open_id = open.id().clone();
    let close_id = close.id().clone();

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == open_id {
            let _ = menu_proxy.send_event(UserEvent::TrayRestore);
        } else if event.id == close_id {
            let _ = menu_proxy.send_event(UserEvent::TrayExit);
        }
    }));

    let tray_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::DoubleClick { .. } = event {
            let _ = tray_proxy.send_event(UserEvent::TrayRestore);
        }
    }));

    TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("ThodHUB")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()
}

// ---------------------------------------------------------------------------
// Single instance
// ---------------------------------------------------------------------------

struct SingleInstance {
    handle: HANDLE,
    already_running: bool,
}

impl SingleInstance {
    fn acquire(name: &str) -> Result<Self> {
        let full = HSTRING::from(format!("Local\\{name}"));
        unsafe { SetLastError(Default::default()) };
        let handle = unsafe { CreateMutexW(None, true, &full)? };
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        Ok(Self {
            handle,
            already_running,
        })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

fn instance_name(home: &Path) -> Result<String> {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let full = util::absolute(home)?;
    let mut hasher = Sha256::new();
    hasher.update(
        format!("{user}{}", full.to_string_lossy().to_uppercase()).as_bytes(),
    );
    let hash = hex::encode_upper(hasher.finalize());
    Ok(format!("ThodHUB.{}", &hash[..24]))
}

async fn send_instance_request(name: &str, requested: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match ClientOptions::new().pipe_mode(PipeMode::Byte).open(name) {
            Ok(mut client) => {
                client
                    .write_all(format!("{requested}\n").as_bytes())
                    .await?;
                client.flush().await?;
                return Ok(());
            }
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(error.into());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn listen_instance(name: String, proxy: EventLoopProxy<UserEvent>) {
    loop {
        let mut server = match ServerOptions::new().pipe_mode(PipeMode::Byte).create(&name) {
            Ok(server) => server,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        if server.connect().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let mut buffer = [0u8; 72];
        let mut length = 0usize;
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while length < buffer.len() {
                let mut one = [0u8; 1];
                match server.read(&mut one).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if one[0] == b'\n' {
                            break;
                        }
                        buffer[length] = one[0];
                        length += 1;
                    }
                }
            }
        })
        .await;
        let text = String::from_utf8_lossy(&buffer[..length])
            .trim_end_matches('\r')
            .to_string();
        if text.is_empty() {
            continue;
        }
        let id = text.strip_prefix("remove:").unwrap_or(&text);
        if contract::id(id).is_ok() {
            let _ = proxy.send_event(UserEvent::Open(text));
        }
    }
}

// ---------------------------------------------------------------------------
// --invoke (CopyAll helper)
// ---------------------------------------------------------------------------

fn invoke(id: &str, method: &str, path: &str) {
    let result = (|| -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let store = HubStore::new(util::installed_home())?;
            let manifest = store.manifest(id)?;
            let worker = manifest
                .worker
                .as_ref()
                .ok_or_else(|| anyhow!("Программа не поддерживает копирование из меню."))?;
            if !manifest
                .permissions
                .iter()
                .any(|permission| permission == "clipboard.write")
            {
                return Err(anyhow!("Программа не поддерживает копирование из меню."));
            }
            let root = store.app_version(id, &manifest.version)?;
            let executable = contract::inside(&root, &worker.executable)?;
            let process = process::ModuleProcess::start(
                &executable,
                &worker.arguments,
                &root,
                &store.data_directory(id)?,
            )?;
            let response = process
                .call(method, Some(json!({ "path": path })))
                .await;
            process.dispose().await;
            let response = response?;
            let text = response
                .get("clipboardText")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            util::copy_text(&text)?;
            if let Some(skipped) = response.get("skipped").and_then(|value| value.as_i64()) {
                if skipped > 0 {
                    util::message_box(
                        &format!(
                            "Пути скопированы. Пропущено недоступных папок или ссылок: {skipped}."
                        ),
                        "CopyAll",
                        false,
                        false,
                    );
                }
            }
            Ok(())
        })
    })();
    if let Err(error) = result {
        util::show_error(&format!("{error:#}"));
    }
}

fn hub_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|path| path.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}
