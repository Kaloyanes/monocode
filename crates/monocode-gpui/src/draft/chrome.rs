//! The Chromium side of a draft: a browser in the shared CEF host whose
//! frames the card draws straight from the GPU surfaces CEF renders into.
//! GPUI paints them like anything else, so popovers and scrims cover the page
//! with no native view to step aside, and the person's mouse and keys reach
//! the page as DevTools input, which the page sees as trusted.
use super::{
    cdp::{self, Browser},
    web::{Input, Load, Placement, fit},
};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, CursorStyle, DispatchPhase, FocusHandle, KeyDownEvent, KeyUpEvent,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    RenderImage, ScrollWheelEvent, Task, Window, canvas, div, prelude::*, px,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    sync::Arc,
    time::{Duration, Instant},
};

/// Chromium renders at the Retina scale; frames are scaled down to what the
/// card shows.
const DEVICE_SCALE: f32 = 2.;
/// The largest frame side, in pixels; a taller page is drawn at a lower scale
/// rather than overrun the GPU's texture limit.
const MAX_FRAME: f32 = 16384.;
/// After this many failed starts the draft goes back to WebKit.
const MAX_FAILURES: u8 = 3;
/// How long a ready page takes to fade in over its placeholder.
const FADE: Duration = Duration::from_millis(180);
/// Bindings the page calls; see `page::WRAPPER_SCRIPT` and `CURSOR_SCRIPT`.
const WAKE: &str = "__mcWake";
const CURSOR: &str = "__mcCursor";
/// Where the agent's page is served from; requests to it never leave the app.
const ORIGIN: &str = "https://draft.monocode.invalid/";

/// Reports the cursor the page asks for, which DevTools does not.
const CURSOR_SCRIPT: &str = r#"(() => {
  let last = "";
  addEventListener("mousemove", (event) => {
    const el = event.target instanceof Element ? event.target : document.body;
    if (!el) return;
    let cursor = getComputedStyle(el).cursor;
    if (cursor === "auto") {
      cursor = el.closest("input, textarea, [contenteditable]") ? "text" : "default";
    }
    if (cursor !== last) { last = cursor; try { globalThis.__mcCursor(cursor); } catch (e) {} }
  }, true);
})()"#;

/// The selection in a frame, as text, for copying.
const SELECTION_SCRIPT: &str = r#"(() => {
  const a = document.activeElement;
  if (a && (a.tagName === "INPUT" || a.tagName === "TEXTAREA") && typeof a.selectionStart === "number")
    return a.value.substring(a.selectionStart, a.selectionEnd);
  return String(getSelection() || "");
})()"#;

struct TabInner {
    browser: Arc<Browser>,
    id: u64,
    session: String,
    /// The top frame, the only one the agent's page is served to.
    frame: String,
}

impl Drop for TabInner {
    fn drop(&mut self) {
        self.browser.close_tab(self.id);
    }
}

/// One draft's tab: a browser with its own in-memory storage, so nothing it
/// stores outlives it or reaches another draft.
#[derive(Clone)]
pub(crate) struct Tab(Arc<TabInner>);

impl Tab {
    fn send(&self, method: &str, params: Value) -> async_channel::Receiver<Result<Value, String>> {
        self.0.browser.send(method, params, Some(&self.0.session))
    }
    /// Sends a command whose reply does not matter.
    fn fire(&self, method: &str, params: Value) {
        drop(self.send(method, params));
    }
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.0
            .browser
            .call(method, params, Some(&self.0.session))
            .await
    }

    async fn open(
        host: &std::path::Path,
        html: bool,
        (width, height): (u32, u32),
    ) -> Result<(Self, async_channel::Receiver<Value>, async_channel::Receiver<cdp::Frame>), String>
    {
        let browser = Browser::shared(host)?;
        let id = browser.open_tab(width, height, DEVICE_SCALE).await?;
        let session = id.to_string();
        let (send, events) = async_channel::unbounded();
        browser.listen(&session, send);
        let (send, frames) = async_channel::bounded(2);
        browser.watch_frames(id, send);
        let mut tab = Self(Arc::new(TabInner {
            browser,
            id,
            session,
            frame: String::new(),
        }));
        let mut setup = vec![
            ("Page.enable", json!({})),
            ("Runtime.enable", json!({})),
            ("Runtime.addBinding", json!({"name": WAKE})),
            ("Runtime.addBinding", json!({"name": CURSOR})),
            // A headless page never has focus: no caret, no :focus styles.
            (
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled": true}),
            ),
            (
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": CURSOR_SCRIPT}),
            ),
            (
                "Page.setInterceptFileChooserDialog",
                json!({"enabled": true}),
            ),
            (
                "Fetch.enable",
                json!({"patterns": [{"urlPattern": "*", "resourceType": "Document"}]}),
            ),
        ];
        if html {
            // The agent's page sits on the thread's own background.
            setup.push((
                "Emulation.setDefaultBackgroundColorOverride",
                json!({"color": {"r": 0, "g": 0, "b": 0, "a": 0}}),
            ));
        } else {
            // A dev server's page is not ours to wrap, so the probe goes in
            // beside it, in the top frame only.
            setup.push((
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": format!("if(window===window.top){{{}}}", super::page::browser_probe())}),
            ));
        }
        for (method, params) in setup {
            tab.call(method, params).await?;
        }
        let tree = tab.call("Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or("Chromium did not return its frame")?
            .to_owned();
        if let Some(inner) = Arc::get_mut(&mut tab.0) {
            inner.frame = frame;
        }
        Ok((tab, events, frames))
    }
}

/// Runs `script` in the page's top document and returns its string result.
pub(super) async fn evaluate(tab: &Tab, script: &str, cx: &gpui::AsyncApp) -> Option<String> {
    let reply = tab.send(
        "Runtime.evaluate",
        json!({"expression": script, "returnByValue": true}),
    );
    let value = cdp::within(reply, Duration::from_secs(3), cx).await?;
    value["result"]["value"].as_str().map(str::to_owned)
}

/// The visible page as a PNG, `width` points wide at the Retina scale.
pub(super) async fn png(
    tab: &Tab,
    width: f64,
    cx: &gpui::AsyncApp,
) -> Result<(Vec<u8>, (u32, u32)), String> {
    let failed = || "The page could not be captured".to_string();
    let metrics = cdp::within(
        tab.send("Page.getLayoutMetrics", json!({})),
        Duration::from_secs(3),
        cx,
    )
    .await
    .ok_or_else(failed)?;
    let viewport = &metrics["cssLayoutViewport"];
    let number = |key: &str| viewport[key].as_f64().unwrap_or(0.);
    let (page_width, page_height) = (number("clientWidth"), number("clientHeight"));
    if page_width <= 0. || page_height <= 0. {
        return Err(failed());
    }
    let shot = cdp::within(
        tab.send(
            "Page.captureScreenshot",
            json!({"format": "png", "clip": {
                "x": number("pageX"), "y": number("pageY"),
                "width": page_width, "height": page_height,
                "scale": width / page_width,
            }}),
        ),
        Duration::from_secs(8),
        cx,
    )
    .await
    .ok_or_else(failed)?;
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().unwrap_or_default())
        .map_err(|_| failed())?;
    if png.len() < 24 {
        return Err(failed());
    }
    let size = (
        u32::from_be_bytes([png[16], png[17], png[18], png[19]]),
        u32::from_be_bytes([png[20], png[21], png[22], png[23]]),
    );
    Ok((png, size))
}

/// A picture of the page as it is now, for the card to show while the next
/// version loads.
pub(super) async fn picture(tab: &Tab, cx: &gpui::AsyncApp) -> Option<Arc<RenderImage>> {
    let width = page_width(tab, cx).await?;
    let (png, _) = png(tab, width, cx).await.ok()?;
    let image = cx
        .background_executor()
        .spawn(async move { pixels(&png) })
        .await?;
    Some(Arc::new(image))
}

async fn page_width(tab: &Tab, cx: &gpui::AsyncApp) -> Option<f64> {
    let metrics = cdp::within(
        tab.send("Page.getLayoutMetrics", json!({})),
        Duration::from_secs(3),
        cx,
    )
    .await?;
    metrics["cssLayoutViewport"]["clientWidth"].as_f64()
}

/// A PNG as the BGRA sprite GPUI draws.
fn pixels(png: &[u8]) -> Option<RenderImage> {
    let mut image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(RenderImage::new(vec![image::Frame::new(image)]))
}

/// The newest frame, and the page area it shows, in CSS pixels.
struct Frame {
    buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    size: (f32, f32),
}

/// The viewport and scale last asked of the tab.
#[derive(Clone, Copy, PartialEq)]
struct Metrics {
    width: u32,
    height: u32,
    scale: f32,
}

#[derive(Default)]
struct State {
    tab: Option<Tab>,
    opening: bool,
    failures: u8,
    /// Set once Chromium cannot run this draft; the engine falls back to WebKit.
    failed: Option<String>,
    _task: Option<Task<()>>,
    /// The version the draft is at, for the tab to load once it opens.
    wanted: Option<(u64, Load)>,
    /// The content handed to the tab, and the version whose document it shows.
    given: Option<(u64, Load)>,
    committed: Option<u64>,
    /// The agent's page, served at `ORIGIN` when the tab asks for it.
    document: Option<(String, Arc<str>)>,
    /// The tab's frames by DevTools execution context, for copying.
    contexts: Vec<i64>,
    frame: Option<Frame>,
    sent: Option<Metrics>,
    streaming: bool,
    /// The agent is working with the page; it keeps running out of sight.
    leases: u32,
    /// Out of sight and not leased: the page's timers and frames are paused,
    /// as WebKit pauses a hidden page.
    frozen: bool,
    /// When the card last drew the page. A window that stops painting, such
    /// as a hidden one, stops its frames too.
    last_paint: Option<Instant>,
    /// The card's own viewport, and one laid out out of sight to measure.
    layout: (u32, u32),
    measuring: Option<(u32, u32)>,
    /// The panel area the draft was last painted in, and the window zoom.
    area: Option<(f32, f32, f32)>,
    /// Where the page was last drawn, its scale, and the visible part.
    drawn: Option<(Point<Pixels>, f32, Bounds<Pixels>)>,
    painted: Option<(gpui::WindowId, u64)>,
    scrolls: bool,
    appearance: Option<bool>,
    cursor: CursorStyle,
    /// The person's button is down in the page: moves outside still reach it.
    pressed: bool,
    hovered: bool,
    focus: Option<FocusHandle>,
    /// Redraws the card that shows the page.
    notify: Option<Notify>,
    /// The agent's own HTML rather than a dev server: it gets no network.
    html: bool,
}

type Notify = Rc<dyn Fn(&mut App)>;

/// While held, the page runs for the agent even with its card out of sight.
pub(crate) struct Lease(Rc<RefCell<State>>);

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.0.borrow_mut();
        state.leases = state.leases.saturating_sub(1);
        state.freeze();
    }
}

/// A draft's tab and the card's view of it, shared with the tasks that read it.
#[derive(Clone)]
pub(super) struct Host {
    state: Rc<RefCell<State>>,
    input: Input,
    wake: async_channel::Sender<()>,
}

thread_local! {
    /// Bumped before each paint of a window, so a page can tell whether its
    /// card was drawn in the paint that just happened.
    static EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static HOSTS: RefCell<Vec<Weak<RefCell<State>>>> = const { RefCell::new(Vec::new()) };
}

/// Put last in a window's root: stops the frames of pages whose card was not
/// drawn this time, and starts them again when it is.
pub(super) fn sentinel() -> gpui::AnyElement {
    canvas(
        |_, _, _| EPOCH.with(|epoch| epoch.set(epoch.get() + 1)),
        |_, _, window, _| {
            let now = (
                Window::window_handle(window).window_id(),
                EPOCH.with(std::cell::Cell::get),
            );
            HOSTS.with(|hosts| {
                hosts.borrow_mut().retain(|state| {
                    let Some(state) = state.upgrade() else {
                        return false;
                    };
                    let mut state = state.borrow_mut();
                    if state
                        .painted
                        .is_some_and(|painted| painted.0 == now.0 && painted != now)
                    {
                        state.stop_streaming();
                    }
                    true
                });
            });
        },
    )
    .absolute()
    .size_0()
    .into_any_element()
}

impl State {
    fn stop_streaming(&mut self) {
        if self.streaming {
            self.streaming = false;
            if let Some(tab) = &self.tab {
                tab.0.browser.set_hidden(tab.0.id, true);
                tab.0.browser.set_focus(tab.0.id, false);
            }
        }
        self.freeze();
    }

    /// Pauses a page nobody sees and the agent is not using. Focus emulation
    /// keeps a page visible, so it goes off first.
    fn freeze(&mut self) {
        if self.frozen || self.streaming || self.leases > 0 {
            return;
        }
        if let Some(tab) = &self.tab {
            self.frozen = true;
            tab.fire(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled": false}),
            );
            tab.fire("Page.setWebLifecycleState", json!({"state": "frozen"}));
        }
    }

    /// Runs the page again. Setting it active leaves it hidden; focus
    /// emulation is what makes it visible and animate.
    fn thaw(&mut self) {
        if !self.frozen {
            return;
        }
        self.frozen = false;
        if let Some(tab) = &self.tab {
            tab.fire("Page.setWebLifecycleState", json!({"state": "active"}));
            tab.fire(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled": true}),
            );
        }
    }

    /// Lays the page out at `width` × `height`, drawn at the Retina scale
    /// unless that would make a frame too large for the GPU.
    fn apply(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        let metrics = Metrics {
            width,
            height,
            scale: DEVICE_SCALE
                .min(MAX_FRAME / width as f32)
                .min(MAX_FRAME / height as f32),
        };
        if self.sent == Some(metrics) {
            return;
        }
        let Some(tab) = self.tab.clone() else {
            return;
        };
        self.sent = Some(metrics);
        tab.0
            .browser
            .resize(tab.0.id, metrics.width, metrics.height, metrics.scale);
    }

    fn stream(&mut self) {
        self.thaw();
        let (Some(tab), Some(_)) = (&self.tab, self.sent) else {
            return;
        };
        if !self.streaming {
            self.streaming = true;
            tab.0.browser.set_hidden(tab.0.id, false);
            tab.0.browser.set_focus(tab.0.id, true);
        }
    }

    fn give(&mut self, generation: u64, load: &Load) {
        let Some(tab) = self.tab.clone() else {
            return;
        };
        self.given = Some((generation, load.clone()));
        self.committed = None;
        match load {
            Load::Html(html) => {
                let url = format!("{ORIGIN}{generation}");
                self.document = Some((url.clone(), html.clone()));
                tab.fire("Page.navigate", json!({ "url": url }));
            }
            Load::Url(url) => {
                self.document = None;
                tab.fire("Page.navigate", json!({ "url": url.as_ref() }));
            }
        }
    }

    fn notify(&self, cx: &mut App) {
        if let Some(notify) = self.notify.clone() {
            notify(cx);
        }
    }
}

impl Host {
    pub(super) fn new(wake: async_channel::Sender<()>, input: Input) -> Self {
        Self {
            state: Rc::default(),
            input,
            wake,
        }
    }

    /// Whether Chromium gave up on this draft.
    pub(super) fn failed(&self) -> bool {
        self.state.borrow().failed.is_some()
    }

    /// Why Chromium cannot run this draft, once it cannot.
    pub(super) fn failure(&self) -> Option<String> {
        self.state.borrow().failed.clone()
    }

    /// Starts the tab unless it is open or on its way.
    pub(super) fn open(&self, placement: &Placement) {
        {
            let state = self.state.borrow();
            if state.tab.is_some() || state.opening || state.failed.is_some() {
                return;
            }
        }
        let Some(binary) = crate::chromium::host() else {
            self.state.borrow_mut().failed = Some("Chromium is not installed".into());
            return;
        };
        HOSTS.with(|hosts| {
            let mut hosts = hosts.borrow_mut();
            if !hosts
                .iter()
                .any(|state| std::ptr::eq(state.as_ptr(), Rc::as_ptr(&self.state)))
            {
                hosts.push(Rc::downgrade(&self.state));
            }
        });
        let html = matches!(placement.load, Load::Html(_));
        let weak = Rc::downgrade(&self.state);
        let wake = self.wake.clone();
        let mut state = self.state.borrow_mut();
        state.opening = true;
        state.html = html;
        state.scrolls = !html;
        let size = (placement.width, placement.height);
        state._task = Some(placement.app.spawn(async move |cx| {
            let opened = Tab::open(&binary, html, size).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            let events = {
                let mut state = state.borrow_mut();
                state.opening = false;
                match opened {
                    Ok((tab, events, frames)) => {
                        crate::chromium::report_failure(None);
                        state.tab = Some(tab.clone());
                        // Nothing shows until a paint asks for frames.
                        tab.0.browser.set_hidden(tab.0.id, true);
                        state.sent = None;
                        state.streaming = false;
                        state.frozen = false;
                        if let Some(light) = state.appearance {
                            tab.fire("Emulation.setEmulatedMedia", media(Some(light)));
                        }
                        // The agent may be waiting on a page whose card has
                        // never been painted: lay it out and load it now.
                        let (width, height) = state.layout;
                        if width > 0 && height > 0 {
                            state.apply(width, height);
                        }
                        if let Some((generation, load)) = state.wanted.clone() {
                            state.give(generation, &load);
                        }
                        // Nothing shows it yet; a paint or a lease runs it.
                        state.freeze();
                        Some((tab, events, frames))
                    }
                    Err(error) => {
                        state.failures += 1;
                        if state.failures >= MAX_FAILURES {
                            crate::chromium::report_failure(Some(error.clone()));
                            state.failed = Some(error);
                        }
                        None
                    }
                }
            };
            cx.update(|cx| state.borrow().notify(cx));
            drop(state);
            if let Some((tab, events, frames)) = events {
                let shown = weak.clone();
                let _frames = cx.spawn(async move |cx| show_frames(shown, frames, cx).await);
                run(weak, tab, events, wake, cx).await;
            }
        }));
    }

    /// Keeps the page running for the agent until the lease drops, opening
    /// and loading it if its card was never painted.
    pub(super) fn lease(&self, placement: &Placement) -> Lease {
        {
            let mut state = self.state.borrow_mut();
            state.leases += 1;
            state.layout = (placement.width, placement.height);
            state.wanted = Some((placement.generation, placement.load.clone()));
            state.thaw();
        }
        self.open(placement);
        self.refresh(placement);
        Lease(self.state.clone())
    }

    /// Gives an open tab a newer version of the draft now, rather than at the
    /// card's next paint, or once it opens.
    pub(super) fn refresh(&self, placement: &Placement) {
        let mut state = self.state.borrow_mut();
        state.wanted = Some((placement.generation, placement.load.clone()));
        if state.tab.is_some()
            && state
                .given
                .as_ref()
                .is_none_or(|(generation, _)| *generation != placement.generation)
        {
            state.give(placement.generation, &placement.load);
        }
    }

    pub(super) fn tab(&self) -> Option<Tab> {
        self.state.borrow().tab.clone()
    }

    /// Where the page was last drawn in the window, in points: x, y, width,
    /// height, and whether its frames are stopped.
    #[cfg(debug_assertions)]
    pub(super) fn placement(&self) -> Option<[f64; 5]> {
        let state = self.state.borrow();
        let (_, _, visible) = state.drawn?;
        Some([
            f64::from(visible.origin.x),
            f64::from(visible.origin.y),
            f64::from(visible.size.width),
            f64::from(visible.size.height),
            f64::from(u8::from(!state.streaming)),
        ])
    }

    #[cfg(test)]
    pub(super) fn painted_in(&self, window: gpui::WindowId) -> bool {
        self.state.borrow().painted == Some((window, EPOCH.with(std::cell::Cell::get)))
    }

    pub(super) fn column(&self) -> Option<f32> {
        self.state.borrow().area.map(|(width, _, _)| width)
    }

    /// The page is running and has a document to read.
    pub(super) fn visible(&self) -> bool {
        let state = self.state.borrow();
        state.tab.is_some() && state.committed.is_some()
    }

    pub(super) fn hide(&self) {
        self.state.borrow_mut().stop_streaming();
    }

    /// Lays the page out for a new viewport now rather than at the next
    /// paint, so a picture taken straight after is of the new size.
    pub(super) fn resize(&self, width: u32, height: u32) {
        self.state.borrow_mut().apply(width, height);
    }

    pub(super) fn measure_at(&self, viewport: Option<(u32, u32)>, own: (u32, u32)) {
        self.state.borrow_mut().measuring = viewport;
        let (width, height) = viewport.unwrap_or(own);
        self.resize(width, height);
    }

    pub(super) fn measuring(&self) -> bool {
        self.state.borrow().measuring.is_some()
    }

    pub(super) fn set_scrolls(&self, scrolls: bool) {
        self.state.borrow_mut().scrolls = scrolls;
    }

    pub(super) fn set_appearance(&self, light: Option<bool>) {
        let mut state = self.state.borrow_mut();
        state.appearance = light;
        if let Some(tab) = &state.tab {
            tab.fire("Emulation.setEmulatedMedia", media(light));
        }
    }

    pub(super) fn has_loaded(&self, generation: u64) -> bool {
        self.state.borrow().committed == Some(generation)
    }
}

fn media(light: Option<bool>) -> Value {
    match light {
        Some(light) => json!({"features": [{"name": "prefers-color-scheme",
            "value": if light { "light" } else { "dark" }}]}),
        None => json!({"features": []}),
    }
}

/// Handles the tab's events until it closes: frames, the page's wake and
/// cursor, what it may load, and dialogs nobody will see.
async fn run(
    weak: Weak<RefCell<State>>,
    tab: Tab,
    events: async_channel::Receiver<Value>,
    wake: async_channel::Sender<()>,
    cx: &mut gpui::AsyncApp,
) {
    while let Ok(event) = events.recv().await {
        let Some(state) = weak.upgrade() else {
            return;
        };
        let params = &event["params"];
        match event["method"].as_str().unwrap_or_default() {
            "Host.popup" => {
                // No windows: a link the person opened in a new one goes to
                // their browser instead.
                let url = params["url"].as_str().unwrap_or_default();
                let web = url.starts_with("http://") || url.starts_with("https://");
                if params["userGesture"] == true && web && !url.starts_with(ORIGIN) {
                    let url = url.to_owned();
                    cx.update(|cx| cx.open_url(&url));
                }
            }
            "Runtime.bindingCalled" => match params["name"].as_str() {
                Some(WAKE) => {
                    let _ = wake.try_send(());
                }
                Some(CURSOR) => {
                    let cursor = match params["payload"].as_str() {
                        Some("pointer") => CursorStyle::PointingHand,
                        Some("text") => CursorStyle::IBeam,
                        Some("grab") => CursorStyle::OpenHand,
                        Some("grabbing") => CursorStyle::ClosedHand,
                        Some("not-allowed") => CursorStyle::OperationNotAllowed,
                        Some("crosshair") => CursorStyle::Crosshair,
                        Some("col-resize" | "ew-resize") => CursorStyle::ResizeLeftRight,
                        Some("row-resize" | "ns-resize") => CursorStyle::ResizeUpDown,
                        _ => CursorStyle::Arrow,
                    };
                    let changed = {
                        let mut state = state.borrow_mut();
                        std::mem::replace(&mut state.cursor, cursor) != cursor
                    };
                    if changed {
                        cx.update(|cx| state.borrow().notify(cx));
                    }
                }
                _ => {}
            },
            "Runtime.executionContextCreated" => {
                let context = &params["context"];
                if context["auxData"]["isDefault"] == true
                    && let Some(id) = context["id"].as_i64()
                {
                    state.borrow_mut().contexts.push(id);
                }
            }
            "Runtime.executionContextDestroyed" => {
                let id = params["executionContextId"].as_i64();
                state
                    .borrow_mut()
                    .contexts
                    .retain(|context| Some(*context) != id);
            }
            "Runtime.executionContextsCleared" => state.borrow_mut().contexts.clear(),
            "Fetch.requestPaused" => {
                let url = params["request"]["url"].as_str().unwrap_or_default();
                let id = params["requestId"].clone();
                let top = params["frameId"].as_str() == Some(tab.0.frame.as_str());
                let state = state.borrow();
                let page = state
                    .document
                    .as_ref()
                    .filter(|(served, _)| top && served == url);
                if let Some((_, html)) = page {
                    use base64::Engine as _;
                    tab.fire(
                        "Fetch.fulfillRequest",
                        json!({"requestId": id, "responseCode": 200,
                               "responseHeaders": [{"name": "Content-Type", "value": "text/html; charset=utf-8"}],
                               "body": base64::engine::general_purpose::STANDARD.encode(html.as_bytes())}),
                    );
                } else if !state.html && super::web::is_local(url) {
                    tab.fire("Fetch.continueRequest", json!({ "requestId": id }));
                } else {
                    tab.fire(
                        "Fetch.failRequest",
                        json!({"requestId": id, "errorReason": "BlockedByClient"}),
                    );
                }
            }
            "Page.frameRequestedNavigation" => {
                // Links to anywhere but the draft itself open in the person's
                // browser.
                let url = params["url"].as_str().unwrap_or_default();
                let web = url.starts_with("http://") || url.starts_with("https://");
                let ours =
                    url.starts_with(ORIGIN) || !state.borrow().html && super::web::is_local(url);
                if params["reason"] == "anchorClick" && web && !ours {
                    let url = url.to_owned();
                    cx.update(|cx| cx.open_url(&url));
                }
            }
            "Page.frameNavigated" => {
                if params["frame"]["parentId"].is_null() {
                    let mut state = state.borrow_mut();
                    state.committed = state.given.as_ref().map(|(generation, _)| *generation);
                }
            }
            "Page.javascriptDialogOpening" => {
                // Nobody sees the dialog: answer it the way closing it would.
                let accept = matches!(params["type"].as_str(), Some("alert" | "beforeunload"));
                tab.fire("Page.handleJavaScriptDialog", json!({ "accept": accept }));
            }
            _ => {}
        }
    }
    // Chromium closed the tab or quit: the next paint opens another and loads
    // the page again.
    if let Some(state) = weak.upgrade() {
        {
            let mut state = state.borrow_mut();
            state.tab = None;
            state.given = None;
            state.committed = None;
            state.sent = None;
            state.streaming = false;
            state.contexts.clear();
            state.failures += 1;
            if state.failures >= MAX_FAILURES {
                let reason = tab.0.browser.went_away();
                crate::chromium::report_failure(Some(reason.clone()));
                state.failed = Some(reason);
            }
        }
        cx.update(|cx| state.borrow().notify(cx));
    }
}

/// Shows the tab's frames as they arrive, until it closes. A card that has
/// not drawn the page for a while stops them.
async fn show_frames(
    weak: Weak<RefCell<State>>,
    frames: async_channel::Receiver<cdp::Frame>,
    cx: &mut gpui::AsyncApp,
) {
    while let Ok(frame) = frames.recv().await {
        let Some(state) = weak.upgrade() else {
            return;
        };
        {
            let mut s = state.borrow_mut();
            // A page laid out at another width to measure it is not what the
            // card shows.
            if s.measuring.is_some() {
                continue;
            }
            if s.last_paint
                .is_none_or(|at| at.elapsed() > Duration::from_secs(1))
            {
                s.stop_streaming();
            }
            let scale = s.sent.map_or(DEVICE_SCALE, |sent| sent.scale);
            s.frame = Some(Frame {
                buffer: frame.buffer,
                size: (frame.size.0 as f32 / scale, frame.size.1 as f32 / scale),
            });
        }
        cx.update(|cx| state.borrow().notify(cx));
    }
}

/// Paints the draft from the tab's latest frame and passes the person's mouse
/// and keys to the page.
pub(super) fn surface<T: 'static>(
    host: &Host,
    placement: Placement,
    window: &mut Window,
    cx: &mut gpui::Context<T>,
) -> gpui::AnyElement {
    host.open(&placement);
    let focus = {
        let mut state = host.state.borrow_mut();
        if state.notify.is_none() {
            let this = cx.entity().downgrade();
            state.notify = Some(Rc::new(move |cx: &mut App| {
                let _ = this.update(cx, |_, cx| cx.notify());
            }));
        }
        state.layout = (placement.width, placement.height);
        state.wanted = Some((placement.generation, placement.load.clone()));
        if state.tab.is_some() {
            if state
                .given
                .as_ref()
                .is_none_or(|(generation, _)| *generation != placement.generation)
            {
                state.give(placement.generation, &placement.load);
            } else if let (Some(script), Some(tab)) = (&placement.retheme, &state.tab) {
                tab.fire("Runtime.evaluate", json!({ "expression": script }));
            }
        }
        state.focus.get_or_insert_with(|| cx.focus_handle()).clone()
    };
    let alpha = placement.revealed.map_or(0., |at| {
        let t = (at.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.);
        1. - (1. - t).powi(3)
    });
    if alpha < 1. && placement.revealed.is_some() {
        window.request_animation_frame();
    }
    let state = host.state.clone();
    let input = host.input.clone();
    let paint_focus = focus.clone();
    let paint = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            paint(&state, &input, &paint_focus, bounds, &hitbox, window)
        },
    )
    .absolute()
    .size_full();
    let keys = host.state.clone();
    let releases = host.state.clone();
    let typed = host.input.clone();
    div()
        .id("draft-chromium")
        .relative()
        .size_full()
        .opacity(alpha)
        .track_focus(&focus)
        .on_key_down(move |event: &KeyDownEvent, _, cx| {
            typed.set(Some(Instant::now()));
            if key_down(&keys, event, cx) {
                cx.stop_propagation();
            }
        })
        .on_key_up(move |event: &KeyUpEvent, _, cx| {
            if key_up(&releases, event) {
                cx.stop_propagation();
            }
        })
        .child(paint)
        .into_any_element()
}

fn paint(
    state: &Rc<RefCell<State>>,
    input: &Input,
    focus: &FocusHandle,
    bounds: Bounds<Pixels>,
    hitbox: &gpui::Hitbox,
    window: &mut Window,
) {
    let mut s = state.borrow_mut();
    s.last_paint = Some(Instant::now());
    s.painted = Some((
        Window::window_handle(window).window_id(),
        EPOCH.with(std::cell::Cell::get),
    ));
    let zoom = window.content_zoom();
    let available = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    s.area = Some((available.0, available.1, zoom));
    let (width, height, scale) = match s.measuring {
        Some((width, height)) => (width, height, 1.),
        None => (s.layout.0, s.layout.1, fit(s.layout.0, available).scale),
    };
    let shown = gpui::size(px(width as f32 * scale), px(height as f32 * scale));
    let draft = Bounds::new(
        gpui::point(
            bounds.left() + (bounds.size.width - shown.width) / 2.,
            bounds.top(),
        ),
        shown,
    );
    let visible = draft.intersect(&window.content_mask().bounds);
    if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
        s.drawn = None;
        s.stop_streaming();
        return;
    }
    s.drawn = Some((draft.origin, scale, visible));
    if s.measuring.is_none() {
        s.apply(width, height);
        s.stream();
    }
    if let Some(frame) = &s.frame {
        let image_bounds = Bounds::new(
            draft.origin,
            gpui::size(px(frame.size.0 * scale), px(frame.size.1 * scale)),
        );
        let buffer = frame.buffer.clone();
        window.with_content_mask(Some(ContentMask { bounds: visible }), |window| {
            window.paint_surface(image_bounds, buffer)
        });
    }
    window.set_cursor_style(s.cursor, hitbox);
    drop(s);

    let listen = |state: &Rc<RefCell<State>>| state.clone();
    window.on_mouse_event({
        let (state, hitbox, focus, input) =
            (listen(state), hitbox.clone(), focus.clone(), input.clone());
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            window.focus(&focus, cx);
            input.set(Some(Instant::now()));
            state.borrow_mut().pressed = true;
            mouse(
                &state,
                "mousePressed",
                event.position,
                Some(event.button),
                event.click_count,
                &event.modifiers,
            );
            cx.stop_propagation();
        }
    });
    window.on_mouse_event({
        let (state, hitbox) = (listen(state), hitbox.clone());
        move |event: &MouseUpEvent, phase, window, _| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let pressed = std::mem::take(&mut state.borrow_mut().pressed);
            if pressed || hitbox.is_hovered(window) {
                mouse(
                    &state,
                    "mouseReleased",
                    event.position,
                    Some(event.button),
                    event.click_count.max(1),
                    &event.modifiers,
                );
            }
        }
    });
    window.on_mouse_event({
        let (state, hitbox) = (listen(state), hitbox.clone());
        move |event: &MouseMoveEvent, phase, window, _| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let over = hitbox.is_hovered(window);
            let (pressed, was) = {
                let mut s = state.borrow_mut();
                (s.pressed, std::mem::replace(&mut s.hovered, over))
            };
            if over || pressed {
                mouse(
                    &state,
                    "mouseMoved",
                    event.position,
                    event.pressed_button,
                    0,
                    &event.modifiers,
                );
            } else if was {
                // The pointer left: the page's hover styles end too.
                let s = state.borrow();
                if let Some(tab) = &s.tab {
                    tab.fire(
                        "Input.dispatchMouseEvent",
                        json!({"type": "mouseMoved", "x": -1, "y": -1}),
                    );
                }
            }
        }
    });
    window.on_mouse_event({
        let (state, hitbox) = (listen(state), hitbox.clone());
        move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || !hitbox.should_handle_scroll(window)
                || !state.borrow().scrolls
            {
                return;
            }
            let Some((x, y)) = local(&state.borrow(), event.position) else {
                return;
            };
            let delta = event.delta.pixel_delta(px(16.));
            if let Some(tab) = &state.borrow().tab {
                tab.fire(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": x, "y": y,
                           "deltaX": -f32::from(delta.x), "deltaY": -f32::from(delta.y),
                           "modifiers": flags(&event.modifiers)}),
                );
            }
            cx.stop_propagation();
        }
    });
}

/// A point in the window as a point in the page, in CSS pixels.
fn local(state: &State, position: Point<Pixels>) -> Option<(f32, f32)> {
    let (origin, scale, _) = state.drawn?;
    Some((
        f32::from(position.x - origin.x) / scale,
        f32::from(position.y - origin.y) / scale,
    ))
}

fn mouse(
    state: &Rc<RefCell<State>>,
    kind: &str,
    position: Point<Pixels>,
    button: Option<MouseButton>,
    clicks: usize,
    modifiers: &Modifiers,
) {
    let state = state.borrow();
    let (Some(tab), Some((x, y))) = (&state.tab, local(&state, position)) else {
        return;
    };
    if state.measuring.is_some() {
        return;
    }
    let (button, buttons) = match button {
        Some(MouseButton::Left) => ("left", 1),
        Some(MouseButton::Right) => ("right", 2),
        Some(MouseButton::Middle) => ("middle", 4),
        _ => ("none", 0),
    };
    tab.fire(
        "Input.dispatchMouseEvent",
        json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons,
               "clickCount": clicks, "modifiers": flags(modifiers)}),
    );
}

/// DevTools modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8.
fn flags(modifiers: &Modifiers) -> u8 {
    u8::from(modifiers.alt)
        | u8::from(modifiers.control) << 1
        | u8::from(modifiers.platform) << 2
        | u8::from(modifiers.shift) << 3
}

/// Keys DevTools needs named, with their Windows key codes.
fn special(key: &str) -> Option<(&'static str, u32, Option<&'static str>)> {
    Some(match key {
        "backspace" => ("Backspace", 8, None),
        "enter" => ("Enter", 13, Some("\r")),
        "tab" => ("Tab", 9, None),
        "escape" => ("Escape", 27, None),
        "left" => ("ArrowLeft", 37, None),
        "up" => ("ArrowUp", 38, None),
        "right" => ("ArrowRight", 39, None),
        "down" => ("ArrowDown", 40, None),
        "delete" => ("Delete", 46, None),
        "home" => ("Home", 36, None),
        "end" => ("End", 35, None),
        "pageup" => ("PageUp", 33, None),
        "pagedown" => ("PageDown", 34, None),
        _ => return None,
    })
}

/// Passes a key press to the page. Returns whether the page took it; the
/// app's own shortcuts pass through.
fn key_down(state: &Rc<RefCell<State>>, event: &KeyDownEvent, cx: &mut App) -> bool {
    let Some(tab) = state.borrow().tab.clone() else {
        return false;
    };
    let stroke = &event.keystroke;
    let modifiers = flags(&stroke.modifiers);
    let repeat = event.is_held;
    if let Some((key, code, text)) = special(&stroke.key) {
        let mut down = json!({"type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key, "code": key, "windowsVirtualKeyCode": code,
            "modifiers": modifiers, "autoRepeat": repeat});
        if let Some(text) = text {
            down["text"] = text.into();
        }
        tab.fire("Input.dispatchKeyEvent", down);
        return true;
    }
    if stroke.modifiers.platform {
        match stroke.key.as_str() {
            "c" | "x" => {
                let cut = stroke.key == "x";
                let contexts = state.borrow().contexts.clone();
                cx.spawn(async move |cx| {
                    for context in contexts {
                        let reply = tab.send(
                            "Runtime.evaluate",
                            json!({"expression": SELECTION_SCRIPT, "contextId": context, "returnByValue": true}),
                        );
                        let text = cdp::within(reply, Duration::from_secs(1), cx)
                            .await
                            .and_then(|value| value["result"]["value"].as_str().map(str::to_owned))
                            .filter(|text| !text.is_empty());
                        if let Some(text) = text {
                            cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
                            if cut {
                                tab.fire(
                                    "Input.dispatchKeyEvent",
                                    json!({"type": "rawKeyDown", "key": "x", "modifiers": modifiers, "commands": ["cut"]}),
                                );
                            }
                            return;
                        }
                    }
                })
                .detach();
                true
            }
            "v" => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    tab.fire("Input.insertText", json!({ "text": text }));
                }
                true
            }
            "a" | "z" => {
                let command = match (stroke.key.as_str(), stroke.modifiers.shift) {
                    ("a", _) => "selectAll",
                    ("z", true) => "redo",
                    _ => "undo",
                };
                tab.fire(
                    "Input.dispatchKeyEvent",
                    json!({"type": "rawKeyDown", "key": stroke.key, "modifiers": modifiers, "commands": [command]}),
                );
                true
            }
            _ => false,
        }
    } else if let Some(text) = stroke.key_char.as_deref() {
        tab.fire(
            "Input.dispatchKeyEvent",
            json!({"type": "keyDown", "key": text, "text": text, "unmodifiedText": text,
                   "modifiers": modifiers, "autoRepeat": repeat}),
        );
        true
    } else {
        false
    }
}

fn key_up(state: &Rc<RefCell<State>>, event: &KeyUpEvent) -> bool {
    let Some(tab) = state.borrow().tab.clone() else {
        return false;
    };
    let stroke = &event.keystroke;
    if stroke.modifiers.platform {
        return false;
    }
    let modifiers = flags(&stroke.modifiers);
    if let Some((key, code, _)) = special(&stroke.key) {
        tab.fire(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": key, "code": key, "windowsVirtualKeyCode": code, "modifiers": modifiers}),
        );
        return true;
    }
    let key = stroke.key_char.as_deref().unwrap_or(&stroke.key);
    tab.fire(
        "Input.dispatchKeyEvent",
        json!({"type": "keyUp", "key": key, "modifiers": modifiers}),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_carry_their_codes_and_enter_types_a_return() {
        assert_eq!(special("enter"), Some(("Enter", 13, Some("\r"))));
        assert_eq!(
            special("left").map(|(key, code, _)| (key, code)),
            Some(("ArrowLeft", 37))
        );
        assert_eq!(special("a"), None);
    }

    #[test]
    fn modifiers_map_to_devtools_bits() {
        let modifiers = Modifiers {
            platform: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(flags(&modifiers), 4 | 8);
    }

    #[test]
    fn color_scheme_follows_the_appearance_or_the_default() {
        assert_eq!(media(Some(false))["features"][0]["value"], "dark");
        assert_eq!(media(None)["features"], json!([]));
    }
}
