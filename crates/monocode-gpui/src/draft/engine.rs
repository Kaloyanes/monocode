//! Which engine draws a draft: WebKit, built into macOS and the default, or
//! Chromium, downloaded when the person picks it in Settings › Visualizations
//! › Page engine. Open drafts move when the choice changes, and one whose
//! Chromium cannot start goes back to WebKit.
use super::{chrome, web};
use gpui::{AnyWindowHandle, Context, Window, prelude::*};
use objc2::rc::Retained;
use objc2_web_kit::WKWebView;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

pub(super) use web::{Load, Placement};

/// A draft's page, for the tasks that read and drive it.
#[derive(Clone)]
pub(super) enum Page {
    WebKit(Retained<WKWebView>),
    Chrome(chrome::Tab),
}

/// The page of one draft in whichever engine runs it. Both engines share the
/// wake channel and the record of the person's input, so a draft that falls
/// back to WebKit keeps both.
#[derive(Clone)]
pub(super) struct Host {
    webkit: web::Host,
    chrome: Option<chrome::Host>,
    agent: Rc<Agent>,
}

/// The agent's leases on a Chromium draft. Once Chromium gives up, a WebKit
/// lease in the same window stands in for them until the last one drops.
#[derive(Default)]
pub(crate) struct Agent {
    leases: Cell<u32>,
    window: Cell<Option<AnyWindowHandle>>,
    fallback: RefCell<Option<web::Lease>>,
}

/// While held, the agent is working with the page; see `web::Lease`.
#[allow(dead_code)]
pub(crate) enum Lease {
    WebKit(web::Lease),
    Chrome(chrome::Lease, Rc<Agent>),
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Lease::Chrome(_, agent) = self {
            agent.leases.set(agent.leases.get().saturating_sub(1));
            if agent.leases.get() == 0 {
                let fallback = agent.fallback.borrow_mut().take();
                drop(fallback);
            }
        }
    }
}

/// Whether a draft opened now runs in Chromium: the person chose it and it
/// is downloaded. Debug builds take `MONOCODE_DRAFT_ENGINE=chromium` too, so
/// the self-test can run either engine.
pub(super) fn chromium_chosen(cx: &gpui::App) -> bool {
    #[cfg(debug_assertions)]
    if let Ok(engine) = std::env::var("MONOCODE_DRAFT_ENGINE") {
        return engine == "chromium" && crate::chromium::installed();
    }
    cx.try_global::<crate::settings::Preferences>()
        .is_some_and(|prefs| prefs.page_engine == "chromium")
        && crate::chromium::installed()
}

impl Host {
    pub(super) fn new(chromium: bool) -> Self {
        let webkit = web::Host::default();
        let chrome = chromium.then(|| chrome::Host::new(webkit.wake_sender(), webkit.input_cell()));
        Self {
            webkit,
            chrome,
            agent: Rc::default(),
        }
    }

    /// Whether this was made for Chromium, even if it went back to WebKit.
    pub(super) fn made_for_chromium(&self) -> bool {
        self.chrome.is_some()
    }

    /// The Chromium tab, unless this draft runs in WebKit or went back to it.
    fn chrome(&self) -> Option<&chrome::Host> {
        self.chrome.as_ref().filter(|chrome| !chrome.failed())
    }

    pub(super) fn lease(&self, placement: &Placement, window: &Window) -> Lease {
        match self.chrome() {
            Some(chrome) => {
                let agent = &self.agent;
                agent.leases.set(agent.leases.get() + 1);
                agent.window.set(Some(window.window_handle()));
                Lease::Chrome(chrome.lease(placement), agent.clone())
            }
            None => Lease::WebKit(self.webkit.lease(placement, window)),
        }
    }

    /// The window the agent's lease asked for while a Chromium draft has no
    /// page, so it can be kept: see `keep_leased`.
    pub(super) fn leased_without_page(&self) -> Option<AnyWindowHandle> {
        let chrome = self.chrome.as_ref()?;
        let waiting = if chrome.failed() {
            self.agent.fallback.borrow().is_none()
        } else {
            chrome.tab().is_none()
        };
        (self.agent.leases.get() > 0 && waiting)
            .then(|| self.agent.window.get())
            .flatten()
    }

    /// Keeps the agent's lease good without a card on screen: a Chromium
    /// start that failed is tried again, and once Chromium gives up the page
    /// opens in WebKit, parked in the leased window.
    pub(super) fn keep_leased(&self, placement: &Placement, window: &Window) {
        let Some(chrome) = &self.chrome else {
            return;
        };
        if self.agent.leases.get() == 0 {
            return;
        }
        if chrome.failed() {
            let mut fallback = self.agent.fallback.borrow_mut();
            if fallback.is_none() {
                *fallback = Some(self.webkit.lease(placement, window));
            }
        } else {
            chrome.open(placement);
        }
    }

    /// Whether a WebKit lease stands in for the agent's Chromium leases.
    #[cfg(test)]
    pub(super) fn standing_in(&self) -> bool {
        self.agent.fallback.borrow().is_some()
    }

    /// Why Chromium could not run this draft, once it went back to WebKit.
    pub(super) fn failure(&self) -> Option<String> {
        self.chrome.as_ref()?.failure()
    }
    /// Gives an open page a newer version of the draft now, rather than at
    /// the card's next paint.
    pub(super) fn refresh(&self, placement: &Placement) {
        match self.chrome() {
            Some(chrome) => chrome.refresh(placement),
            None => self.webkit.refresh(placement),
        }
    }
    pub(super) fn woken(&self) -> async_channel::Receiver<()> {
        self.webkit.woken()
    }
    pub(super) fn wake(&self) {
        self.webkit.wake();
    }
    pub(super) fn waker(&self) -> impl Fn() + Send + 'static {
        self.webkit.waker()
    }
    pub(super) fn input(&self) -> Option<std::time::Instant> {
        self.webkit.input()
    }
    #[cfg(debug_assertions)]
    pub(super) fn simulate_input(&self) {
        self.webkit.simulate_input();
    }
    pub(super) fn web(&self) -> Option<Page> {
        match self.chrome() {
            Some(chrome) => chrome.tab().map(Page::Chrome),
            None => self.webkit.web().map(Page::WebKit),
        }
    }
    #[cfg(debug_assertions)]
    pub(super) fn placement(&self) -> Option<[f64; 5]> {
        match self.chrome() {
            Some(chrome) => chrome.placement(),
            None => self.webkit.placement(),
        }
    }
    #[cfg(test)]
    pub(super) fn painted_in(&self, window: gpui::WindowId) -> bool {
        match self.chrome() {
            Some(chrome) => chrome.painted_in(window),
            None => self.webkit.painted_in(window),
        }
    }
    pub(super) fn column(&self) -> Option<f32> {
        match self.chrome() {
            Some(chrome) => chrome.column(),
            None => self.webkit.column(),
        }
    }
    pub(super) fn visible(&self) -> bool {
        match self.chrome() {
            Some(chrome) => chrome.visible(),
            None => self.webkit.visible(),
        }
    }
    pub(super) fn hide(&self) {
        match self.chrome() {
            Some(chrome) => chrome.hide(),
            None => self.webkit.hide(),
        }
    }
    pub(super) fn resize(&self, width: u32, height: u32) {
        match self.chrome() {
            Some(chrome) => chrome.resize(width, height),
            None => self.webkit.resize(width, height),
        }
    }
    pub(super) fn measure_at(&self, viewport: Option<(u32, u32)>, own: (u32, u32)) {
        match self.chrome() {
            Some(chrome) => chrome.measure_at(viewport, own),
            None => self.webkit.measure_at(viewport, own),
        }
    }
    pub(super) fn measuring(&self) -> bool {
        match self.chrome() {
            Some(chrome) => chrome.measuring(),
            None => self.webkit.measuring(),
        }
    }
    pub(super) fn set_scrolls(&self, scrolls: bool) {
        match self.chrome() {
            Some(chrome) => chrome.set_scrolls(scrolls),
            None => self.webkit.set_scrolls(scrolls),
        }
    }
    pub(super) fn set_appearance(&self, light: Option<bool>) {
        match self.chrome() {
            Some(chrome) => chrome.set_appearance(light),
            None => self.webkit.set_appearance(light),
        }
    }
    pub(super) fn has_loaded(&self, generation: u64) -> bool {
        match self.chrome() {
            Some(chrome) => chrome.has_loaded(generation),
            None => self.webkit.has_loaded(generation),
        }
    }
}

/// Paints the draft with its engine.
pub(super) fn surface<T: 'static>(
    host: &Host,
    placement: Placement,
    window: &mut Window,
    cx: &mut Context<T>,
) -> gpui::AnyElement {
    match host.chrome() {
        Some(chrome) => chrome::surface(chrome, placement, window, cx),
        None => web::surface(&host.webkit, placement, window, cx),
    }
}

/// Put last in a window's root: pages whose cards were not painted step
/// aside (WebKit) or stop their frames (Chromium).
pub(crate) fn sentinel() -> gpui::AnyElement {
    gpui::div()
        .absolute()
        .size_0()
        .child(web::sentinel())
        .child(chrome::sentinel())
        .into_any_element()
}

/// Runs `script` in the page's top document and returns its string result.
pub(super) async fn evaluate(page: &Page, script: &str, cx: &gpui::AsyncApp) -> Option<String> {
    match page {
        Page::WebKit(web) => web::evaluate(web, script, cx).await,
        Page::Chrome(tab) => chrome::evaluate(tab, script, cx).await,
    }
}

/// The page as a PNG, `width` points wide.
pub(super) async fn png(
    page: &Page,
    width: f64,
    cx: &gpui::AsyncApp,
) -> Result<(Vec<u8>, (u32, u32)), String> {
    match page {
        Page::WebKit(web) => web::png(web, width, cx).await,
        Page::Chrome(tab) => chrome::png(tab, width, cx).await,
    }
}

/// A picture of the page as it is now, for the card to show while the next
/// version loads.
pub(super) async fn picture(page: &Page, cx: &gpui::AsyncApp) -> Option<Arc<gpui::RenderImage>> {
    match page {
        Page::WebKit(web) => web::picture(web).await,
        Page::Chrome(tab) => chrome::picture(tab, cx).await,
    }
}
