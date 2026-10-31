//! The CEF host shared by every Chromium draft (`crates/monocode-cef-host`),
//! started with the first tab and quit with the last. Commands go in on its
//! fd 3 and events come back on fd 4, each JSON message ending in a NUL.
//! Each draft is a browser there, driven over DevTools as a session would be
//! in a stand-alone Chromium, and its frames come as IOSurfaces over Mach.
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::fd::FromRawFd,
    path::Path,
    process::Stdio,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[path = "../../../monocode-cef-host/src/frame_port.rs"]
mod frame_port;

type Reply = async_channel::Sender<Result<Value, String>>;
type Sessions = Arc<Mutex<HashMap<String, async_channel::Sender<Value>>>>;
type Frames = Arc<Mutex<HashMap<u64, async_channel::Sender<Frame>>>>;

/// A frame of a draft: the host's IOSurface as a pixel buffer GPUI draws.
pub(super) struct Frame {
    pub buffer: CFRetained<CVPixelBuffer>,
    pub size: (u32, u32),
}
// SAFETY: a CVPixelBuffer may be passed between threads; it is only read.
unsafe impl Send for Frame {}

pub(super) struct Browser {
    write: Mutex<std::fs::File>,
    next: AtomicU64,
    next_tab: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    /// Tabs waiting for the host to open them.
    opening: Arc<Mutex<HashMap<u64, Reply>>>,
    /// Where each tab's events go, by its id as a string.
    sessions: Sessions,
    frames: Frames,
    child: Mutex<std::process::Child>,
    stopped: Arc<AtomicBool>,
    /// The last fatal line the host wrote, to say why it went away.
    last_error: Arc<Mutex<Option<String>>>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        // Closing the command pipe asks the host to close its browsers and
        // quit; it gets a moment before it is killed.
        if let Ok(mut write) = self.write.lock() {
            let mut quit = serde_json::to_vec(&json!({"quit": true})).unwrap_or_default();
            quit.push(0);
            let _ = write.write_all(&quit);
        }
        if let Ok(mut child) = self.child.lock() {
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

thread_local! {
    static SHARED: std::cell::RefCell<Weak<Browser>> = const { std::cell::RefCell::new(Weak::new()) };
}

impl Browser {
    /// The running host, started if no tab holds one.
    pub(super) fn shared(host: &Path) -> Result<Arc<Self>, String> {
        if let Some(browser) = SHARED.with(|shared| shared.borrow().upgrade())
            && browser.alive()
        {
            return Ok(browser);
        }
        let browser =
            Self::launch(host).map_err(|error| format!("Chromium could not start: {error}"))?;
        SHARED.with(|shared| *shared.borrow_mut() = Arc::downgrade(&browser));
        Ok(browser)
    }

    fn alive(&self) -> bool {
        self.child
            .lock()
            .is_ok_and(|mut child| matches!(child.try_wait(), Ok(None)))
    }

    fn launch(host: &Path) -> std::io::Result<Arc<Self>> {
        use std::os::unix::process::CommandExt;
        static LAUNCHES: AtomicU64 = AtomicU64::new(0);
        let receiver = frame_port::Receiver::new(&LAUNCHES.fetch_add(1, Ordering::Relaxed).to_string())
            .map_err(std::io::Error::other)?;
        let pipe = || -> std::io::Result<(i32, i32)> {
            let mut fds = [0; 2];
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            for fd in fds {
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            }
            Ok((fds[0], fds[1]))
        };
        let (commands_read, commands_write) = pipe()?;
        let (events_read, events_write) = pipe()?;
        let mut command = std::process::Command::new(host);
        command
            .env("MONOCODE_CEF_FRAMES", receiver.name())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // The pipe ends may already be fds 3 or 4, and dup2 onto itself keeps
        // close-on-exec; copying them out of the way first gives the host
        // fresh, inheritable 3 and 4, and it sees the pipe close when the app
        // goes.
        unsafe {
            command.pre_exec(move || {
                let commands = libc::fcntl(commands_read, libc::F_DUPFD, 10);
                let events = libc::fcntl(events_write, libc::F_DUPFD, 10);
                if commands < 0
                    || events < 0
                    || libc::dup2(commands, 3) < 0
                    || libc::dup2(events, 4) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
        };
        let child = command.spawn();
        unsafe {
            libc::close(commands_read);
            libc::close(events_write);
        }
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                unsafe {
                    libc::close(commands_write);
                    libc::close(events_read);
                }
                return Err(error);
            }
        };
        let last_error = Arc::<Mutex<Option<String>>>::default();
        if let Some(stderr) = child.stderr.take() {
            let last_error = last_error.clone();
            std::thread::Builder::new()
                .name("cef-host-log".into())
                .spawn(move || read_log(stderr, &last_error))?;
        }
        let write = unsafe { std::fs::File::from_raw_fd(commands_write) };
        let read = unsafe { std::fs::File::from_raw_fd(events_read) };
        let browser = Arc::new(Self {
            write: Mutex::new(write),
            next: AtomicU64::new(0),
            next_tab: AtomicU64::new(0),
            pending: Arc::default(),
            opening: Arc::default(),
            sessions: Arc::default(),
            frames: Arc::default(),
            child: Mutex::new(child),
            stopped: Arc::default(),
            last_error,
        });
        let routes = Routes {
            pending: browser.pending.clone(),
            opening: browser.opening.clone(),
            sessions: browser.sessions.clone(),
        };
        std::thread::Builder::new()
            .name("cef-host-events".into())
            .spawn(move || read_messages(read, routes))?;
        let (frames, stopped) = (browser.frames.clone(), browser.stopped.clone());
        std::thread::Builder::new()
            .name("cef-host-frames".into())
            .spawn(move || receive_frames(receiver, frames, stopped))?;
        Ok(browser)
    }

    fn command(&self, message: &Value) {
        let mut bytes = serde_json::to_vec(message).unwrap_or_default();
        bytes.push(0);
        let _ = self.write.lock().unwrap().write_all(&bytes);
    }

    /// Opens a tab `width` × `height` points at `scale`; resolves to its id.
    pub(super) async fn open_tab(&self, width: u32, height: u32, scale: f32) -> Result<u64, String> {
        let id = self.next_tab.fetch_add(1, Ordering::Relaxed) + 1;
        let (reply, receiver) = async_channel::bounded(1);
        self.opening.lock().unwrap().insert(id, reply);
        self.command(&json!({"open": {
            "id": id, "width": width.max(1), "height": height.max(1), "scale": scale,
        }}));
        receiver
            .recv()
            .await
            .map_err(|_| self.went_away())??;
        Ok(id)
    }

    /// Says that the host quit, and why if it said so.
    pub(super) fn went_away(&self) -> String {
        match self.last_error.lock().ok().and_then(|last| last.clone()) {
            Some(error) => format!("Chromium quit: {error}"),
            None => "Chromium quit".into(),
        }
    }

    pub(super) fn close_tab(&self, id: u64) {
        self.forget(&id.to_string());
        self.frames.lock().unwrap().remove(&id);
        self.command(&json!({ "close": id }));
    }

    /// Lays a tab out at `width` × `height` points, drawn at `scale`.
    pub(super) fn resize(&self, id: u64, width: u32, height: u32, scale: f32) {
        self.command(&json!({"resize": {
            "id": id, "width": width.max(1), "height": height.max(1), "scale": scale,
        }}));
    }

    /// A hidden tab draws nothing and its page runs as a background tab.
    pub(super) fn set_hidden(&self, id: u64, hidden: bool) {
        self.command(&json!({"hidden": {"id": id, "hidden": hidden}}));
        if !hidden {
            self.command(&json!({ "invalidate": id }));
        }
    }

    pub(super) fn set_focus(&self, id: u64, focus: bool) {
        self.command(&json!({"focus": {"id": id, "focus": focus}}));
    }

    /// Where a tab's frames go.
    pub(super) fn watch_frames(&self, id: u64, frames: async_channel::Sender<Frame>) {
        self.frames.lock().unwrap().insert(id, frames);
    }

    /// Sends a DevTools command to the tab `session` names; the reply arrives
    /// on the receiver, which may be dropped.
    pub(super) fn send(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> async_channel::Receiver<Result<Value, String>> {
        let (reply, receiver) = async_channel::bounded(1);
        let Some(tab) = session.and_then(|session| session.parse::<u64>().ok()) else {
            let _ = reply.try_send(Err(format!("{method}: no tab")));
            return receiver;
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.pending.lock().unwrap().insert(id, reply);
        self.command(&json!({"cdp": {
            "id": tab,
            "message": {"id": id, "method": method, "params": params},
        }}));
        receiver
    }

    pub(super) async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        self.send(method, params, session)
            .recv()
            .await
            .map_err(|_| format!("{method}: Chromium went away"))?
            .map_err(|error| format!("{method}: {error}"))
    }

    /// Routes a tab's events to `events` until it is closed.
    pub(super) fn listen(&self, session: &str, events: async_channel::Sender<Value>) {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.to_owned(), events);
    }

    pub(super) fn forget(&self, session: &str) {
        self.sessions.lock().unwrap().remove(session);
    }
}

/// Keeps the last fatal line of the host's log: a Chromium `FATAL` message,
/// or what a panic said. Debug builds pass the whole log on.
fn read_log(stderr: std::process::ChildStderr, last_error: &Mutex<Option<String>>) {
    use std::io::BufRead;
    let mut after_panic = false;
    for line in std::io::BufReader::new(stderr).lines() {
        let Ok(line) = line else {
            break;
        };
        if cfg!(debug_assertions) {
            eprintln!("{line}");
        }
        if let Some(error) = fatal_line(&line, after_panic)
            && let Ok(mut last) = last_error.lock()
        {
            *last = Some(error);
        }
        after_panic = line.contains("panicked at");
    }
}

/// The reason a log line gives for quitting, if it gives one.
fn fatal_line(line: &str, after_panic: bool) -> Option<String> {
    if let Some((_, message)) = line.split_once(":FATAL:") {
        // `[pid:tid:time:FATAL:file.cc:389] GPU process isn't usable.`
        let message = message.split_once("] ").map_or(message, |(_, text)| text);
        return Some(message.trim().to_owned());
    }
    (after_panic && !line.trim().is_empty()).then(|| line.trim().to_owned())
}

struct Routes {
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    opening: Arc<Mutex<HashMap<u64, Reply>>>,
    sessions: Sessions,
}

impl Routes {
    fn to_tab(&self, tab: u64, mut event: Value) {
        let session = tab.to_string();
        event["sessionId"] = session.clone().into();
        let events = self.sessions.lock().unwrap().get(&session).cloned();
        if let Some(events) = events {
            let _ = events.send_blocking(event);
        }
    }
}

/// Reads the host's messages until the pipe closes: DevTools replies go to
/// the waiting command, a tab's events to its session.
fn read_messages(mut read: std::fs::File, routes: Routes) {
    let mut buffer = Vec::new();
    let mut chunk = vec![0; 1 << 20];
    let mut scanned = 0;
    loop {
        let n = match read.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buffer.extend_from_slice(&chunk[..n]);
        while let Some(end) = buffer[scanned..].iter().position(|b| *b == 0) {
            let end = scanned + end;
            let message = serde_json::from_slice::<Value>(&buffer[..end]);
            buffer.drain(..=end);
            scanned = 0;
            let Ok(message) = message else { continue };
            route(&routes, message);
        }
        scanned = buffer.len();
    }
    // Dropping the waiting replies fails their calls; dropping the senders
    // ends each tab's event loop.
    routes.pending.lock().unwrap().clear();
    routes.opening.lock().unwrap().clear();
    routes.sessions.lock().unwrap().clear();
}

fn route(routes: &Routes, message: Value) {
    if let Some(cdp) = message.get("cdp") {
        let (Some(tab), message) = (cdp["id"].as_u64(), &cdp["message"]) else {
            return;
        };
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            if let Some(reply) = routes.pending.lock().unwrap().remove(&id) {
                let result = match message.get("error") {
                    Some(error) => Err(error["message"]
                        .as_str()
                        .map_or_else(|| error.to_string(), str::to_owned)),
                    None => Ok(message.get("result").cloned().unwrap_or_default()),
                };
                let _ = reply.try_send(result);
            }
        } else {
            routes.to_tab(tab, message.clone());
        }
    } else if let Some(tab) = message["opened"].as_u64() {
        if let Some(reply) = routes.opening.lock().unwrap().remove(&tab) {
            let _ = reply.try_send(Ok(Value::Null));
        }
    } else if let Some(tab) = message["failed"].as_u64() {
        if let Some(reply) = routes.opening.lock().unwrap().remove(&tab) {
            let error = message["error"].as_str().unwrap_or("Chromium could not open a tab");
            let _ = reply.try_send(Err(error.to_owned()));
        }
    } else if let Some(popup) = message.get("popup") {
        if let Some(tab) = popup["id"].as_u64() {
            routes.to_tab(
                tab,
                json!({"method": "Host.popup", "params": {
                    "url": popup["url"], "userGesture": popup["userGesture"],
                }}),
            );
        }
    } else if let Some(tab) = message["closed"].as_u64() {
        routes.sessions.lock().unwrap().remove(&tab.to_string());
    }
}

/// Receives the host's frames until it is stopped and hands each to its tab.
fn receive_frames(receiver: frame_port::Receiver, frames: Frames, stopped: Arc<AtomicBool>) {
    use objc2_io_surface::IOSurfaceRef;
    while !stopped.load(Ordering::Relaxed) {
        let Some(frame) = receiver.recv(Duration::from_millis(500)) else {
            continue;
        };
        let surface = IOSurfaceRef::lookup_from_mach_port(frame.surface);
        frame_port::release(frame.surface);
        let Some(surface) = surface else {
            continue;
        };
        let target = frames.lock().unwrap().get(&frame.browser).cloned();
        let Some(target) = target else {
            continue;
        };
        let mut buffer = std::ptr::null_mut();
        let created = unsafe {
            objc2_core_video::CVPixelBufferCreateWithIOSurface(
                None,
                &surface,
                None,
                std::ptr::NonNull::from(&mut buffer),
            )
        };
        let Some(buffer) = std::ptr::NonNull::new(buffer)
            .filter(|_| created == objc2_core_video::kCVReturnSuccess)
        else {
            continue;
        };
        let _ = target.try_send(Frame {
            buffer: unsafe { CFRetained::from_raw(buffer) },
            size: (frame.width, frame.height),
        });
    }
}

/// Waits for a reply at most `limit`.
pub(super) async fn within(
    reply: async_channel::Receiver<Result<Value, String>>,
    limit: Duration,
    cx: &gpui::AsyncApp,
) -> Option<Value> {
    let (send, receive) = async_channel::bounded(2);
    let deadline = send.clone();
    let timer = cx.background_executor().timer(limit);
    cx.background_executor()
        .spawn(async move {
            timer.await;
            let _ = deadline.try_send(None);
        })
        .detach();
    cx.background_executor()
        .spawn(async move {
            let _ = send.try_send(reply.recv().await.ok().and_then(Result::ok));
        })
        .detach();
    receive.recv().await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::fatal_line;

    #[test]
    fn the_hosts_log_says_why_it_quit() {
        assert_eq!(
            fatal_line(
                "[26354:5601075:1031/013143.911545:FATAL:content/browser/gpu/gpu_data_manager_impl_private.cc:389] GPU process isn't usable. Goodbye.",
                false
            )
            .as_deref(),
            Some("GPU process isn't usable. Goodbye.")
        );
        assert_eq!(
            fatal_line("called `Result::unwrap()` on an `Err` value: NotFound", true).as_deref(),
            Some("called `Result::unwrap()` on an `Err` value: NotFound")
        );
        assert_eq!(fatal_line("[1:2:3:ERROR:x.cc:1] Not fatal", false), None);
        assert_eq!(fatal_line("note: run with `RUST_BACKTRACE=1`", false), None);
    }
}
