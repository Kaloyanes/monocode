//! MonoCode's CEF host: renders drafts off screen in its own process, so the
//! app keeps its strict code signing while this one loads the downloaded,
//! unsigned Chromium Embedded Framework.
//!
//! The app starts it with two pipes: commands come in on fd 3 and events go
//! out on fd 4, each a JSON message ending in a NUL, as over Chromium's
//! DevTools pipe. Each draft is a browser here, driven over DevTools
//! (`send_dev_tools_message`), and its frames go to the app as IOSurfaces over
//! Mach (see `frame_port`), named by `MONOCODE_CEF_FRAMES`.
//!
//! The same binary is the helper app CEF starts for its renderer, GPU and
//! utility processes; those run `execute_process` and exit.
mod frame_port;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod surfaces;

use cef::{args::Args, *};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{Read, Write},
    os::fd::FromRawFd,
    process::ExitCode,
    rc::Rc as StdRc,
    sync::{Mutex, OnceLock},
};

const HELPER: &str = "MonoCode CEF Helper";

static EVENTS: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

/// Sends an event to the app.
fn emit(message: &Value) {
    let Some(events) = EVENTS.get() else {
        return;
    };
    let mut bytes = serde_json::to_vec(message).unwrap_or_default();
    bytes.push(0);
    if let Ok(mut events) = events.lock() {
        let _ = events.write_all(&bytes);
    }
}

struct Open {
    browser: Browser,
    size: StdRc<Cell<(i32, i32)>>,
    scale: StdRc<Cell<f32>>,
    _observer: Option<Registration>,
}

thread_local! {
    static BROWSERS: RefCell<HashMap<u64, Open>> = RefCell::new(HashMap::new());
    #[cfg(target_os = "macos")]
    static COPIER: RefCell<Option<surfaces::Copier>> = const { RefCell::new(None) };
    static FRAMES: RefCell<Option<frame_port::Sender>> = const { RefCell::new(None) };
    static QUITTING: Cell<bool> = const { Cell::new(false) };
}

fn main() -> ExitCode {
    // CEF's own command line parsing needs the framework loaded, and where
    // the framework is depends on whether this is the helper.
    let browser_process = !std::env::args().any(|arg| arg.starts_with("--type="));
    #[cfg(target_os = "macos")]
    if browser_process {
        mac::setup();
    }
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    #[cfg(target_os = "macos")]
    let _library = {
        let loader = library_loader::LibraryLoader::new(&exe, !browser_process);
        if !loader.load() {
            eprintln!("monocode-cef-host: could not load the Chromium Embedded Framework");
            return ExitCode::FAILURE;
        }
        loader
    };
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();

    let mut app = HostApp::new();
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if !browser_process {
        return ExitCode::from(code.clamp(0, 255) as u8);
    }

    // SAFETY: the app hands these two pipe ends to this process only.
    let commands = unsafe { std::fs::File::from_raw_fd(3) };
    let events = unsafe { std::fs::File::from_raw_fd(4) };
    let _ = EVENTS.set(Mutex::new(events));

    let contents = exe
        .parent()
        .and_then(|macos| macos.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let helper = contents
        .join("Frameworks")
        .join(format!("{HELPER}.app/Contents/MacOS/{HELPER}"));
    let cache = std::env::temp_dir().join(format!("monocode-cef-{}", std::process::id()));
    let settings = Settings {
        no_sandbox: 1,
        windowless_rendering_enabled: 1,
        browser_subprocess_path: helper.to_string_lossy().as_ref().into(),
        main_bundle_path: contents
            .parent()
            .map(|bundle| bundle.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_str()
            .into(),
        root_cache_path: cache.to_string_lossy().as_ref().into(),
        persist_session_cookies: 0,
        log_severity: LogSeverity::ERROR,
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        eprintln!("monocode-cef-host: CEF did not start");
        return ExitCode::FAILURE;
    }

    #[cfg(target_os = "macos")]
    COPIER.with_borrow_mut(|copier| *copier = surfaces::Copier::new());
    if let Ok(name) = std::env::var("MONOCODE_CEF_FRAMES") {
        match frame_port::Sender::connect(&name) {
            Ok(sender) => FRAMES.with_borrow_mut(|frames| *frames = Some(sender)),
            Err(error) => eprintln!("monocode-cef-host: {error}"),
        }
    }
    std::thread::Builder::new()
        .name("monocode-cef-commands".into())
        .spawn(move || read_commands(commands))
        .expect("the command thread starts");
    emit(&json!({"ready": true, "version": cef_version()}));

    run_message_loop();
    BROWSERS.with_borrow_mut(HashMap::clear);
    shutdown();
    let _ = std::fs::remove_dir_all(cache);
    ExitCode::SUCCESS
}

fn cef_version() -> String {
    format!(
        "{}.{}.{}",
        sys::CEF_VERSION_MAJOR,
        sys::CEF_VERSION_MINOR,
        sys::CEF_VERSION_PATCH
    )
}

/// Reads the app's commands until the pipe closes, which means the app is
/// gone or wants this process to quit.
fn read_commands(mut commands: std::fs::File) {
    let mut buffer = Vec::new();
    let mut chunk = vec![0; 1 << 16];
    loop {
        let n = match commands.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buffer.extend_from_slice(&chunk[..n]);
        while let Some(end) = buffer.iter().position(|b| *b == 0) {
            let message = serde_json::from_slice::<Value>(&buffer[..end]);
            buffer.drain(..=end);
            if let Ok(message) = message {
                post_task(ThreadId::UI, Some(&mut CommandTask::new(message)));
            }
        }
    }
    post_task(ThreadId::UI, Some(&mut CommandTask::new(json!({"quit": true}))));
}

wrap_task! {
    struct CommandTask {
        command: Value,
    }

    impl Task {
        fn execute(&self) {
            handle(&self.command);
        }
    }
}

fn id_of(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value["id"].as_u64())
}

fn with_host(id: u64, act: impl FnOnce(&BrowserHost, &Open)) {
    BROWSERS.with_borrow(|browsers| {
        if let Some(open) = browsers.get(&id)
            && let Some(host) = open.browser.host()
        {
            act(&host, open);
        }
    });
}

/// Runs one command from the app on the UI thread.
fn handle(command: &Value) {
    if let Some(open) = command.get("open") {
        open_browser(open);
    } else if let Some(id) = command.get("close").and_then(id_of) {
        with_host(id, |host, _| host.close_browser(1));
    } else if let Some(resize) = command.get("resize") {
        let size = (
            resize["width"].as_i64().unwrap_or(1).max(1) as i32,
            resize["height"].as_i64().unwrap_or(1).max(1) as i32,
        );
        let scale = resize["scale"].as_f64().map(|scale| scale as f32);
        if let Some(id) = id_of(resize) {
            with_host(id, |host, open| {
                if let Some(scale) = scale.filter(|scale| *scale != open.scale.get()) {
                    open.scale.set(scale);
                    host.notify_screen_info_changed();
                }
                open.size.set(size);
                host.was_resized();
            });
        }
    } else if let Some(hidden) = command.get("hidden") {
        if let Some(id) = id_of(hidden) {
            let on = hidden["hidden"].as_bool().unwrap_or(true);
            with_host(id, |host, _| host.was_hidden(on as i32));
        }
    } else if let Some(focus) = command.get("focus") {
        if let Some(id) = id_of(focus) {
            let on = focus["focus"].as_bool().unwrap_or(false);
            with_host(id, |host, _| host.set_focus(on as i32));
        }
    } else if let Some(invalidate) = command.get("invalidate").and_then(id_of) {
        with_host(invalidate, |host, _| host.invalidate(PaintElementType::VIEW));
    } else if let Some(cdp) = command.get("cdp") {
        if let Some(id) = id_of(cdp) {
            let message = serde_json::to_vec(&cdp["message"]).unwrap_or_default();
            with_host(id, |host, _| {
                host.send_dev_tools_message(Some(&message));
            });
        }
    } else if command.get("quit").is_some() {
        quit();
    }
}

/// Closes every browser, then ends the message loop once the last is gone.
fn quit() {
    QUITTING.set(true);
    let open: Vec<Browser> =
        BROWSERS.with_borrow(|browsers| browsers.values().map(|open| open.browser.clone()).collect());
    if open.is_empty() {
        quit_message_loop();
        return;
    }
    for browser in open {
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }
}

fn open_browser(open: &Value) {
    let Some(id) = id_of(open) else {
        return;
    };
    let size = StdRc::new(Cell::new((
        open["width"].as_i64().unwrap_or(800).max(1) as i32,
        open["height"].as_i64().unwrap_or(600).max(1) as i32,
    )));
    let scale = StdRc::new(Cell::new(open["scale"].as_f64().unwrap_or(2.0) as f32));
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: 1,
        ..Default::default()
    };
    let settings = BrowserSettings {
        windowless_frame_rate: open["fps"].as_i64().unwrap_or(60) as i32,
        // Transparent, so a page without a background shows the card's.
        background_color: 0,
        ..Default::default()
    };
    // Each draft gets its own storage, kept in memory only.
    let mut context = request_context_create_context(Some(&RequestContextSettings::default()), None);
    let mut client = HostClient::new(
        HostRenderHandler::new(id, size.clone(), scale.clone()),
        HostLifeSpanHandler::new(id),
        HostJsdialogHandler::new(),
        HostDialogHandler::new(),
        HostDownloadHandler::new(),
    );
    let browser = browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut client),
        Some(&CefString::from("about:blank")),
        Some(&settings),
        None,
        context.as_mut(),
    );
    let Some(browser) = browser else {
        emit(&json!({"failed": id, "error": "CEF could not open a browser"}));
        return;
    };
    let observer = browser.host().and_then(|host| {
        host.add_dev_tools_message_observer(Some(&mut HostDevToolsObserver::new(id)))
    });
    BROWSERS.with_borrow_mut(|browsers| {
        browsers.insert(
            id,
            Open {
                browser,
                size,
                scale,
                _observer: observer,
            },
        )
    });
    emit(&json!({"opened": id}));
}

wrap_app! {
    struct HostApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };
            if process_type.is_some_and(|kind| !kind.to_string().is_empty()) {
                return;
            }
            for switch in [
                // Never ask for the login keychain.
                "use-mock-keychain",
                "disable-background-networking",
                "disable-component-update",
                "disable-sync",
                "disable-extensions",
                "no-first-run",
                "no-default-browser-check",
                "hide-scrollbars",
                "mute-audio",
                "noerrdialogs",
            ] {
                command_line.append_switch(Some(&CefString::from(switch)));
            }
        }
    }
}

wrap_render_handler! {
    struct HostRenderHandler {
        id: u64,
        size: StdRc<Cell<(i32, i32)>>,
        scale: StdRc<Cell<f32>>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (width, height) = self.size.get();
                rect.x = 0;
                rect.y = 0;
                rect.width = width;
                rect.height = height;
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(screen_info) = screen_info else {
                return 0;
            };
            screen_info.device_scale_factor = self.scale.get();
            1
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            _type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            _buffer: *const u8,
            _width: ::std::os::raw::c_int,
            _height: ::std::os::raw::c_int,
        ) {
        }

        fn on_accelerated_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            if type_ != PaintElementType::VIEW {
                return;
            }
            let Some(info) = info else {
                return;
            };
            #[cfg(target_os = "macos")]
            send_frame(self.id, info.shared_texture_io_surface);
        }
    }
}

#[cfg(target_os = "macos")]
fn send_frame(id: u64, surface: *mut std::ffi::c_void) {
    let Some(surface) = std::ptr::NonNull::new(surface.cast::<objc2_io_surface::IOSurfaceRef>())
    else {
        return;
    };
    // SAFETY: CEF keeps the surface alive for the length of this callback.
    let surface = unsafe { surface.as_ref() };
    COPIER.with_borrow_mut(|copier| {
        let Some(own) = copier.as_mut().and_then(|copier| copier.copy(id, surface)) else {
            return;
        };
        let (width, height) = (own.width() as u32, own.height() as u32);
        let port = own.create_mach_port();
        FRAMES.with_borrow(|frames| match frames {
            Some(frames) => {
                frames.send(id, port, width, height);
            }
            None => frame_port::release(port),
        });
    });
}

wrap_dev_tools_message_observer! {
    struct HostDevToolsObserver {
        id: u64,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            if let Some(message) = message.and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok()) {
                emit(&json!({"cdp": {"id": self.id, "message": message}}));
            }
            1
        }
    }
}

wrap_life_span_handler! {
    struct HostLifeSpanHandler {
        id: u64,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // No windows here. A link the person opened goes to the app,
            // which decides whether to hand it to their browser.
            let url = target_url.map(|url| url.to_string()).unwrap_or_default();
            emit(&json!({"popup": {"id": self.id, "url": url, "userGesture": user_gesture == 1}}));
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let id = self.id;
            BROWSERS.with_borrow_mut(|browsers| browsers.remove(&id));
            #[cfg(target_os = "macos")]
            COPIER.with_borrow_mut(|copier| {
                if let Some(copier) = copier.as_mut() {
                    copier.forget(id);
                }
            });
            emit(&json!({"closed": id}));
            if QUITTING.get() && BROWSERS.with_borrow(HashMap::is_empty) {
                quit_message_loop();
            }
        }
    }
}

wrap_jsdialog_handler! {
    struct HostJsdialogHandler;

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            _origin_url: Option<&CefString>,
            _dialog_type: JsdialogType,
            _message_text: Option<&CefString>,
            _default_prompt_text: Option<&CefString>,
            _callback: Option<&mut JsdialogCallback>,
            suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // Alerts, confirms and prompts are dismissed, as in a headless
            // browser.
            if let Some(suppress) = suppress_message {
                *suppress = 1;
            }
            0
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            if let Some(callback) = callback {
                callback.cont(1, None);
            }
            1
        }
    }
}

wrap_dialog_handler! {
    struct HostDialogHandler;

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _mode: FileDialogMode,
            _title: Option<&CefString>,
            _default_file_path: Option<&CefString>,
            _accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            // A draft may not read the person's files.
            if let Some(callback) = callback {
                callback.cancel();
            }
            1
        }
    }
}

wrap_download_handler! {
    struct HostDownloadHandler;

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            0
        }
    }
}

wrap_client! {
    struct HostClient {
        render: RenderHandler,
        life_span: LifeSpanHandler,
        jsdialog: JsdialogHandler,
        dialog: DialogHandler,
        download: DownloadHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.jsdialog.clone())
        }
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialog.clone())
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.download.clone())
        }
    }
}
