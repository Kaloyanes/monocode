//! One device's live connection to the hub, all off the main thread: video
//! is demuxed here and handed straight to the screen's decoder, and input
//! goes out the moment it arrives. The app hears only about state changes
//! (connected, picture size, input ready), never about frames.
use super::http;
use monocode_core::device::{
    Platform,
    hub::{self, Endpoint},
    protocol::{
        self, Button, Chunk, HingeResult, Message, Modifiers, Orientation, ParameterSets, Phase,
        Pose, ScreenConfig,
    },
};
use std::{
    os::fd::AsRawFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tungstenite::{Message as WsMessage, WebSocket, client::IntoClientRequest};

/// Where decoded frames go: the screen's video layer.
pub(crate) trait Sink: Send + Sync + 'static {
    /// New parameter sets; returns the picture's size in pixels.
    fn configure(&self, sets: &ParameterSets) -> Result<(u32, u32), String>;
    /// Queues one access unit to be shown as soon as it decodes.
    fn frame(&self, avcc: &[u8]) -> Queued;
    /// Drops queued frames after a failure, a pause or a new stream.
    fn flush(&self);
    /// Whether anyone can see the picture. A video layer that nothing
    /// composites keeps every frame it is given (1.2 million of them, 1.4 GB,
    /// measured on 2026-11-11), so while this is false frames are dropped
    /// before they reach it.
    fn showing(&self) -> bool;
}

/// What the sink did with an access unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Queued {
    Yes,
    /// The decoder failed; it needs a keyframe.
    Failed,
    /// Dropped: the layer holds all it will take, so it isn't keeping up.
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    Connecting(Option<String>),
    Streaming,
    /// The picture's size in pixels, on the first frame and when it changes.
    Size(u32, u32),
    /// Whether touches and keys reach the device, and why not.
    Input(bool, Option<String>),
    /// What serve-sim says about the screen: its orientation, which turns
    /// the picture, and a foldable's hinge.
    Screen(ScreenConfig),
    /// The device refused a fold.
    FoldFailed(String),
    /// The hub this stream talks to stopped or was replaced: its port and
    /// token are gone, so the panel connects again through `hub::ensure`,
    /// which waits for a restart.
    HubChanged,
}

#[derive(Clone, Debug)]
pub(crate) enum Input {
    /// 0–1 across the picture.
    Touch(Phase, f64, f64),
    Button(Button),
    Key {
        key: String,
        character: Option<String>,
        modifiers: Modifiers,
    },
    Text(String),
    Rotate,
    /// A foldable's pose; iOS only.
    Fold(Pose),
    /// A foldable's hinge opened to this many degrees; iOS only.
    FoldAngle(f64),
}

/// Wakes a socket thread's `poll` when input is queued or the stream stops.
struct Wake {
    read: std::os::fd::OwnedFd,
    write: std::os::fd::OwnedFd,
}

impl Wake {
    fn new() -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;
        let mut fds = [0; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        for fd in fds {
            unsafe {
                libc::fcntl(
                    fd,
                    libc::F_SETFL,
                    libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK,
                );
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        Ok(Self {
            read: unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[0]) },
            write: unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[1]) },
        })
    }
    fn ring(&self) {
        unsafe {
            libc::write(self.write.as_raw_fd(), [1u8].as_ptr().cast(), 1);
        }
    }
    fn drain(&self) {
        let mut buf = [0u8; 64];
        while unsafe { libc::read(self.read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {
        }
    }
    /// Blocks until `socket` is readable, the bell rings, or `timeout`.
    fn wait(&self, socket: &impl AsRawFd, timeout: Duration) {
        let mut fds = [
            libc::pollfd {
                fd: socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe {
            libc::poll(fds.as_mut_ptr(), 2, timeout.as_millis() as i32);
        }
    }
}

struct Shared {
    stop: AtomicBool,
    /// Frames were dropped while nobody could see the picture, so it is out
    /// of date until a keyframe comes.
    dropped: AtomicBool,
    /// Asked for a keyframe from outside the stream threads, as when the
    /// picture comes back into view.
    resync: AtomicBool,
    wake: Wake,
    closers: Mutex<Vec<http::Closer>>,
    events: async_channel::Sender<Event>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    fn emit(&self, event: Event) {
        let _ = self.events.try_send(event);
    }
    /// Sleeps before a retry, waking early to stop.
    fn pause(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while !self.stopped() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A live device. Dropping it stops its threads and closes its sockets.
pub(crate) struct Stream {
    shared: Arc<Shared>,
    input: mpsc::Sender<Input>,
    pub(crate) events: async_channel::Receiver<Event>,
    _viewer: hub::Viewer,
}

impl Stream {
    pub(crate) fn send(&self, input: Input) {
        if self.input.send(input).is_ok() {
            self.shared.wake.ring();
        }
    }
    /// A sender for the screen's own event handlers, which run outside GPUI.
    pub(crate) fn sender(&self) -> InputSender {
        InputSender {
            input: self.input.clone(),
            shared: self.shared.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct InputSender {
    input: mpsc::Sender<Input>,
    shared: Arc<Shared>,
}

impl InputSender {
    pub(crate) fn send(&self, input: Input) {
        if self.input.send(input).is_ok() {
            self.shared.wake.ring();
        }
    }
    /// The picture is in view again. If frames were dropped while it wasn't,
    /// asks the device for a keyframe, so a screen that has since gone still
    /// isn't left showing what it showed before. An Android stream asks over
    /// its socket; serve-sim sends one to each new video connection, so an
    /// iOS stream's video is closed and reopened.
    pub(crate) fn resync(&self) {
        if !self.shared.dropped.swap(false, Ordering::Relaxed) {
            return;
        }
        self.shared.resync.store(true, Ordering::Relaxed);
        self.shared.wake.ring();
        for closer in self
            .shared
            .closers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            closer.close();
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.ring();
        for closer in self
            .shared
            .closers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            closer.close();
        }
    }
}

pub(crate) fn start(
    endpoint: Arc<Endpoint>,
    platform: Platform,
    device: String,
    sink: Arc<dyn Sink>,
) -> Result<Stream, String> {
    let (events_tx, events) = async_channel::bounded(64);
    let viewer = hub::Viewer::new(&device);
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        dropped: AtomicBool::new(false),
        resync: AtomicBool::new(false),
        wake: Wake::new().map_err(|e| e.to_string())?,
        closers: Mutex::new(Vec::new()),
        events: events_tx,
    });
    let (input, inputs) = mpsc::channel();
    shared.emit(Event::Connecting(None));
    match platform {
        Platform::Ios => {
            let (video, endpoint_video, device_video) =
                (shared.clone(), endpoint.clone(), device.clone());
            std::thread::Builder::new()
                .name("device-video".into())
                .spawn(move || ios_video(&video, &endpoint_video, &device_video, &*sink))
                .map_err(|e| e.to_string())?;
            let control = shared.clone();
            std::thread::Builder::new()
                .name("device-input".into())
                .spawn(move || ios_input(&control, &endpoint, &device, &inputs))
                .map_err(|e| e.to_string())?;
        }
        Platform::Android => {
            let control = shared.clone();
            std::thread::Builder::new()
                .name("device-android".into())
                .spawn(move || android(&control, &endpoint, &device, &inputs, &*sink))
                .map_err(|e| e.to_string())?;
        }
    }
    let watch = shared.clone();
    let generation = hub::generation();
    std::thread::Builder::new()
        .name("device-hub-watch".into())
        .spawn(move || watch_hub(&watch, generation))
        .map_err(|e| e.to_string())?;
    Ok(Stream {
        shared,
        input,
        events,
        _viewer: viewer,
    })
}

/// Tells the panel once the hub run this stream started on is over.
/// Asking for the state is what notices a hub that exited, when its output
/// is held open by a helper it started.
fn watch_hub(shared: &Shared, generation: u64) {
    while !shared.stopped() {
        shared.pause(Duration::from_millis(500));
        let _ = hub::state();
        if hub::generation() != generation {
            while !shared.stopped() && shared.events.try_send(Event::HubChanged).is_err() {
                shared.pause(Duration::from_millis(100));
            }
            return;
        }
    }
}

/// Turns access units into frames on screen, asking for a keyframe when the
/// decoder has none to start from.
///
/// H.264 frames build on the ones before, so once one is dropped nothing is
/// queued again until a keyframe. A frame is dropped when the layer is full
/// (it isn't keeping up) or nobody can see the picture; the layer is flushed
/// then, so what it held is let go.
struct Feed<'a> {
    sink: &'a dyn Sink,
    sets: Option<ParameterSets>,
    size: Option<(u32, u32)>,
    awaiting_key: bool,
    streaming: bool,
    /// Dropping frames because nobody can see the picture.
    paused: bool,
    /// A keyframe is wanted now, not whenever one comes: frames were dropped
    /// mid-stream, so the picture is out of date.
    resync: bool,
}

#[derive(Debug, PartialEq)]
enum Fed {
    Shown,
    Skipped,
    /// No keyframe has come yet to start from.
    Waiting,
    /// The picture needs a keyframe now: the decoder failed or fell behind,
    /// or frames were dropped while the picture was out of view.
    Resync,
    /// Dropped: nobody can see the picture.
    Paused,
}

impl<'a> Feed<'a> {
    fn new(sink: &'a dyn Sink) -> Self {
        sink.flush();
        Self {
            sink,
            sets: None,
            size: None,
            awaiting_key: true,
            streaming: false,
            paused: false,
            resync: false,
        }
    }
    fn need_key(&self) -> Fed {
        if self.resync || self.streaming {
            Fed::Resync
        } else {
            Fed::Waiting
        }
    }
    fn configure(&mut self, shared: &Shared, sets: ParameterSets) -> Result<(), String> {
        if self.sets.as_ref() == Some(&sets) {
            return Ok(());
        }
        let size = self.sink.configure(&sets)?;
        self.sets = Some(sets);
        self.awaiting_key = true;
        if self.size != Some(size) {
            self.size = Some(size);
            shared.emit(Event::Size(size.0, size.1));
        }
        Ok(())
    }
    fn frame(&mut self, shared: &Shared, avcc: &[u8], key: bool) -> Fed {
        // A stopped stream's last reads must not reach a layer that may
        // already show the next device.
        if shared.stopped() {
            return Fed::Skipped;
        }
        if !self.sink.showing() {
            if !self.paused {
                self.paused = true;
                self.sink.flush();
            }
            self.awaiting_key = true;
            self.resync = true;
            shared.dropped.store(true, Ordering::Relaxed);
            return Fed::Paused;
        }
        self.paused = false;
        if self.sets.is_none() || (self.awaiting_key && !key) || avcc.is_empty() {
            return if self.sets.is_none() || self.awaiting_key {
                self.need_key()
            } else {
                Fed::Skipped
            };
        }
        match self.sink.frame(avcc) {
            Queued::Yes => {}
            Queued::Failed => {
                self.sink.flush();
                self.awaiting_key = true;
                return self.need_key();
            }
            Queued::Full => {
                self.sink.flush();
                self.awaiting_key = true;
                self.resync = true;
                return Fed::Resync;
            }
        }
        self.awaiting_key = false;
        self.resync = false;
        shared.dropped.store(false, Ordering::Relaxed);
        if !self.streaming {
            self.streaming = true;
            shared.emit(Event::Streaming);
        }
        Fed::Shown
    }
}

/// serve-sim stops sending while the screen is still; an open body that
/// stays silent this long has lost its helper.
const VIDEO_SILENCE: Duration = Duration::from_secs(15);

fn ios_video(shared: &Shared, endpoint: &Endpoint, device: &str, sink: &dyn Sink) {
    let path = hub::vendor_path(
        Platform::Ios,
        &format!("/helper/{}/stream.avcc", hub::encode(device)),
    );
    let mut first = true;
    // Set when the helper seems gone: attaches are remembered per hub run,
    // and a helper that died needs starting again.
    let mut reattach = false;
    // The last connection closed only to get a keyframe.
    let mut resyncing = false;
    while !shared.stopped() {
        // Only the one reconnect after a keyframe close comes quickly; a
        // failure on the way retries at the usual pace.
        let quick = std::mem::take(&mut resyncing);
        if !first {
            shared.pause(if quick {
                RESYNC_GAP
            } else {
                Duration::from_secs(1)
            });
            if shared.stopped() {
                break;
            }
        }
        first = false;
        if std::mem::take(&mut reattach) {
            hub::forget_attached(device);
            if let Err(error) = hub::attach(endpoint, device) {
                shared.emit(Event::Connecting(Some(error)));
                continue;
            }
        }
        let (mut body, closer) = match http::get(
            endpoint.port,
            &path,
            &endpoint.authorization(),
            VIDEO_SILENCE,
        ) {
            Ok(opened) => opened,
            Err(error) => {
                shared.emit(Event::Connecting(Some(format!("Video: {error}"))));
                reattach = true;
                continue;
            }
        };
        shared
            .closers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(closer.clone());
        if shared.stopped() {
            closer.close();
            break;
        }
        // A new connection starts with a keyframe.
        shared.resync.store(false, Ordering::Relaxed);
        let mut demuxer = protocol::AvccDemuxer::default();
        let mut feed = Feed::new(sink);
        let mut buf = vec![0u8; 256 * 1024];
        let reason = 'read: loop {
            let read = match std::io::Read::read(&mut body, &mut buf) {
                Ok(0) => break 'read "The video stream ended.".to_string(),
                Ok(read) => read,
                Err(error) => break 'read format!("Video: {error}"),
            };
            hub::touch();
            hub::touch_device(device);
            let chunks = match demuxer.push(&buf[..read]) {
                Ok(chunks) => chunks,
                Err(error) => break 'read error,
            };
            for chunk in chunks {
                let chunk_kind = match chunk {
                    Chunk::Keyframe(_) => Kind::Key,
                    _ => Kind::Other,
                };
                match chunk {
                    Chunk::Description(record) => {
                        let Some(sets) = protocol::avcc_parameter_sets(&record) else {
                            break 'read "The device sent an unreadable video description.".into();
                        };
                        if let Err(error) = feed.configure(shared, sets) {
                            break 'read error;
                        }
                    }
                    Chunk::Keyframe(unit) | Chunk::Delta(unit) => {
                        let key = matches!(chunk_kind, Kind::Key);
                        if let Fed::Resync = feed.frame(shared, &unit, key) {
                            // Reconnecting is how serve-sim sends a keyframe.
                            shared.resync.store(true, Ordering::Relaxed);
                            break 'read "Video decoder needed a keyframe.".into();
                        }
                    }
                    Chunk::Seed(_) => {}
                }
            }
        };
        closer.close();
        shared
            .closers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|other| !other.same(&closer));
        // Closed for a keyframe: the picture stays up and nothing is
        // reported, and the reconnect comes at once. One that showed nothing
        // after the last such reconnect is a fault, retried as any other.
        let wanted = shared.resync.swap(false, Ordering::Relaxed);
        resyncing = wanted && (feed.streaming || !quick);
        if resyncing {
            continue;
        }
        // Ending before any frame showed is a missing helper, not the
        // reconnect that asks serve-sim for a keyframe.
        reattach = !feed.streaming;
        if !shared.stopped() {
            shared.emit(Event::Connecting(Some(reason)));
        }
    }
}

/// How soon video reconnects for a keyframe; a decoder that keeps failing
/// still can't spin.
const RESYNC_GAP: Duration = Duration::from_millis(100);

enum Kind {
    Key,
    Other,
}

type Socket = WebSocket<std::net::TcpStream>;

fn connect(endpoint: &Endpoint, path: &str) -> Result<Socket, String> {
    let mut request = endpoint
        .ws_url(path)
        .into_client_request()
        .map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        "Authorization",
        endpoint
            .authorization()
            .parse()
            .map_err(|_| "invalid token".to_string())?,
    );
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], endpoint.port));
    let stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    stream.set_nodelay(true).ok();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let (socket, _) = tungstenite::client(request, stream).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(error) => error.to_string(),
        tungstenite::HandshakeError::Interrupted(_) => "handshake interrupted".into(),
    })?;
    socket
        .get_ref()
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;
    Ok(socket)
}

fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// Queues a message and pushes it out; a full loopback buffer only delays it.
fn send(socket: &mut Socket, message: Message) -> Result<(), String> {
    let message = match message {
        Message::Binary(bytes) => WsMessage::Binary(bytes.into()),
        Message::Text(text) => WsMessage::Text(text.into()),
    };
    match socket.send(message) {
        Ok(()) => Ok(()),
        Err(error) if would_block(&error) => flush(socket),
        Err(error) => Err(error.to_string()),
    }
}

fn flush(socket: &mut Socket) -> Result<(), String> {
    for _ in 0..200 {
        match socket.flush() {
            Ok(()) => return Ok(()),
            Err(error) if would_block(&error) => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("The device stopped reading input.".into())
}

/// serve-sim only accepts input and reports its screen once capture runs.
/// The video request starts it too, and admits input in about 0.1 s, so this
/// is only for a retry: waiting on it first held input back half a second.
fn prime_ios(endpoint: &Endpoint, device: &str) {
    let path = hub::vendor_path(
        Platform::Ios,
        &format!("/helper/{}/stream.mjpeg", hub::encode(device)),
    );
    if let Ok((mut body, closer)) = http::get(
        endpoint.port,
        &path,
        &endpoint.authorization(),
        Duration::from_secs(2),
    ) {
        let mut buf = [0u8; 1024];
        let _ = std::io::Read::read(&mut body, &mut buf);
        closer.close();
    }
}

fn ios_input(shared: &Shared, endpoint: &Endpoint, device: &str, inputs: &mpsc::Receiver<Input>) {
    let path = hub::vendor_path(
        Platform::Ios,
        &format!("/helper/ws?device={}", hub::encode(device)),
    );
    let mut first = true;
    while !shared.stopped() {
        if !first {
            shared.pause(Duration::from_secs(1));
            prime_ios(endpoint, device);
        }
        first = false;
        if shared.stopped() {
            break;
        }
        let mut socket = match connect(endpoint, &path) {
            Ok(socket) => socket,
            Err(error) => {
                shared.emit(Event::Input(false, Some(error)));
                continue;
            }
        };
        let mut admitted = false;
        let mut screen: Option<ScreenConfig> = None;
        let mut requests = 0u64;
        let reason = loop {
            shared.wake.wait(socket.get_ref(), Duration::from_secs(1));
            shared.wake.drain();
            if shared.stopped() {
                let _ = socket.close(None);
                return;
            }
            let mut failed = None;
            while let Ok(input) = inputs.try_recv() {
                // Input before admission would be dropped by serve-sim.
                if !admitted {
                    continue;
                }
                for message in ios_messages(input, screen.as_ref(), &mut requests) {
                    if let Err(error) = send(&mut socket, message) {
                        failed = Some(error);
                    }
                }
            }
            if let Some(error) = failed {
                break error;
            }
            let read = loop {
                match socket.read() {
                    Ok(WsMessage::Binary(bytes)) if !bytes.is_empty() => match bytes[0] {
                        protocol::ios::INPUT_ADMITTED => {
                            admitted = true;
                            let available = !screen.is_some_and(|s| s.input_unavailable);
                            shared.emit(Event::Input(available, None));
                        }
                        protocol::ios::SCREEN_CONFIG => {
                            if let Some(config) = ScreenConfig::parse(&bytes[1..]) {
                                screen = Some(config);
                                shared.emit(Event::Screen(config));
                                shared.emit(Event::Input(
                                    admitted && !config.input_unavailable,
                                    config
                                        .input_unavailable
                                        .then(|| "Simulator input is unavailable".into()),
                                ));
                            }
                        }
                        protocol::ios::HINGE_RESULT => {
                            if let Some(result) = HingeResult::parse(&bytes[1..])
                                && !result.ok
                            {
                                shared.emit(Event::FoldFailed(result.error.unwrap_or_else(|| {
                                    "The simulator could not change its pose.".into()
                                })));
                            }
                        }
                        _ => {}
                    },
                    Ok(WsMessage::Close(_)) => break Err("The device closed its input.".into()),
                    Ok(_) => {}
                    Err(error) if would_block(&error) => break Ok(()),
                    Err(error) => break Err(error.to_string()),
                }
            };
            if let Err(error) = read {
                break error;
            }
        };
        if !shared.stopped() {
            shared.emit(Event::Input(false, Some(reason)));
        }
    }
}

fn ios_messages(input: Input, screen: Option<&ScreenConfig>, requests: &mut u64) -> Vec<Message> {
    match input {
        Input::Touch(phase, x, y) => vec![protocol::touch(Platform::Ios, screen, phase, x, y)],
        Input::Button(button) => protocol::button(Platform::Ios, button)
            .into_iter()
            .collect(),
        Input::Key {
            key,
            character,
            modifiers,
        } => protocol::key_press(Platform::Ios, &key, character.as_deref(), modifiers),
        // serve-sim has no text message; type it key by key.
        Input::Text(text) => text
            .chars()
            .flat_map(|c| {
                let lower = c.to_ascii_lowercase().to_string();
                let modifiers = Modifiers {
                    shift: c.is_ascii_uppercase(),
                    ..Default::default()
                };
                protocol::key_press(Platform::Ios, &lower, None, modifiers)
            })
            .collect(),
        Input::Rotate => vec![protocol::orientation(
            screen
                .map_or(Orientation::Portrait, |s| s.orientation)
                .next(),
        )],
        Input::Fold(pose) => {
            *requests += 1;
            vec![protocol::hinge(*requests, pose)]
        }
        Input::FoldAngle(degrees) => {
            *requests += 1;
            vec![protocol::hinge_angle(*requests, degrees)]
        }
    }
}

fn android(
    shared: &Shared,
    endpoint: &Endpoint,
    device: &str,
    inputs: &mpsc::Receiver<Input>,
    sink: &dyn Sink,
) {
    let path = format!(
        "/vendor/serve-emu/ws?device={}&frame-meta=1",
        hub::encode(device)
    );
    let mut first = true;
    while !shared.stopped() {
        if !first {
            shared.pause(Duration::from_secs(1));
            if shared.stopped() {
                break;
            }
        }
        first = false;
        let mut socket = match connect(endpoint, &path) {
            Ok(socket) => socket,
            Err(error) => {
                shared.emit(Event::Connecting(Some(error)));
                continue;
            }
        };
        shared.emit(Event::Input(true, None));
        let mut feed = Feed::new(sink);
        let mut asked: Option<Instant> = None;
        let mut ask = |socket: &mut Socket| -> Result<(), String> {
            // One request per half second is plenty; each costs an IDR.
            if asked.is_none_or(|at| at.elapsed() > Duration::from_millis(500)) {
                asked = Some(Instant::now());
                send(socket, protocol::reset_video())?;
            }
            Ok(())
        };
        let reason = 'session: loop {
            shared.wake.wait(socket.get_ref(), Duration::from_secs(1));
            shared.wake.drain();
            if shared.stopped() {
                let _ = socket.close(None);
                return;
            }
            if shared.resync.swap(false, Ordering::Relaxed)
                && let Err(error) = ask(&mut socket)
            {
                break 'session error;
            }
            while let Ok(input) = inputs.try_recv() {
                let messages = match input {
                    Input::Touch(phase, x, y) => {
                        vec![protocol::touch(Platform::Android, None, phase, x, y)]
                    }
                    Input::Button(button) => protocol::button(Platform::Android, button)
                        .into_iter()
                        .collect(),
                    Input::Key {
                        key,
                        character,
                        modifiers,
                    } => protocol::key_press(
                        Platform::Android,
                        &key,
                        character.as_deref(),
                        modifiers,
                    ),
                    Input::Text(text) => protocol::text_messages(&text),
                    Input::Rotate | Input::Fold(_) | Input::FoldAngle(_) => Vec::new(),
                };
                for message in messages {
                    if let Err(error) = send(&mut socket, message) {
                        break 'session error;
                    }
                }
            }
            loop {
                match socket.read() {
                    Ok(WsMessage::Binary(bytes)) => {
                        hub::touch();
                        hub::touch_device(device);
                        let packet = protocol::semu(&bytes);
                        let unit = protocol::annexb_access_unit(packet.data);
                        if unit.sets.is_complete()
                            && let Err(error) = feed.configure(shared, unit.sets.clone())
                        {
                            break 'session error;
                        }
                        let key = packet.key.unwrap_or(unit.key) || unit.key;
                        if let Fed::Waiting | Fed::Resync = feed.frame(shared, &unit.avcc, key)
                            && let Err(error) = ask(&mut socket)
                        {
                            break 'session error;
                        }
                    }
                    Ok(WsMessage::Text(text)) => {
                        // The encoder restarted at a new size, as after a
                        // rotation; its next keyframe carries new sets.
                        if protocol::is_video_session(&text) {
                            feed = Feed::new(sink);
                            if let Err(error) = ask(&mut socket) {
                                break 'session error;
                            }
                        }
                    }
                    Ok(WsMessage::Close(frame)) => {
                        break 'session frame
                            .map(|frame| frame.reason.to_string())
                            .filter(|reason| !reason.is_empty())
                            .unwrap_or_else(|| "The emulator closed the stream.".into());
                    }
                    Ok(_) => {}
                    Err(error) if would_block(&error) => break,
                    Err(error) => break 'session error.to_string(),
                }
            }
        };
        if !shared.stopped() {
            shared.emit(Event::Connecting(Some(reason)));
            shared.emit(Event::Input(false, None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layer that takes what it is given, as many as `room` frames until
    /// flushed, and can be out of view.
    struct Recorder {
        frames: Mutex<Vec<usize>>,
        fail: AtomicBool,
        hidden: AtomicBool,
        room: Mutex<Option<usize>>,
        queued: Mutex<usize>,
        flushes: Mutex<usize>,
    }
    impl Default for Recorder {
        fn default() -> Self {
            Self {
                frames: Mutex::default(),
                fail: AtomicBool::new(false),
                hidden: AtomicBool::new(false),
                room: Mutex::new(None),
                queued: Mutex::new(0),
                flushes: Mutex::new(0),
            }
        }
    }
    impl Sink for Recorder {
        fn configure(&self, _: &ParameterSets) -> Result<(u32, u32), String> {
            Ok((1206, 2622))
        }
        fn frame(&self, avcc: &[u8]) -> Queued {
            if self.fail.load(Ordering::Relaxed) {
                self.frames.lock().unwrap().push(avcc.len());
                return Queued::Failed;
            }
            let mut queued = self.queued.lock().unwrap();
            if self
                .room
                .lock()
                .unwrap()
                .is_some_and(|room| *queued >= room)
            {
                return Queued::Full;
            }
            *queued += 1;
            self.frames.lock().unwrap().push(avcc.len());
            Queued::Yes
        }
        fn flush(&self) {
            *self.queued.lock().unwrap() = 0;
            *self.flushes.lock().unwrap() += 1;
        }
        fn showing(&self) -> bool {
            !self.hidden.load(Ordering::Relaxed)
        }
    }

    fn shared() -> (Shared, async_channel::Receiver<Event>) {
        let (events, receiver) = async_channel::unbounded();
        (
            Shared {
                stop: AtomicBool::new(false),
                dropped: AtomicBool::new(false),
                resync: AtomicBool::new(false),
                wake: Wake::new().unwrap(),
                closers: Mutex::new(Vec::new()),
                events,
            },
            receiver,
        )
    }

    fn sets() -> ParameterSets {
        ParameterSets {
            sps: vec![vec![0x67]],
            pps: vec![vec![0x68]],
        }
    }
    const KEY: &[u8] = &[0, 0, 0, 1, 0x65];
    const DELTA: &[u8] = &[0, 0, 0, 1, 0x41];

    #[test]
    fn frames_wait_for_sets_and_a_keyframe_and_report_once() {
        let (shared, events) = shared();
        let sink = Recorder::default();
        let mut feed = Feed::new(&sink);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Waiting);
        feed.configure(&shared, sets()).unwrap();
        feed.configure(&shared, sets()).unwrap();
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Waiting);
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Shown);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Shown);
        sink.fail.store(true, Ordering::Relaxed);
        // A decoder failing mid-stream wants a keyframe now.
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Resync);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Resync);
        let seen: Vec<Event> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert_eq!(seen, [Event::Size(1206, 2622), Event::Streaming]);
        assert_eq!(sink.frames.lock().unwrap().len(), 3);
    }

    /// A layer that isn't keeping up drops the frame, lets go of what it
    /// holds, and takes nothing more until a keyframe: a delta after a
    /// dropped one would decode to garbage.
    #[test]
    fn a_full_layer_drops_and_starts_again_at_a_keyframe() {
        let (shared, _) = shared();
        let sink = Recorder::default();
        *sink.room.lock().unwrap() = Some(2);
        let mut feed = Feed::new(&sink);
        feed.configure(&shared, sets()).unwrap();
        let flushes = *sink.flushes.lock().unwrap();
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Shown);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Shown);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Resync);
        assert_eq!(*sink.flushes.lock().unwrap(), flushes + 1, "flushed");
        assert_eq!(*sink.queued.lock().unwrap(), 0, "what it held is let go");
        // Deltas are not queued until a keyframe comes.
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Resync);
        assert_eq!(*sink.queued.lock().unwrap(), 0);
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Shown);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Shown);
        assert_eq!(sink.frames.lock().unwrap().len(), 4);
    }

    /// Out of view, nothing reaches the layer and it is flushed once; back in
    /// view, the stream asks for a keyframe and starts again from it.
    #[test]
    fn an_unseen_picture_takes_no_frames_and_resyncs_when_seen() {
        let (shared, _) = shared();
        let sink = Recorder::default();
        let mut feed = Feed::new(&sink);
        feed.configure(&shared, sets()).unwrap();
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Shown);
        let flushes = *sink.flushes.lock().unwrap();
        sink.hidden.store(true, Ordering::Relaxed);
        for _ in 0..100 {
            assert_eq!(feed.frame(&shared, DELTA, false), Fed::Paused);
        }
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Paused);
        assert_eq!(*sink.flushes.lock().unwrap(), flushes + 1, "flushed once");
        assert_eq!(
            sink.frames.lock().unwrap().len(),
            1,
            "nothing queued unseen"
        );
        assert!(shared.dropped.load(Ordering::Relaxed));
        sink.hidden.store(false, Ordering::Relaxed);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Resync);
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Shown);
        assert!(!shared.dropped.load(Ordering::Relaxed));
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Shown);
    }

    /// A stream that opened while out of view never streamed, but the
    /// keyframe it dropped is still wanted once it is seen.
    #[test]
    fn a_stream_opened_unseen_resyncs_when_seen() {
        let (shared, _) = shared();
        let sink = Recorder::default();
        sink.hidden.store(true, Ordering::Relaxed);
        let mut feed = Feed::new(&sink);
        feed.configure(&shared, sets()).unwrap();
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Paused);
        sink.hidden.store(false, Ordering::Relaxed);
        assert_eq!(feed.frame(&shared, DELTA, false), Fed::Resync);
    }

    /// Coming back into view asks for a keyframe only when frames were
    /// dropped meanwhile; an iOS stream does it by closing its video.
    #[test]
    fn resync_asks_only_after_frames_were_dropped() {
        let (shared, _) = shared();
        let sender = InputSender {
            input: mpsc::channel().0,
            shared: Arc::new(shared),
        };
        sender.resync();
        assert!(!sender.shared.resync.load(Ordering::Relaxed));
        sender.shared.dropped.store(true, Ordering::Relaxed);
        sender.resync();
        assert!(sender.shared.resync.load(Ordering::Relaxed));
        assert!(!sender.shared.dropped.load(Ordering::Relaxed));
    }

    /// A stopped stream's late reads never touch the layer, which may be
    /// showing the next device already.
    #[test]
    fn a_stopped_stream_leaves_the_layer_alone() {
        let (shared, _) = shared();
        let sink = Recorder::default();
        let mut feed = Feed::new(&sink);
        feed.configure(&shared, sets()).unwrap();
        shared.stop.store(true, Ordering::Relaxed);
        let flushes = *sink.flushes.lock().unwrap();
        assert_eq!(feed.frame(&shared, KEY, true), Fed::Skipped);
        assert!(sink.frames.lock().unwrap().is_empty());
        assert_eq!(*sink.flushes.lock().unwrap(), flushes);
    }

    #[test]
    fn the_wake_pipe_interrupts_a_wait() {
        let (shared, _) = shared();
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        shared.wake.ring();
        let started = Instant::now();
        shared.wake.wait(&a, Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
        shared.wake.drain();
    }

    #[test]
    fn ios_text_is_typed_with_shift_for_capitals() {
        let messages = ios_messages(Input::Text("Hi".into()), None, &mut 0);
        // Shift down, H down, H up, shift up, then i down and up.
        assert_eq!(messages.len(), 6);
    }
}
