//! A draft: a live page the agent builds inside the conversation, as a card at
//! the point in the thread where it started. The page is a web view inside the
//! app's own window, so it is visible to the person the whole time and never a
//! stray window.
//!
//! The agent reads it through screenshots and drives it with script events.
//! Those are untrusted events, so they cannot move the person's focus or type
//! into the composer, and a real click or keystroke in the page is how the app
//! knows the person took over.
mod page;
#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) mod selftest;
#[cfg(target_os = "macos")]
mod web;

use crate::{right_panel::icon_action, theme::*};
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, Global, SharedString, WeakEntity, Window, div,
    prelude::*, px,
};
use monocode_core::visual_bridge::{
    DRAFT_DEFAULT_HEIGHT, DRAFT_DEFAULT_WIDTH, DraftAction, DraftContent, DraftDiagnostics,
    DraftElement, DraftFeedback, DraftLog, DraftReply, DraftRequest, DraftSnapshot,
};
use page::Palette;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// How long a page may take to load before it is reported as it is.
#[cfg(target_os = "macos")]
const LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
/// Entrance animations and late layout settle before a first picture.
#[cfg(target_os = "macos")]
const SETTLE: std::time::Duration = std::time::Duration::from_millis(700);
/// After the agent's own click or typing, effects only need a moment.
#[cfg(target_os = "macos")]
const AFTER_ACTION: std::time::Duration = std::time::Duration::from_millis(300);
/// Pictures are taken at most this many points wide; the display adds its scale.
#[cfg(target_os = "macos")]
const MAX_PICTURE_WIDTH: u32 = 1000;

/// The tallest and shortest a draft's card gets when it follows its page.
#[cfg(target_os = "macos")]
const MIN_HEIGHT: u32 = 120;
#[cfg(target_os = "macos")]
const MAX_HEIGHT: u32 = 1600;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

type Inbox = (
    async_channel::Sender<DraftRequest>,
    async_channel::Receiver<DraftRequest>,
);
fn channel() -> &'static Inbox {
    static CHANNEL: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
    CHANNEL.get_or_init(async_channel::unbounded)
}

/// Routes the agent's draft tools to the app. Called once at startup.
pub(crate) fn start() {
    let sender = channel().0.clone();
    monocode_core::visual_bridge::set_draft_handler(move |request| {
        let _ = sender.send_blocking(request);
    });
}

/// Requests from the agent, for the shell to run on the main thread.
pub(crate) fn inbox() -> async_channel::Receiver<DraftRequest> {
    channel().1.clone()
}

/// The drafts that are open, by id, so the conversation's cards can find the
/// page they stand for.
#[derive(Default)]
struct Registry(std::collections::HashMap<String, WeakEntity<Draft>>);
impl Global for Registry {}

/// Makes a draft findable by its card. Cards re-render when this changes.
pub(crate) fn register(id: &str, draft: &Entity<Draft>, cx: &mut App) {
    cx.default_global::<Registry>()
        .0
        .insert(id.to_owned(), draft.downgrade());
    changed(cx);
}

pub(crate) fn unregister(id: &str, cx: &mut App) {
    if cx.default_global::<Registry>().0.remove(id).is_some() {
        changed(cx);
    }
}

/// The open draft with this id.
pub(crate) fn live(id: &str, cx: &App) -> Option<Entity<Draft>> {
    cx.try_global::<Registry>()?.0.get(id)?.upgrade()
}

/// Runs `change` whenever a draft appears, goes, or changes size.
pub(crate) fn watch<T: 'static>(
    cx: &mut Context<T>,
    change: impl FnMut(&mut T, &mut Context<T>) + 'static,
) -> gpui::Subscription {
    cx.observe_global::<Registry>(change)
}

/// Put last in the thread: hides pages whose cards are out of view.
#[cfg(target_os = "macos")]
pub(crate) fn sentinel() -> gpui::AnyElement {
    web::sentinel()
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn sentinel() -> gpui::AnyElement {
    div().into_any_element()
}

/// Tells the conversation a draft appeared, went, or changed size.
pub(crate) fn changed(cx: &mut App) {
    cx.default_global::<Registry>();
    cx.update_global::<Registry, _>(|_, _| ());
}

/// Whether a card stands for a kept draft's page being opened in place.
pub(crate) fn kept_id(page: &std::path::Path) -> String {
    format!("kept:{}", page.display())
}

/// Opens the page a kept picture was made from, in its card.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = draft, no_json)]
pub(crate) struct OpenKept {
    pub page: std::path::PathBuf,
}

pub(crate) enum DraftEvent {
    /// The person closed the draft.
    Closed,
    /// The person sent their notes. If the agent is not waiting for them, the
    /// shell delivers them as a message.
    Send,
}

pub(crate) struct Draft {
    id: String,
    /// What the card is called; the agent's title for a new draft.
    title: Option<SharedString>,
    /// The height follows the page until the agent picks one.
    auto_height: bool,
    content: DraftContent,
    /// Bumps whenever the page itself is replaced.
    generation: u64,
    width: u32,
    height: u32,
    /// The agent's chosen appearance; the app's own when unset.
    light: Option<bool>,
    palette: Palette,
    markup: Option<Arc<str>>,
    retheme: Option<String>,
    elements: Vec<DraftElement>,
    inspections: u64,
    /// The person clicked or typed in the page since the agent last resumed.
    control: bool,
    /// Comment mode: a click on the page pins a note instead of acting.
    commenting: bool,
    /// Notes pinned but not sent yet.
    pending: Vec<DraftFeedback>,
    /// Notes the person sent that the agent has not read.
    outbox: Vec<DraftFeedback>,
    /// What the person did in the page by hand since the agent last looked.
    trail: Vec<String>,
    /// Why the agent's wait should end: the person sent notes or handed back.
    signal: Option<&'static str>,
    /// The agent is waiting for the person.
    waiting: bool,
    #[cfg(target_os = "macos")]
    host: web::Host,
    #[cfg(target_os = "macos")]
    appearance: Option<bool>,
    _watch: Option<gpui::Task<()>>,
}
impl EventEmitter<DraftEvent> for Draft {}

impl Draft {
    pub(crate) fn new(
        id: Option<String>,
        session: i64,
        content: DraftContent,
        (width, height): (Option<u32>, Option<u32>),
        light: Option<bool>,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            id: id.unwrap_or_else(|| {
                format!(
                    "draft-{session}-{}",
                    NEXT_ID.fetch_add(1, Ordering::Relaxed)
                )
            }),
            title: title.map(Into::into),
            auto_height: height.is_none(),
            content,
            generation: 1,
            width: width.unwrap_or(DRAFT_DEFAULT_WIDTH),
            height: height.unwrap_or(DRAFT_DEFAULT_HEIGHT),
            light,
            palette: Palette::of(light),
            markup: None,
            retheme: None,
            elements: Vec::new(),
            inspections: 0,
            control: false,
            commenting: false,
            pending: Vec::new(),
            outbox: Vec::new(),
            trail: Vec::new(),
            signal: None,
            waiting: false,
            #[cfg(target_os = "macos")]
            host: web::Host::default(),
            #[cfg(target_os = "macos")]
            appearance: None,
            _watch: None,
        };
        this._watch = this.watch(cx);
        this
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn waiting(&self) -> bool {
        self.waiting
    }

    /// The notes and hand-done actions the agent has not seen, handed over.
    pub(crate) fn take_outbox(&mut self) -> (Vec<DraftFeedback>, Vec<String>) {
        self.signal = None;
        (
            std::mem::take(&mut self.outbox),
            std::mem::take(&mut self.trail),
        )
    }

    /// Sends the pinned notes to the agent.
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_empty() {
            return;
        }
        self.outbox.append(&mut self.pending);
        self.signal = Some("feedback");
        self.set_commenting(false, cx);
        cx.emit(DraftEvent::Send);
    }

    /// Turns comment mode on or off in the page.
    fn set_commenting(&mut self, on: bool, cx: &mut Context<Self>) {
        self.commenting = on;
        cx.notify();
        #[cfg(target_os = "macos")]
        if let Some(web) = self.host.web() {
            cx.spawn(async move |_, cx| {
                evaluate(
                    &web,
                    &format!("window.__mcSend({{__mcComment:{on}}});'ok'"),
                    cx,
                )
                .await;
            })
            .detach();
        }
    }

    /// The person is done trying it: hand the draft back to the agent.
    fn hand_back(&mut self, cx: &mut Context<Self>) {
        self.signal = Some("handed back");
        self.set_commenting(false, cx);
        self.resume(cx);
    }

    pub(crate) fn is_html(&self) -> bool {
        matches!(self.content, DraftContent::Html(_))
    }

    /// The page the agent wrote, for keeping.
    pub(crate) fn html(&self) -> Option<&str> {
        match &self.content {
            DraftContent::Html(html) => Some(html),
            DraftContent::Url(_) => None,
        }
    }

    #[cfg(all(debug_assertions, target_os = "macos"))]
    pub(crate) fn placement(&self) -> Option<[f64; 5]> {
        self.host.placement()
    }

    /// Steps the native view aside; it stays where it was last placed otherwise.
    pub(crate) fn hide(&self) {
        #[cfg(target_os = "macos")]
        self.host.hide();
    }

    pub(crate) fn title(&self) -> SharedString {
        if let Some(title) = &self.title {
            return title.clone();
        }
        match &self.content {
            DraftContent::Html(_) => "Draft".into(),
            DraftContent::Url(url) => url
                .split_once("://")
                .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(rest))
                .unwrap_or("Draft")
                .to_owned()
                .into(),
        }
    }

    /// Applies what a request changes before it acts.
    pub(crate) fn apply(&mut self, request: &DraftRequest, cx: &mut Context<Self>) {
        if let Some(content) = &request.content {
            self.content = content.clone();
            self.generation += 1;
            self.markup = None;
            self.elements.clear();
            self.control = false;
        }
        self.width = request.width.unwrap_or(self.width);
        if let Some(height) = request.height {
            self.height = height;
            self.auto_height = false;
        }
        if let Some(light) = request.light {
            self.light = Some(light);
            self.palette = Palette::of(self.light);
            self.markup = None;
            self.retheme = None;
        }
        cx.notify();
        // The card's size may have changed.
        let app: &mut App = cx;
        changed(app);
    }

    #[cfg(target_os = "macos")]
    fn load(&mut self) -> web::Load {
        match &self.content {
            DraftContent::Html(html) => {
                let palette = self.palette;
                web::Load::Html(
                    self.markup
                        .get_or_insert_with(|| page::sandbox(html, palette).into())
                        .clone(),
                )
            }
            DraftContent::Url(url) => web::Load::Url(url.as_str().into()),
        }
    }

    /// Reads the page's own report of whether the person has taken over.
    #[cfg(target_os = "macos")]
    fn watch(&self, cx: &mut Context<Self>) -> Option<gpui::Task<()>> {
        Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                let Ok(web) = this.read_with(cx, |this, _| {
                    this.host.web().filter(|_| this.host.visible())
                }) else {
                    return;
                };
                let Some(web) = web else {
                    continue;
                };
                let reading = state(&web, cx).await;
                let control = reading
                    .as_ref()
                    .is_some_and(|state| state["userControl"] == true);
                let has = |key: &str| {
                    reading
                        .as_ref()
                        .and_then(|state| state[key].as_array())
                        .is_some_and(|items| !items.is_empty())
                };
                // Take the page's notes and trail in one step so none is lost.
                let notes = if has("feedback") || has("trail") {
                    evaluate(
                        &web,
                        "JSON.stringify({f:window.__mc.feedback.splice(0),t:window.__mc.trail.splice(0)})",
                        cx,
                    )
                    .await
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                } else {
                    None
                };
                let changed = this.update(cx, |this, cx| {
                    if this.control != control {
                        this.control = control;
                        cx.notify();
                    }
                    if let Some(notes) = notes {
                        let found: Vec<DraftFeedback> =
                            serde_json::from_value(notes["f"].clone()).unwrap_or_default();
                        let acts: Vec<String> =
                            serde_json::from_value(notes["t"].clone()).unwrap_or_default();
                        this.pending.extend(found);
                        this.trail.extend(acts);
                        this.trail.truncate(40);
                        cx.notify();
                    }
                });
                if changed.is_err() {
                    return;
                }
            }
        }))
    }
    #[cfg(not(target_os = "macos"))]
    fn watch(&self, _: &mut Context<Self>) -> Option<gpui::Task<()>> {
        None
    }

    /// The person hands the draft back to the agent.
    fn resume(&mut self, cx: &mut Context<Self>) {
        self.control = false;
        cx.notify();
        #[cfg(target_os = "macos")]
        if let Some(web) = self.host.web() {
            cx.spawn(async move |_, cx| {
                evaluate(
                    &web,
                    "window.__mc&&(window.__mc.userControl=false);'ok'",
                    cx,
                )
                .await;
            })
            .detach();
        }
    }
}

impl Render for Draft {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Palette::of(self.light);
        if now != self.palette {
            self.palette = now;
            self.markup = None;
            self.retheme = Some(page::retheme_script(now));
        }
        let header = div()
            .id("draft-header")
            .h(px(32.))
            .px_3()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(content(0.08))
            .text_size(px(12.))
            .child(icon("app-window", 14.).text_color(content(0.55)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.8))
                    .child(self.title()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(format!("{} × {}", self.width, self.height)),
            )
            .when(self.waiting, |header| {
                header.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.))
                        .text_color(content(0.6))
                        .child(div().size(px(6.)).rounded_full().bg(accent_color()))
                        .child("Waiting for you"),
                )
            })
            .when(self.commenting && self.pending.is_empty(), |header| {
                header.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.55))
                        .child("Click the page to leave a note"),
                )
            })
            .child(
                div()
                    .id("draft-comment")
                    .debug_selector(|| "draft-comment".into())
                    .role(gpui::Role::Button)
                    .aria_label("Leave a note on the page")
                    .flex_shrink_0()
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(content(0.12))
                    .when(self.commenting, |d| d.bg(content(0.1)))
                    .hover(|s| s.bg(content(0.06)))
                    .cursor(gpui::CursorStyle::PointingHand)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let on = !this.commenting;
                        this.set_commenting(on, cx)
                    }))
                    .child("Comment"),
            )
            .when(!self.pending.is_empty(), |header| {
                header.child(
                    div()
                        .id("draft-send")
                        .debug_selector(|| "draft-send".into())
                        .role(gpui::Role::Button)
                        .aria_label("Send your notes to the agent")
                        .flex_shrink_0()
                        .px_2()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(accent_color())
                        .text_color(gpui::rgb(0x0b1220))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .on_click(cx.listener(|this, _, _, cx| this.send(cx)))
                        .child(format!(
                            "Send {} {}",
                            self.pending.len(),
                            if self.pending.len() == 1 {
                                "note"
                            } else {
                                "notes"
                            }
                        )),
                )
            })
            .when(self.control || self.waiting, |header| {
                header.child(
                    div()
                        .id("draft-resume")
                        .debug_selector(|| "draft-resume".into())
                        .role(gpui::Role::Button)
                        .aria_label("Hand the draft back to the agent")
                        .flex_shrink_0()
                        .px_2()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(content(0.12))
                        .hover(|s| s.bg(content(0.06)))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .on_click(cx.listener(|this, _, _, cx| this.hand_back(cx)))
                        .child("Hand back"),
                )
            })
            .child(
                icon_action("draft-close", "Close draft", "close", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DraftEvent::Closed))),
            );
        let ratio = self.width as f32 / self.height.max(1) as f32;
        let body = self.surface(window, cx);
        div().py_3().w_full().min_w_0().child(
            div()
                .id("draft-card")
                .debug_selector(|| "draft-card".into())
                .w_full()
                .max_w(px(self.width as f32 + 2.))
                .flex()
                .flex_col()
                .rounded_xl()
                .overflow_hidden()
                .border_1()
                .border_color(content(0.08))
                .bg(content(0.03))
                .child(header)
                .child(div().relative().w_full().aspect_ratio(ratio).child(body)),
        )
    }
}

#[cfg(target_os = "macos")]
impl Draft {
    fn surface(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let placement = web::Placement {
            load: self.load(),
            generation: self.generation,
            width: self.width,
            height: self.height,
            retheme: self.retheme.take().filter(|_| self.is_html()),
            app: cx.to_async(),
        };
        // Dev servers follow the window's appearance for prefers-color-scheme.
        let light = self.palette.light();
        if !self.is_html() && self.appearance != Some(light) {
            self.host.set_appearance(Some(light));
            self.appearance = Some(light);
        }
        web::surface(&self.host, placement, window, cx)
    }
}
#[cfg(not(target_os = "macos"))]
impl Draft {
    fn surface(&mut self, _: &mut Window, _: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .p_4()
            .text_size(px(12.))
            .text_color(content(0.55))
            .child("Drafts need the macOS app.")
            .into_any_element()
    }
}

/// The wrapper's state as JSON.
#[cfg(target_os = "macos")]
async fn state(
    web: &objc2::rc::Retained<objc2_web_kit::WKWebView>,
    cx: &gpui::AsyncApp,
) -> Option<serde_json::Value> {
    let text = evaluate(web, "window.__mc ? JSON.stringify(window.__mc) : ''", cx).await?;
    serde_json::from_str(&text).ok()
}

#[cfg(target_os = "macos")]
async fn evaluate(
    web: &objc2::rc::Retained<objc2_web_kit::WKWebView>,
    script: &str,
    cx: &gpui::AsyncApp,
) -> Option<String> {
    web::evaluate(web, script, cx).await
}

#[cfg(target_os = "macos")]
impl Draft {
    /// Carries out one request against the live page and reports what it looks
    /// like afterwards. Runs on the main thread, one request at a time.
    pub(crate) async fn run(
        this: &gpui::WeakEntity<Self>,
        request: &DraftRequest,
        cx: &mut gpui::AsyncApp,
    ) -> Result<DraftReply, String> {
        let gone = |_| "The draft was closed".to_string();
        let acts = matches!(
            request.action,
            DraftAction::Click { .. } | DraftAction::Type { .. }
        );
        let changes = acts
            || request.content.is_some()
            || request.width.is_some()
            || request.height.is_some()
            || request.light.is_some();
        let (control, id) = this
            .read_with(cx, |this, _| (this.control, this.id.clone()))
            .map_err(gone)?;
        if control && changes && !matches!(request.action, DraftAction::Resume) {
            return Err("The user controls this draft. Inspect it freely; resume only after they hand it back.".into());
        }
        match &request.action {
            DraftAction::Close => return Ok(DraftReply::Closed { draft: id }),
            DraftAction::Resume => {
                this.update(cx, |this, cx| {
                    this.signal = None;
                    this.resume(cx)
                })
                .map_err(gone)?;
            }
            _ => {}
        }
        this.update(cx, |this, cx| this.apply(request, cx))
            .map_err(gone)?;
        let loading = request.content.is_some();
        let web = Self::ready(this, cx).await?;
        if loading {
            Self::fit_height(this, &web, cx).await;
        }
        if let Some(light) = request.light {
            // Say so now rather than at the next frame, so the picture below
            // is of the new appearance.
            let is_html = this.read_with(cx, |this, _| this.is_html()).map_err(gone)?;
            if is_html {
                evaluate(&web, &page::retheme_script(Palette::forced(light)), cx).await;
            } else {
                this.update(cx, |this, _| this.host.set_appearance(Some(light)))
                    .map_err(gone)?;
            }
        }
        match &request.action {
            DraftAction::Click { reference } => {
                let known = this
                    .read_with(cx, |this, _| {
                        this.elements.iter().any(|e| &e.reference == reference)
                    })
                    .map_err(gone)?;
                if !known {
                    return Err("Element reference expired; inspect the draft again".into());
                }
                act(
                    &web,
                    serde_json::json!({"kind":"click","ref":reference}),
                    cx,
                )
                .await?;
            }
            DraftAction::Type { text, replace } => {
                act(
                    &web,
                    serde_json::json!({"kind":"type","text":text,"replace":replace}),
                    cx,
                )
                .await?;
            }
            _ => {}
        }
        let event = match &request.action {
            DraftAction::Wait { seconds } => Some(Self::wait(this, *seconds, cx).await?),
            _ => None,
        };
        let settle = if acts || event.is_some() || !loading && !changes {
            AFTER_ACTION
        } else {
            SETTLE
        };
        let mut snapshot = Self::capture(this, &web, settle, cx).await?;
        if let DraftAction::Keep { title } = &request.action {
            return Self::keep(this, snapshot.png, title, cx);
        }
        let (feedback, actions) = this
            .update(cx, |this, _| this.take_outbox())
            .map_err(gone)?;
        snapshot.feedback = feedback;
        snapshot.user_actions = actions;
        snapshot.event = event.map(str::to_owned);
        Ok(DraftReply::Snapshot(Box::new(snapshot)))
    }

    /// Plays the person for the self-test: the page's own report of a trusted
    /// click or a pinned note cannot be produced from outside it.
    #[cfg(debug_assertions)]
    pub(crate) async fn debug(
        this: &gpui::WeakEntity<Self>,
        what: &str,
        cx: &mut gpui::AsyncApp,
    ) -> Result<serde_json::Value, String> {
        let gone = |_| "The draft was closed".to_string();
        match what {
            "takeover" => {
                let web = Self::ready(this, cx).await?;
                evaluate(&web, "window.__mc.userControl=true;'ok'", cx).await;
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(900))
                    .await;
                Ok(serde_json::json!({"takeover": true}))
            }
            "note" => {
                let web = Self::ready(this, cx).await?;
                evaluate(
                    &web,
                    "window.__mc.feedback.push({note:'Make this blue',tag:'button',name:'Add one',x:12.5,y:30});window.__mc.trail.push('clicked \"Add one\"');'ok'",
                    cx,
                )
                .await;
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(900))
                    .await;
                let pending = this
                    .read_with(cx, |this, _| this.pending.len())
                    .map_err(gone)?;
                Ok(serde_json::json!({"pending": pending}))
            }
            "send" => {
                this.update(cx, |this, cx| this.send(cx)).map_err(gone)?;
                Ok(serde_json::json!({"sent": true}))
            }
            "send_later" => {
                let weak = this.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1200))
                        .await;
                    let _ = weak.update(cx, |this, cx| this.send(cx));
                })
                .detach();
                Ok(serde_json::json!({"sending": "soon"}))
            }
            _ => Err(format!("Unknown debug step {what}")),
        }
    }

    /// Holds until the person sends notes or hands the draft back, or the time
    /// is up. Says which.
    async fn wait(
        this: &gpui::WeakEntity<Self>,
        seconds: Option<u32>,
        cx: &mut gpui::AsyncApp,
    ) -> Result<&'static str, String> {
        let gone = |_| "The draft was closed".to_string();
        let limit = std::time::Duration::from_secs(
            seconds
                .unwrap_or(monocode_core::visual_bridge::DRAFT_DEFAULT_WAIT_SECONDS)
                .into(),
        );
        this.update(cx, |this, cx| {
            this.waiting = true;
            cx.notify();
        })
        .map_err(gone)?;
        let started = std::time::Instant::now();
        let event = loop {
            let signal = this.read_with(cx, |this, _| this.signal).map_err(gone)?;
            if let Some(signal) = signal {
                break signal;
            }
            if started.elapsed() >= limit {
                break "timeout";
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
        };
        this.update(cx, |this, cx| {
            this.waiting = false;
            cx.notify();
        })
        .map_err(gone)?;
        Ok(event)
    }

    /// A page the agent gave no height to is as tall as its content.
    async fn fit_height(
        this: &gpui::WeakEntity<Self>,
        web: &objc2::rc::Retained<objc2_web_kit::WKWebView>,
        cx: &mut gpui::AsyncApp,
    ) {
        let Ok(auto) = this.read_with(cx, |this, _| this.auto_height) else {
            return;
        };
        if !auto {
            return;
        }
        let Some(content) = state(web, cx).await.and_then(|state| state["h"].as_u64()) else {
            return;
        };
        let _ = this.update(cx, |this, cx| {
            this.height = (content as u32).clamp(MIN_HEIGHT, MAX_HEIGHT);
            cx.notify();
            let app: &mut App = cx;
            changed(app);
        });
    }

    /// Waits for the conversation to paint the draft, then for the page to load.
    async fn ready(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
    ) -> Result<objc2::rc::Retained<objc2_web_kit::WKWebView>, String> {
        let started = std::time::Instant::now();
        let web = loop {
            let found = this
                .read_with(cx, |this, _| {
                    this.host
                        .has_loaded(this.generation)
                        .then(|| this.host.web())
                        .flatten()
                })
                .map_err(|_| "The draft was closed".to_string())?;
            if let Some(web) = found {
                break web;
            }
            if started.elapsed() > std::time::Duration::from_secs(10) {
                return Err("The draft's card is not on screen. It paints where it sits in the conversation, so the user has to be looking at it: ask them to scroll to the end of the thread and try again.".into());
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(40))
                .await;
        };
        let started = std::time::Instant::now();
        loop {
            let ready = state(&web, cx)
                .await
                .is_some_and(|s| s["loaded"] == true && s["fontsReady"] == true);
            if ready || started.elapsed() > LOAD_TIMEOUT {
                // A page that never finishes usually has a blocked script or a
                // stuck request; report what there is.
                return Ok(web);
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
        }
    }

    async fn capture(
        this: &gpui::WeakEntity<Self>,
        web: &objc2::rc::Retained<objc2_web_kit::WKWebView>,
        settle: std::time::Duration,
        cx: &mut gpui::AsyncApp,
    ) -> Result<DraftSnapshot, String> {
        cx.background_executor().timer(settle).await;
        // The native view steps aside while a popover covers it, and a hidden
        // web view paints nothing.
        let started = std::time::Instant::now();
        while !this
            .read_with(cx, |this, _| this.host.visible())
            .map_err(|_| "The draft was closed".to_string())?
        {
            if started.elapsed() > std::time::Duration::from_secs(2) {
                return Err("The draft is scrolled out of view or covered by a menu; ask the user to bring it back and try again.".into());
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(60))
                .await;
        }
        let (width, height, id, inspection) = this
            .update(cx, |this, _| {
                this.host.resize(this.width, this.height);
                this.inspections += 1;
                (
                    this.width,
                    this.height,
                    this.id.clone(),
                    format!("{}-{}", this.generation, this.inspections),
                )
            })
            .map_err(|_| "The draft was closed".to_string())?;
        // The page lays out again for the new viewport before it is drawn.
        cx.background_executor()
            .timer(std::time::Duration::from_millis(150))
            .await;
        let (png, size) = web::png(web, f64::from(width.min(MAX_PICTURE_WIDTH)), cx).await?;
        let message = serde_json::json!({ "__mcInspect": inspection });
        evaluate(web, &format!("window.__mcSend({message});'sent'"), cx)
            .await
            .ok_or("The draft could not be inspected")?;
        let started = std::time::Instant::now();
        let summary = loop {
            let summary = state(web, cx).await.unwrap_or_default();
            if summary["inspection"] == inspection {
                break summary;
            }
            if started.elapsed() > std::time::Duration::from_secs(3) {
                return Err("The draft did not answer inspection".into());
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(30))
                .await;
        };
        let elements: Vec<DraftElement> = serde_json::from_value(summary["elements"].clone())
            .map_err(|e| format!("Invalid page controls: {e}"))?;
        this.update(cx, |this, _| this.elements = elements.clone())
            .map_err(|_| "The draft was closed".to_string())?;
        let content_height = summary["h"].as_u64().unwrap_or(height.into()) as u32;
        let (url_mode, control) = this
            .read_with(cx, |this, _| (!this.is_html(), this.control))
            .map_err(|_| "The draft was closed".to_string())?;
        let user_control = control || summary["userControl"] == true;
        Ok(DraftSnapshot {
            png,
            draft: id,
            width: size.0,
            height: size.1,
            content_height,
            loaded: summary["loaded"] == true,
            console: summary["log"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    Some(DraftLog {
                        level: entry["level"].as_str()?.into(),
                        text: entry["text"].as_str()?.into(),
                    })
                })
                .collect(),
            diagnostics: DraftDiagnostics {
                ready: summary["loaded"] == true && summary["fontsReady"] == true,
                viewport_width: width,
                viewport_height: height,
                horizontal_overflow: (summary["w"].as_u64().unwrap_or(0) as u32)
                    .saturating_sub(summary["vw"].as_u64().unwrap_or(width.into()) as u32),
                vertical_overflow: content_height.saturating_sub(height),
                failed_images: summary["failedImages"].as_u64().unwrap_or(0) as u32,
            },
            elements,
            text: summary["text"].as_str().unwrap_or_default().into(),
            user_control,
            url: url_mode.then(|| summary["url"].as_str().unwrap_or_default().into()),
            title: url_mode.then(|| summary["title"].as_str().unwrap_or_default().into()),
            feedback: Vec::new(),
            user_actions: Vec::new(),
            event: None,
            issues: summary["issues"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|issue| issue.as_str().map(str::to_owned))
                .collect(),
            state: summary.get("state").filter(|v| v.is_object()).cloned(),
        })
    }

    /// Writes the picture and the page where the bridge can publish them.
    fn keep(
        this: &gpui::WeakEntity<Self>,
        png: Vec<u8>,
        title: &str,
        cx: &mut gpui::AsyncApp,
    ) -> Result<DraftReply, String> {
        let (draft, html) = this
            .read_with(cx, |this, _| {
                (this.id.clone(), this.html().map(str::to_owned))
            })
            .map_err(|_| "The draft was closed".to_string())?;
        let html = html.ok_or("Running apps cannot be kept; only HTML drafts can")?;
        let dir = monocode_core::visual_bridge::draft_staging_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stem = format!("{draft}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let (png_path, html_path) = (
            dir.join(format!("{stem}.png")),
            dir.join(format!("{stem}.html")),
        );
        std::fs::write(&png_path, png).map_err(|e| e.to_string())?;
        std::fs::write(&html_path, html).map_err(|e| e.to_string())?;
        Ok(DraftReply::Kept {
            draft,
            title: title.to_owned(),
            png: png_path,
            html: html_path,
        })
    }
}

#[cfg(not(target_os = "macos"))]
impl Draft {
    pub(crate) async fn run(
        _: &gpui::WeakEntity<Self>,
        _: &DraftRequest,
        _: &mut gpui::AsyncApp,
    ) -> Result<DraftReply, String> {
        Err("Drafts need the macOS app".into())
    }
}

/// Sends a click or typing into the page and waits for it to say it is done.
#[cfg(target_os = "macos")]
async fn act(
    web: &objc2::rc::Retained<objc2_web_kit::WKWebView>,
    mut action: serde_json::Value,
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    static NEXT_ACTION: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ACTION.fetch_add(1, Ordering::Relaxed);
    action["id"] = id.into();
    evaluate(
        web,
        &format!(
            "window.__mcSend({});'sent'",
            serde_json::json!({ "__mcAct": action })
        ),
        cx,
    )
    .await
    .ok_or("The draft could not receive input")?;
    let started = std::time::Instant::now();
    loop {
        if let Some(result) = state(web, cx).await.map(|s| s["result"].clone())
            && result["id"] == id
        {
            return match result["err"].as_str().filter(|e| !e.is_empty()) {
                Some(error) => Err(error.to_owned()),
                None => Ok(()),
            };
        }
        if started.elapsed() > std::time::Duration::from_secs(3) {
            return Err("The draft did not confirm the input".into());
        }
        cx.background_executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn note(text: &str) -> DraftFeedback {
        DraftFeedback {
            note: text.into(),
            tag: "button".into(),
            name: "Go".into(),
            x: 1.,
            y: 2.,
        }
    }

    #[gpui::test]
    fn notes_wait_to_be_sent_then_go_to_the_agent_once(cx: &mut TestAppContext) {
        let draft = cx.new(|cx| {
            Draft::new(
                None,
                1,
                DraftContent::Html("<p>Hi</p>".into()),
                (None, None),
                None,
                Some("Pricing".into()),
                cx,
            )
        });
        let sent = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = sent.clone();
        let _watch = cx.update(|cx| {
            cx.subscribe(&draft, move |_, event: &DraftEvent, _| {
                if matches!(event, DraftEvent::Send) {
                    count.set(count.get() + 1);
                }
            })
        });
        draft.update(cx, |draft, cx| {
            // Nothing to send: nothing happens.
            draft.send(cx);
            draft.pending.push(note("Make this blue"));
            draft.trail.push("clicked \"Go\"".into());
            // Pinned notes are the person's own until sent.
            assert!(draft.outbox.is_empty());
            assert_eq!(draft.signal, None);
            draft.send(cx);
            assert_eq!(draft.signal, Some("feedback"));
            let (notes, actions) = draft.take_outbox();
            assert_eq!(notes.len(), 1);
            assert_eq!(actions, ["clicked \"Go\""]);
            // Handed over once.
            assert_eq!(draft.signal, None);
            assert!(draft.take_outbox().0.is_empty());
        });
        assert_eq!(sent.get(), 1);
        draft.update(cx, |draft, cx| {
            draft.hand_back(cx);
            assert_eq!(draft.signal, Some("handed back"));
        });
    }
}
