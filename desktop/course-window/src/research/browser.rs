use super::{
    google,
    notebook::{self, Notebook},
};
use serde_json::{json, Value};
use std::{cell::Cell, error::Error, path::PathBuf, rc::Rc, thread};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use wry::{
    NewWindowResponse, PermissionResponse, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

enum Action {
    Control(String),
    OpenLink(String),
    Loaded(u64, bool),
    Metadata(u64, String, String),
    Account(Result<String, &'static str>),
    Location,
    ParentClosed,
}
const CONTROLS: &str = "http://studycontrols.localhost/toolbar";
const START: &str = "http://studyhome.localhost/start";

fn bundled_html(
    path: &str,
    expected: &str,
    bytes: &'static [u8],
) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    let found = path == expected;
    wry::http::Response::builder()
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .status(if found { 200 } else { 404 })
        .body(std::borrow::Cow::Borrowed(if found { bytes } else { b"" }))
        .unwrap()
}

fn bounds(y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(0., y).into(),
        size: LogicalSize::new(w.max(1.), h.max(1.)).into(),
    }
}
// This script runs only on supported public article paths. It never inspects input values,
// selects text, attaches event listeners, or walks the full document text.
const METADATA: &str = r#"(() => {
  if (document.querySelector('input[type=password]')) return null;
  const root=document.querySelector('main,article,[role=main]') || document.body;
  if (!root) return null;
  const headings=[];
  for (const h of root.querySelectorAll('h1,h2')) {
    if (headings.length===6) break;
    if (h.checkVisibility({checkVisibilityCSS:true}) && !h.closest('form,[contenteditable=true]')) {
      const text=h.innerText.trim().slice(0,100);
      if (text) headings.push(text);
    }
  }
  return {href:location.href,title:document.title.slice(0,160),headings};
})()"#;

fn collect(
    view: &WebView,
    book: &mut Notebook,
    proxy: &EventLoopProxy<Action>,
    epoch: u64,
    raw: String,
) -> Result<(), Box<dyn Error>> {
    if view.url()? != raw {
        return Ok(());
    }
    let Some(source) = notebook::source(&raw) else {
        return Ok(());
    };
    if source.article {
        let tx = proxy.clone();
        view.evaluate_script_with_callback(METADATA, move |value| {
            let _ = tx.send_event(Action::Metadata(epoch, raw.clone(), value));
        })?;
    } else {
        book.record(&notebook::observation(&source, &Value::Null))?;
    }
    Ok(())
}

fn version(text: &str) -> Option<[u32; 4]> {
    let parts: Vec<_> = text
        .split_whitespace()
        .next()?
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<_, _>>()
        .ok()?;
    parts.try_into().ok()
}
pub fn run() -> Result<(), Box<dyn Error>> {
    let marker = std::env::current_exe()?.with_file_name("research-runtime.txt");
    if marker.is_file() && std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none() {
        let pinned = PathBuf::from(std::fs::read_to_string(marker)?);
        let bundled = pinned
            .file_name()
            .and_then(|v| v.to_str())
            .and_then(version);
        let evergreen = wry::webview_version().ok().as_deref().and_then(version);
        if bundled > evergreen {
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", pinned);
        }
    }
    let home = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Missing lesson research workspace")?,
    );
    let mut book = Notebook::open(&home.join("sources.sqlite3"))?;
    let events = EventLoopBuilder::<Action>::with_user_event().build();
    let proxy = events.create_proxy();
    if std::env::args_os().nth(2).as_deref() == Some(std::ffi::OsStr::new("--parent-pipe")) {
        let tx = proxy.clone();
        thread::spawn(move || {
            // Closing or crashing the owning desktop also closes this lesson's research path.
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = tx.send_event(Action::ParentClosed);
        });
    }
    let window = WindowBuilder::new()
        .with_title("Robotutor · Research browser")
        .with_inner_size(LogicalSize::new(1040., 750.))
        .with_min_inner_size(LogicalSize::new(520., 440.))
        .build(&events)?;
    let mut context = WebContext::new(Some(home.join("webview")));
    let toolbar_tx = proxy.clone();
    let toolbar = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("studycontrols".into(), |_, req| {
            bundled_html(req.uri().path(), "/toolbar", include_bytes!("toolbar.html"))
        })
        .with_url("studycontrols://localhost/toolbar")
        .with_bounds(bounds(0., 1040., 122.))
        .with_navigation_handler(|url| url == CONTROLS)
        .with_devtools(false)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |req| {
            if req.uri().to_string() == CONTROLS && req.body().len() < 8192 {
                let _ = toolbar_tx.send_event(Action::Control(req.into_body()));
            }
        })
        .build_as_child(&window)?;
    let epoch = Rc::new(Cell::new(0_u64));
    let nav_epoch = epoch.clone();
    let load_epoch = epoch.clone();
    let ready = Rc::new(Cell::new(false));
    let nav_ready = ready.clone();
    let load_tx = proxy.clone();
    let nav_tx = proxy.clone();
    let web = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("studyhome".into(), |_, req| {
            bundled_html(req.uri().path(), "/start", include_bytes!("welcome.html"))
        })
        .with_url("studyhome://localhost/start")
        .with_bounds(bounds(122., 1040., 628.))
        .with_incognito(true)
        .with_devtools(false)
        .with_general_autofill_enabled(false)
        .with_clipboard(false)
        .with_permission_handler(|_| PermissionResponse::Deny)
        .with_browser_accelerator_keys(false)
        .with_navigation_handler(move |url| {
            let allowed = url == START || notebook::internet(&url).is_some();
            if allowed {
                nav_ready.set(false);
                nav_epoch.set(nav_epoch.get().wrapping_add(1));
                let _ = nav_tx.send_event(Action::Location);
            }
            allowed
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .build_as_child(&window)?;
    // Wry's generic finished event includes failures. Only the latest successful
    // main-frame navigation is eligible to supply pedagogical evidence.
    let nav_id = Rc::new(Cell::new(0));
    let latest = nav_id.clone();
    unsafe {
        let core = web.controller().CoreWebView2()?;
        let mut token = 0;
        let link_tx = proxy.clone();
        core.add_NewWindowRequested(
            &webview2_com::NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                    let mut clicked = Default::default();
                    args.IsUserInitiated(&mut clicked)?;
                    if clicked.as_bool() {
                        let mut uri = Default::default();
                        args.Uri(&mut uri)?;
                        let url = webview2_com::take_pwstr(uri);
                        if notebook::internet(&url).is_some() {
                            let _ = link_tx.send_event(Action::OpenLink(url));
                        }
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        core.add_NavigationStarting(
            &webview2_com::NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut id = 0;
                    args.NavigationId(&mut id)?;
                    latest.set(id);
                }
                Ok(())
            })),
            &mut token,
        )?;
        core.add_NavigationCompleted(
            &webview2_com::NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut id = 0;
                    args.NavigationId(&mut id)?;
                    if id == nav_id.get() {
                        let mut ok = Default::default();
                        args.IsSuccess(&mut ok)?;
                        let _ = load_tx.send_event(Action::Loaded(load_epoch.get(), ok.as_bool()));
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    let mut recording = false;
    let mut expanded = false;
    let mut toolbar_height = 104.0_f64;
    let mut login_pending = false;
    let mut account = None::<String>;
    let mut message = String::new();
    let runtime = unsafe {
        let mut name = Default::default();
        web.environment().BrowserVersionString(&mut name)?;
        webview2_com::take_pwstr(name)
    };
    events.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let result = (|| -> Result<(), Box<dyn Error>> {
            match event {
                Event::UserEvent(Action::ParentClosed) | Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                    *flow = ControlFlow::Exit;
                    return Ok(());
                }
                Event::UserEvent(Action::OpenLink(url)) => {
                    expanded=false;
                    message.clear();
                    web.load_url(&url)?;
                }
                Event::UserEvent(Action::Control(body)) => {
                    let req: Value = serde_json::from_str(&body)?;
                    match req["action"].as_str().unwrap_or("") {
                        "go" => {
                            if let Some(url) = notebook::destination(req["value"].as_str().unwrap_or("")) {
                                web.load_url(&url)?;
                                expanded = false;
                                message.clear();
                            } else {
                                message = "Enter a search or public HTTPS address.".into();
                            }
                        }
                        "back" if web.can_go_back()? => { expanded=false; web.go_back()?; }
                        "forward" if web.can_go_forward()? => { expanded=false; web.go_forward()?; }
                        "reload" => { expanded=false; web.reload()?; }
                        "toggle" => {
                            epoch.set(epoch.get().wrapping_add(1));
                            recording = !recording;
                            message.clear();
                            if recording && ready.get() {
                                collect(&web, &mut book, &proxy, epoch.get(), web.url()?)?;
                            }
                        }
                        "height" => {
                            if let Some(pixels) = req["value"].as_f64() {
                                toolbar_height = pixels.clamp(84.0, 240.0);
                            }
                        }
                        "notebook" => expanded = !expanded,
                        "clear" => {
                            epoch.set(epoch.get().wrapping_add(1));
                            recording = false;
                            book.clear()?;
                            message = "Notebook cleared. Recording is paused.".into();
                        }
                        "google" if !login_pending => {
                            if account.take().is_some() {
                                message = "Signed out of this research window.".into();
                            } else {
                                login_pending = true;
                                message = "Waiting for Google in your usual browser...".into();
                                let tx = proxy.clone();
                                thread::spawn(move || { let _ = tx.send_event(Action::Account(google::sign_in())); });
                            }
                        }
                        _ => {}
                    }
                }
                Event::UserEvent(Action::Account(result)) => {
                    login_pending = false;
                    match result {
                        Ok(email) => { account = Some(email); message.clear(); }
                        Err(msg) => message = msg.into(),
                    }
                }
                Event::UserEvent(Action::Loaded(n, success)) if n == epoch.get() => {
                    ready.set(success);
                    if success && recording {
                        collect(&web, &mut book, &proxy, n, web.url()?)?;
                    } else if !success {
                        message = "The page could not be loaded.".into();
                    }
                }
                Event::UserEvent(Action::Metadata(n, url, raw)) if recording && n == epoch.get() && web.url()? == url => {
                    let val: Value = serde_json::from_str(&raw)?;
                    if val["href"].as_str() == Some(&url) {
                        if let Some(source) = notebook::source(&url) {
                            book.record(&notebook::observation(&source, &val))?;
                        }
                    }
                }
                Event::UserEvent(_) | Event::WindowEvent { event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }, .. }
                    | Event::NewEvents(tao::event::StartCause::Init) => {}
                _ => return Ok(()),
            }
            let size: LogicalSize<f64> = window.inner_size().to_logical(window.scale_factor());
            let height = if expanded { size.height } else { toolbar_height };
            toolbar.set_bounds(bounds(0., size.width, height))?;
            web.set_visible(!expanded)?;
            if !expanded { web.set_bounds(bounds(height, size.width, size.height-height))?; }
            let rows = if expanded { book.recent()? } else { vec![] };
            let address = web.url()?;
            let data = json!({"recording":recording,"expanded":expanded,"rows":rows,
                "url":if address == START { "" } else { &address },"account":account,"waiting":login_pending,"message":message,"runtime":runtime});
            toolbar.evaluate_script(&format!("window.render && window.render({data})"))?;
            Ok(())
        })();
        if result.is_err() {
            recording = false;
            epoch.set(epoch.get().wrapping_add(1));
            let _ = toolbar.evaluate_script("window.showError && window.showError('Research action failed. Recording paused.')");
        }
    })
}
