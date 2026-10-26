//! Generated HTML lives in an opaque sandbox with no app bridge or network.
use gpui::{Context, Window, canvas, div, prelude::*};
#[cfg(target_os = "macos")]
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSApplication, NSBitmapFormat, NSBitmapImageRep, NSEventType, NSImage, NSView,
    NSWindowOrderingMode,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
#[cfg(target_os = "macos")]
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType,
    WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "macos")]
use std::{cell::RefCell, rc::Rc, sync::Arc};
#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSView))]
    #[name = "MonocodeArtifactClip"]
    #[thread_kind = MainThreadOnly]
    #[ivars = std::cell::Cell<bool>]
    struct ArtifactClip;
    unsafe impl NSObjectProtocol for ArtifactClip {}
    impl ArtifactClip {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // Wheel events belong to the conversation underneath: the page
            // has a fixed frame and must not trap scrolling.
            let scrolling = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .is_some_and(|event| event.r#type() == NSEventType::ScrollWheel);
            if !self.ivars().get() || scrolling {
                None
            } else {
                unsafe { msg_send![super(self), hitTest: point] }
            }
        }
    }
);
#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MonocodeArtifactNavigation"]
    #[thread_kind=MainThreadOnly]
    #[ivars=gpui::AsyncApp]
    struct NavigationGuard;
    unsafe impl NSObjectProtocol for NavigationGuard {}
    unsafe impl WKNavigationDelegate for NavigationGuard {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide(
            &self,
            _: &WKWebView,
            action: &WKNavigationAction,
            reply: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() }.and_then(|url| url.absoluteString());
            if unsafe { action.navigationType() } == WKNavigationType::LinkActivated
                && let Some(url) = url.as_ref()
                && (url.to_string().starts_with("https://")
                    || url.to_string().starts_with("http://"))
            {
                let url = url.to_string();
                self.ivars().clone().update(|cx| cx.open_url(&url));
            }
            let allowed = url.is_none_or(|url| {
                let s = url.to_string();
                s.starts_with("about:blank") || s.starts_with("about:srcdoc")
            });
            reply.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
);
#[cfg(target_os = "macos")]
struct Hosted {
    clip: Retained<ArtifactClip>,
    web: Retained<WKWebView>,
    _guard: Retained<NavigationGuard>,
    markup: Arc<str>,
    window: gpui::WindowId,
    /// The window frame this page last painted in.
    seen: u64,
    /// Clip frame, document frame and zoom as last applied.
    placed: [f64; 9],
    modal: bool,
}
/// The page can't sit under GPUI's popovers, so while one covers it a still
/// of the page is painted in its place and the native view steps aside.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Cover {
    snapshot: Option<Arc<gpui::RenderImage>>,
    pending: bool,
    /// Set once nothing covers the page, so the next render frees the still.
    retired: bool,
}
#[cfg(target_os = "macos")]
struct Markup {
    palette: Palette,
    html: Arc<str>,
}
/// What a conversation page has reported about itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Page {
    pub(crate) loaded: bool,
    /// The height the content needs at the current width.
    pub(crate) height: Option<f32>,
}
pub(crate) struct PageChanged;
impl gpui::EventEmitter<PageChanged> for Html {}
pub(crate) struct Html {
    body: String,
    /// Mono artifacts may load https libraries; session visuals never do.
    remote: bool,
    interactive: bool,
    /// While a modal page is shown in a window, the window's other pages hide:
    /// native views would otherwise sit on top of the modal's backdrop.
    modal: bool,
    page: Page,
    _watch: Option<gpui::Task<()>>,
    #[cfg(target_os = "macos")]
    markup: Option<Markup>,
    #[cfg(target_os = "macos")]
    view: Rc<RefCell<Option<Hosted>>>,
    #[cfg(target_os = "macos")]
    cover: Rc<RefCell<Cover>>,
}
#[cfg(target_os = "macos")]
fn modal_shown(window: gpui::WindowId) -> bool {
    HOSTS.with(|hosts| {
        hosts
            .borrow()
            .iter()
            .filter_map(std::rc::Weak::upgrade)
            .any(|host| {
                host.try_borrow().is_ok_and(|view| {
                    view.as_ref()
                        .is_some_and(|v| v.modal && v.window == window && !v.clip.isHidden())
                })
            })
    })
}
/// Opens an image a page was clicked on in the app's viewer.
#[cfg(target_os = "macos")]
fn open_image(window: gpui::WindowId, src: &str, alt: &str, cx: &mut gpui::AsyncApp) {
    let Some(mime) = src
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(';'))
        .map(|(mime, _)| mime.to_owned())
    else {
        return;
    };
    let open = crate::image_lightbox::Open {
        source: crate::image_lightbox::Source::Embedded {
            data: src.into(),
            mime: mime.into(),
        },
        alt: if alt.is_empty() {
            "Image".into()
        } else {
            alt.to_owned().into()
        },
    };
    cx.update(|cx| {
        if let Some(handle) = cx.windows().into_iter().find(|w| w.window_id() == window) {
            let _ = handle.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(open), cx)
            });
        }
    });
}
/// Runs `script` in a web view's top document and returns its string result.
#[cfg(target_os = "macos")]
pub(crate) async fn evaluate(web: &WKWebView, script: &str, cx: &gpui::AsyncApp) -> Option<String> {
    let (send, receive) = async_channel::bounded(1);
    let block = block2::RcBlock::new(
        move |value: *mut objc2::runtime::AnyObject, _: *mut objc2_foundation::NSError| {
            let text = unsafe { value.as_ref() }
                .and_then(|value| value.downcast_ref::<NSString>())
                .map(|text| text.to_string());
            let _ = send.try_send(text);
        },
    );
    unsafe {
        web.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&block));
    }
    let started = std::time::Instant::now();
    loop {
        if let Ok(value) = receive.try_recv() {
            return value;
        }
        if started.elapsed() > std::time::Duration::from_secs(3) {
            return None;
        }
        cx.background_executor()
            .timer(std::time::Duration::from_millis(30))
            .await;
    }
}
#[cfg(target_os = "macos")]
thread_local! {
    // A native view stays where it was last placed until someone moves it, so a
    // row that scrolls out of a virtualized list must be hidden by its owner.
    static HOSTS: RefCell<Vec<std::rc::Weak<RefCell<Option<Hosted>>>>> =
        const { RefCell::new(Vec::new()) };
}
/// Runs first inside an isolated page and reports to the wrapper: console
/// output, uncaught errors, load, and the document's height.
const PAGE_PROBE: &str = r#"<script>(()=>{const post=m=>{try{parent.postMessage(Object.assign({__mc:1},m),'*')}catch(e){}};
const text=a=>a.map(x=>{try{return typeof x==='string'?x:JSON.stringify(x)}catch(e){return String(x)}}).join(' ').slice(0,500);
for(const l of ['log','info','warn','error']){const o=console[l];console[l]=(...a)=>{post({k:'c',l,t:text(a)});o.apply(console,a)}}
addEventListener('error',e=>post({k:'c',l:'error',t:(e.message||'Script error')+(e.lineno?' (line '+e.lineno+')':'')}));
addEventListener('unhandledrejection',e=>post({k:'c',l:'error',t:'Unhandled rejection: '+text([e.reason])}));
addEventListener('securitypolicyviolation',e=>post({k:'c',l:'error',t:'Blocked by the page policy: '+e.blockedURI+' ('+e.violatedDirective+')'}));
addEventListener('load',()=>post({k:'l'}));
addEventListener('click',e=>{const t=e.composedPath()[0];if(!e.isTrusted||e.button!==0||e.metaKey||e.ctrlKey||e.shiftKey||e.altKey||!t||t.tagName!=='IMG'||t.closest('a[href],button'))return;
setTimeout(()=>{const u=t.currentSrc||t.src;if(!e.defaultPrevented&&u&&u.startsWith('data:image/'))post({k:'i',src:u,alt:t.alt||''})})});
addEventListener('message',e=>{const d=e.data;if(e.source===parent&&d&&typeof d.__mcTheme==='string'){const s=document.getElementById('mc-theme');if(s)s.textContent=d.__mcTheme}});
const h=()=>{const r=document.documentElement;post({k:'h',v:Math.ceil(r.scrollHeight>r.clientHeight?r.scrollHeight:r.getBoundingClientRect().height)})};
const o=new ResizeObserver(h);o.observe(document.documentElement);
addEventListener('DOMContentLoaded',()=>{if(document.body)o.observe(document.body);h()});addEventListener('load',h);})();</script>"#;
/// MonoCode's component kit for conversation pages: `mc-*` classes and the
/// `window.mc` helpers that the visual documentation describes.
const KIT_STYLE: &str = concat!(include_str!("kit.css"), include_str!("kit_viz.css"));
const KIT_SCRIPT: &str = concat!(include_str!("kit.js"), "\n", include_str!("kit_viz.js"));
/// The wrapper keeps the probe's reports where the host can read them.
const WRAPPER_SCRIPT: &str = r#"<script>window.__mc={log:[],loaded:false,h:0};
window.__mcTheme=t=>{const f=document.querySelector('iframe');if(f&&f.contentWindow)f.contentWindow.postMessage({__mcTheme:t},'*')};
addEventListener('message',e=>{const d=e.data,s=window.__mc;if(!d||d.__mc!==1)return;
if(d.k==='c'){if(s.log.length<50)s.log.push({level:String(d.l).slice(0,8),text:String(d.t).slice(0,500)})}
else if(d.k==='l')s.loaded=true;
else if(d.k==='i'&&typeof d.src==='string'&&d.src.startsWith('data:image/')&&d.src.length<28e6)s.open={src:d.src,alt:String(d.alt||'').slice(0,200)};
else if(d.k==='h'&&Number.isFinite(d.v))s.h=Math.min(d.v,20000)});</script>"#;
/// A conversation page on its surface color, for a screenshot.
pub(crate) fn preview_page(body: &str, palette: Palette) -> String {
    sandbox_on(body, false, true, palette)
}
#[cfg(target_os = "macos")]
thread_local! {
    static FRAMES: RefCell<std::collections::HashMap<gpui::WindowId, u64>> =
        RefCell::new(std::collections::HashMap::new());
}
#[cfg(target_os = "macos")]
fn frame(window: gpui::WindowId) -> u64 {
    FRAMES.with(|frames| frames.borrow().get(&window).copied().unwrap_or(0))
}
/// Call first in the window root's render, which runs on every draw.
pub(crate) fn begin_frame(window: &Window) {
    #[cfg(target_os = "macos")]
    FRAMES.with(|frames| {
        *frames
            .borrow_mut()
            .entry(Window::window_handle(window).window_id())
            .or_default() += 1;
    });
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}
/// Paint this after everything else in the root. A native view stays where it
/// was last placed, so any page that did not paint this frame (scrolled out of
/// a virtualized list, in a pane that is no longer shown, or under a cached
/// view that replayed its quads) is hidden here.
pub(crate) fn reaper() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            #[cfg(target_os = "macos")]
            {
                let id = Window::window_handle(window).window_id();
                let now = frame(id);
                HOSTS.with(|hosts| {
                    for host in hosts.borrow().iter().filter_map(std::rc::Weak::upgrade) {
                        if let Some(view) = host.borrow().as_ref()
                            && view.window == id
                            && view.seen != now
                        {
                            view.clip.setHidden(true);
                        }
                    }
                });
            }
            #[cfg(not(target_os = "macos"))]
            let _ = window;
        },
    )
    .absolute()
    .size_0()
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
/// The colors a page is themed from.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Palette {
    ink: gpui::Hsla,
    accent: gpui::Hsla,
}
impl Palette {
    pub(crate) fn current() -> Self {
        Self {
            ink: crate::theme::content(1.),
            accent: crate::theme::accent_color(),
        }
    }
    /// The app's accent on a light or a dark surface, whatever the app uses.
    pub(crate) fn forced(light: bool) -> Self {
        let ink = if light {
            gpui::rgb(0x171717)
        } else {
            gpui::rgb(0xededed)
        };
        Self {
            ink: ink.into(),
            accent: crate::theme::accent_color(),
        }
    }
    fn light(self) -> bool {
        let ink: gpui::Rgba = self.ink.into();
        ink.r * 0.2126 + ink.g * 0.7152 + ink.b * 0.0722 < 0.5
    }
}
fn hex(value: gpui::Hsla) -> String {
    let value: gpui::Rgba = value.into();
    format!(
        "#{:02x}{:02x}{:02x}",
        (value.r * 255.) as u8,
        (value.g * 255.) as u8,
        (value.b * 255.) as u8
    )
}
fn background(palette: Palette) -> &'static str {
    if palette.light() {
        "#fafafa"
    } else {
        "#141414"
    }
}
/// The theme of a conversation page as CSS variables. A page that is already
/// open receives a new set through `__mcTheme` instead of reloading.
fn tokens(palette: Palette) -> String {
    let foreground = hex(palette.ink);
    let accent = hex(palette.accent);
    let light = palette.light();
    let background = background(palette);
    let muted = if light { "#737373" } else { "#969696" };
    let scheme = if light { "light" } else { "dark" };
    // Backgrounds are transparent mixes of the ink, so they follow the theme
    // and the window's glass.
    let (popover, on_primary) = if light {
        ("#ffffff", "#fafafa")
    } else {
        ("#1f1f1f", "#141414")
    };
    format!(
        ":root{{--background:{background};--foreground:{foreground};--muted-foreground:{muted};--muted:color-mix(in srgb,{foreground} 7%,transparent);--card:color-mix(in srgb,{foreground} 5%,transparent);--card-foreground:{foreground};--popover:{popover};--popover-foreground:{foreground};--secondary:color-mix(in srgb,{foreground} 9%,transparent);--secondary-foreground:{foreground};--border:color-mix(in srgb,{foreground} 13%,transparent);--input:color-mix(in srgb,{foreground} 16%,transparent);--ring:{accent};--primary:{foreground};--primary-foreground:{on_primary};--accent:{accent};--accent-foreground:#ffffff;--accent-surface:color-mix(in srgb,{accent} 16%,transparent);--accent-surface-foreground:{accent};--destructive:#f43f5e;--destructive-foreground:#ffffff;--destructive-surface:color-mix(in srgb,#f43f5e 16%,transparent);--warning:#f59e0b;--warning-foreground:#1a1a1a;--warning-surface:color-mix(in srgb,#f59e0b 16%,transparent);--success:#10b981;--success-foreground:#ffffff;--info:#38bdf8;--info-foreground:#0b1a24;--code-background:color-mix(in srgb,{foreground} 6%,transparent);--code-foreground:{foreground};--surface:var(--card);--link:{accent};--positive:var(--success);--negative:var(--destructive);--chart-1:{accent};--chart-2:#10b981;--chart-3:#f59e0b;--chart-4:#f43f5e;--chart-5:#a78bfa;--chart-6:#22d3ee;--font-sans:-apple-system,BlinkMacSystemFont,system-ui,sans-serif;--font-mono:ui-monospace,'SF Mono',Menlo,monospace;--radius:10px;color-scheme:{scheme}}}"
    )
}
/// The script that restyles an open conversation page.
fn retheme_script(palette: Palette) -> String {
    format!(
        "window.__mcTheme&&__mcTheme({})",
        serde_json::Value::String(tokens(palette))
    )
}
fn sandbox(body: &str, remote: bool) -> String {
    sandbox_on(body, remote, false, Palette::current())
}
/// `backdrop` paints the app's surface color behind the page, for screenshots
/// that have no conversation under them.
fn sandbox_on(body: &str, remote: bool, backdrop: bool, palette: Palette) -> String {
    let foreground = hex(palette.ink);
    let accent = hex(palette.accent);
    let light = palette.light();
    let background = background(palette);
    let muted = if light { "#737373" } else { "#969696" };
    let surface = if light { "#eeeeee" } else { "#222222" };
    let border = if light { "#d4d4d4" } else { "#383838" };
    let policy = if remote {
        "script-src 'unsafe-inline' https:; style-src 'unsafe-inline' https:; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:".to_owned()
    } else {
        use monocode_core::visual::{FONT_HOSTS, SCRIPT_HOSTS, STYLE_HOSTS, csp_sources};
        // Libraries and fonts may come from a short list of CDNs; the page
        // itself may not make requests (connect-src) or pull images, which
        // would leave data in a URL.
        format!(
            "script-src 'unsafe-inline' {}; style-src 'unsafe-inline' {}; img-src data: blob:; font-src data: {}; media-src data: blob:",
            csp_sources(&SCRIPT_HOSTS),
            csp_sources(&STYLE_HOSTS),
            csp_sources(&FONT_HOSTS),
        )
    };
    let csp = format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; {policy}; worker-src blob:; connect-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'\"><meta name=viewport content=\"width=device-width,initial-scale=1\">"
    );
    let scheme = if light { "light" } else { "dark" };
    if !remote {
        let theme = tokens(palette);
        let probe = PAGE_PROBE;
        let source = format!(
            "<!doctype html>{csp}<style id=mc-theme>{theme}</style><style>*{{box-sizing:border-box}}html{{background:transparent}}body{{margin:0;font:14px/1.5 var(--font-sans);color:var(--foreground);background:transparent;-webkit-font-smoothing:antialiased}}a{{color:var(--link)}}::-webkit-scrollbar{{display:none}}{KIT_STYLE}</style>{probe}<script>{KIT_SCRIPT}</script><body>{body}"
        );
        let behind = if backdrop { background } else { "transparent" };
        return format!(
            "<!doctype html><style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;overflow:hidden;background:transparent}}html,body{{background:{behind}}}</style>{WRAPPER_SCRIPT}<iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" allowtransparency=\"true\" srcdoc=\"{}\"></iframe>",
            escape(&source)
        );
    }
    let source = format!(
        "<!doctype html>{csp}<style>:root{{--background:{background};--foreground:{foreground};--muted:{muted};--surface:{surface};--border:{border};--accent:{accent};--link:{accent};--positive:#10b981;--negative:#f43f5e;--chart-1:{accent};--chart-2:#10b981;--chart-3:#f59e0b;--chart-4:#f43f5e;--chart-5:#a78bfa;--chart-6:#22d3ee;--font-sans:system-ui;--font-mono:ui-monospace,monospace;--radius:10px;color-scheme:{scheme}}}body{{font:14px var(--font-sans);margin:20px;color:var(--foreground);background:var(--background)}}a{{color:var(--link)}}</style>{body}"
    );
    format!(
        "<!doctype html><style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;overflow:hidden;background:{background}}}</style><iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" srcdoc=\"{}\"></iframe>",
        escape(&source)
    )
}
impl Html {
    pub(crate) fn new(body: String) -> Self {
        Self::with_remote(body, true)
    }
    /// For agent-written pages in a conversation: nothing loads from the
    /// network, and the page reports its load and height.
    pub(crate) fn isolated(body: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self::with_remote(body, false);
        #[cfg(target_os = "macos")]
        {
            this._watch = Some(Self::watch(cx));
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
        this
    }
    /// A conversation page shown over everything else in its window.
    pub(crate) fn modal(body: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self::isolated(body, cx);
        this.modal = true;
        this
    }
    pub(crate) fn page(&self) -> Page {
        self.page
    }
    /// Follows the wrapper's state: quickly until the page loads, then slowly
    /// enough to catch content that grows or shrinks.
    #[cfg(target_os = "macos")]
    fn watch(cx: &mut Context<Self>) -> gpui::Task<()> {
        // Takes a pending image click so it opens once.
        const STATE: &str = "(()=>{const m=window.__mc;if(!m)return '';const o=m.open;m.open=null;return JSON.stringify({l:m.loaded,h:m.h,o})})()";
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((web, loaded)) = this.read_with(cx, |this, _| {
                    let web = this
                        .view
                        .borrow()
                        .as_ref()
                        .filter(|view| !view.clip.isHidden())
                        .map(|view| (view.web.clone(), view.window));
                    (web, this.page.loaded)
                }) else {
                    return;
                };
                if let Some((web, window)) = web
                    && let Some(text) = evaluate(&web, STATE, cx).await
                    && let Ok(state) = serde_json::from_str::<serde_json::Value>(&text)
                {
                    if let Some(src) = state["o"]["src"].as_str() {
                        open_image(window, src, state["o"]["alt"].as_str().unwrap_or(""), cx);
                    }
                    let page = Page {
                        loaded: state["l"] == true,
                        height: state["h"]
                            .as_f64()
                            .filter(|height| *height > 0.)
                            .map(|height| height as f32),
                    };
                    let updated = this.update(cx, |this, cx| {
                        if this.page != page {
                            this.page = page;
                            cx.emit(PageChanged);
                            cx.notify();
                        }
                    });
                    if updated.is_err() {
                        return;
                    }
                }
                let wait = if loaded { 250 } else { 80 };
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(wait))
                    .await;
            }
        })
    }
    fn with_remote(body: String, remote: bool) -> Self {
        let this = Self {
            body,
            remote,
            interactive: true,
            modal: false,
            page: Page::default(),
            _watch: None,
            #[cfg(target_os = "macos")]
            markup: None,
            #[cfg(target_os = "macos")]
            view: Rc::new(RefCell::new(None)),
            #[cfg(target_os = "macos")]
            cover: Rc::default(),
        };
        #[cfg(target_os = "macos")]
        HOSTS.with(|hosts| {
            let mut hosts = hosts.borrow_mut();
            hosts.retain(|host| host.strong_count() > 0);
            hosts.push(Rc::downgrade(&this.view));
        });
        this
    }
    #[cfg(target_os = "macos")]
    fn markup(&mut self) -> (Arc<str>, Palette) {
        let palette = Palette::current();
        if self.markup.as_ref().is_none_or(|m| m.palette != palette) {
            self.markup = Some(Markup {
                palette,
                html: sandbox(&self.body, self.remote).into(),
            });
        }
        (self.markup.as_ref().unwrap().html.clone(), palette)
    }
    pub(crate) fn set_interactive(&mut self, interactive: bool) {
        if self.interactive != interactive {
            self.interactive = interactive;
        }
    }
    pub(crate) fn hide(&self) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self.view.borrow().as_ref() {
            view.clip.setHidden(true);
        }
    }
}
impl gpui::Render for Html {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            let view = self.view.clone();
            let (html, palette) = self.markup();
            let app = cx.to_async();
            let interactive = self.interactive;
            let this = cx.entity().downgrade();
            let cover = self.cover.clone();
            let snapshot = {
                let mut state = cover.borrow_mut();
                if std::mem::take(&mut state.retired)
                    && let Some(image) = state.snapshot.take()
                {
                    cx.drop_image(image, Some(window));
                }
                state.snapshot.clone()
            };
            let still = snapshot.is_some();
            let transparent = !self.remote;
            let modal = self.modal;
            // A loaded conversation page takes a new theme in place; reloading
            // would blank it and restart its animations.
            let retheme = (!self.remote && self.page.loaded).then(|| retheme_script(palette));
            let host = canvas(
                |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                move |bounds, hitbox, window, _| {
                    let covered = window.is_hitbox_covered(&hitbox);
                    let Ok(handle) = HasWindowHandle::window_handle(window) else {
                        return;
                    };
                    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                        return;
                    };
                    // GPUI keeps its AppKit content view alive throughout paint.
                    let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
                    // AppKit frames are in window points; GPUI's are divided
                    // by the interface zoom.
                    let zoom = window.content_zoom();
                    let clipped = bounds
                        .intersect(&window.content_mask().bounds)
                        .map(|v| v * zoom);
                    let bounds = bounds.map(|v| v * zoom);
                    let window_id = Window::window_handle(window).window_id();
                    if !modal && modal_shown(window_id) {
                        if let Some(view) = view.borrow().as_ref() {
                            view.clip.setHidden(true);
                        }
                        return;
                    }
                    let mut view = view.borrow_mut();
                    if clipped.size.width <= gpui::px(0.) || clipped.size.height <= gpui::px(0.) {
                        if let Some(view) = view.as_ref() {
                            view.clip.setHidden(true);
                        }
                        return;
                    }
                    let x = f64::from(clipped.origin.x);
                    let y = f64::from(clipped.origin.y);
                    let clip_width = f64::from(clipped.size.width);
                    let clip_height = f64::from(clipped.size.height);
                    let width = f64::from(bounds.size.width);
                    let height = f64::from(bounds.size.height);
                    let y = if parent.isFlipped() {
                        y
                    } else {
                        parent.bounds().size.height - y - clip_height
                    };
                    let rect =
                        NSRect::new(NSPoint::new(x, y), NSSize::new(clip_width, clip_height));
                    // Keep the document at its full layout size while its
                    // native host follows GPUI's moving content mask.
                    let web_rect = NSRect::new(
                        NSPoint::new(
                            f64::from(bounds.left() - clipped.left()),
                            f64::from(bounds.top() - clipped.top()),
                        ),
                        NSSize::new(width, height),
                    );
                    if view.is_none() {
                        let Some(mtm) = MainThreadMarker::new() else {
                            return;
                        };
                        // No message handlers, file base URL,
                        // app credentials or persistent website data are installed.
                        let hosted = unsafe {
                            let clip = ArtifactClip::alloc(mtm)
                                .set_ivars(std::cell::Cell::new(interactive));
                            let clip: Retained<ArtifactClip> =
                                msg_send![super(clip), initWithFrame: rect];
                            clip.setWantsLayer(true);
                            clip.setClipsToBounds(true);
                            let config = WKWebViewConfiguration::new(mtm);
                            config
                                .preferences()
                                .setJavaScriptCanOpenWindowsAutomatically(false);
                            config.setWebsiteDataStore(
                                &WKWebsiteDataStore::nonPersistentDataStore(mtm),
                            );
                            let web = WKWebView::initWithFrame_configuration(
                                WKWebView::alloc(mtm),
                                web_rect,
                                &config,
                            );
                            if transparent {
                                // Let the thread's own surface show through.
                                let _: () = msg_send![&*web, _setDrawsBackground: false];
                            }
                            let guard = NavigationGuard::alloc(mtm).set_ivars(app.clone());
                            let guard: Retained<NavigationGuard> = msg_send![super(guard), init];
                            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*guard)));
                            web.loadHTMLString_baseURL(&NSString::from_str(&html), None);
                            clip.addSubview(&web);
                            Hosted {
                                clip,
                                web,
                                _guard: guard,
                                markup: html.clone(),
                                window: Window::window_handle(window).window_id(),
                                seen: 0,
                                placed: [f64::NAN; 9],
                                modal,
                            }
                        };
                        *view = Some(hosted);
                    }
                    if let Some(view) = view.as_mut() {
                        if !Arc::ptr_eq(&view.markup, &html) {
                            unsafe {
                                if let Some(script) = &retheme {
                                    view.web.evaluateJavaScript_completionHandler(
                                        &NSString::from_str(script),
                                        None,
                                    );
                                } else {
                                    view.web
                                        .loadHTMLString_baseURL(&NSString::from_str(&html), None);
                                }
                            }
                            view.markup = html.clone();
                        }
                        // Every subview already sits above GPUI's layer. Re-adding
                        // pulls a page out of the hierarchy, which blanks WebKit,
                        // so two pages competing for the top slot would flash on
                        // every frame.
                        let attached = unsafe { view.clip.superview() }
                            .is_some_and(|sup| std::ptr::eq(&*sup, parent));
                        if !attached {
                            parent.addSubview_positioned_relativeTo(
                                &view.clip,
                                NSWindowOrderingMode::Above,
                                None,
                            );
                        }
                        view.clip.ivars().set(interactive);
                        let placed = [
                            rect.origin.x,
                            rect.origin.y,
                            rect.size.width,
                            rect.size.height,
                            web_rect.origin.x,
                            web_rect.origin.y,
                            web_rect.size.width,
                            web_rect.size.height,
                            f64::from(zoom),
                        ];
                        if view.placed != placed {
                            view.clip.setFrame(rect);
                            view.web.setFrame(web_rect);
                            unsafe { view.web.setPageZoom(f64::from(zoom)) };
                            view.placed = placed;
                        }
                        let mut state = cover.borrow_mut();
                        if !covered {
                            state.pending = false;
                            if state.snapshot.is_some() {
                                state.retired = true;
                                window.request_animation_frame();
                            }
                        } else if !still && !state.pending {
                            // The page stays up until its still arrives, a
                            // frame or two, rather than leaving a hole.
                            state.pending = true;
                            snapshot_page(&view.web, cover.clone(), app.clone(), this.clone());
                        }
                        drop(state);
                        let hidden = covered && still;
                        if view.clip.isHidden() != hidden {
                            view.clip.setHidden(hidden);
                        }
                        view.seen = frame(view.window);
                    }
                },
            )
            .absolute()
            .size_full();
            div()
                .relative()
                .size_full()
                .children(snapshot.map(|image| gpui::img(image).absolute().size_full()))
                .child(host)
                .into_any_element()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (window, cx);
            div().p_4().child(self.body.clone()).into_any_element()
        }
    }
}
#[cfg(target_os = "macos")]
fn snapshot_page(
    web: &WKWebView,
    cover: Rc<RefCell<Cover>>,
    app: gpui::AsyncApp,
    this: gpui::WeakEntity<Html>,
) {
    let done = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        let mut state = cover.borrow_mut();
        if !std::mem::take(&mut state.pending) {
            return;
        }
        // SAFETY: WebKit passes a live image or null for the duration of the call.
        state.snapshot = unsafe { image.as_ref() }.and_then(pixels).map(Arc::new);
        drop(state);
        let _ = app.update(|cx| this.update(cx, |_, cx| cx.notify()));
    });
    unsafe { web.takeSnapshotWithConfiguration_completionHandler(None, &done) };
}
/// Copies an 8-bit RGB(A) snapshot into the BGRA layout GPUI's sprites use.
#[cfg(target_os = "macos")]
fn pixels(image: &NSImage) -> Option<gpui::RenderImage> {
    let tiff = image.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let format = rep.bitmapFormat();
    let samples = rep.samplesPerPixel() as usize;
    let step = rep.bitsPerPixel() as usize / 8;
    if rep.isPlanar()
        || rep.bitsPerSample() != 8
        || !(samples == 3 || samples == 4)
        || step < samples
        || format.contains(NSBitmapFormat::FloatingPointSamples)
    {
        return None;
    }
    let (width, height) = (rep.pixelsWide() as usize, rep.pixelsHigh() as usize);
    let stride = rep.bytesPerRow() as usize;
    let data = rep.bitmapData();
    if data.is_null() || width == 0 || height == 0 || stride < width * step {
        return None;
    }
    // SAFETY: the rep owns `bytesPerRow * pixelsHigh` bytes while it lives.
    let bytes = unsafe { std::slice::from_raw_parts(data, stride * height) };
    let alpha_first = format.contains(NSBitmapFormat::AlphaFirst);
    let mut out = image::RgbaImage::new(width as u32, height as u32);
    for (y, row) in out.rows_mut().enumerate() {
        let source = &bytes[y * stride..];
        for (x, pixel) in row.enumerate() {
            let p = &source[x * step..];
            let [r, g, b, a] = match (samples, alpha_first) {
                (4, true) => [p[1], p[2], p[3], p[0]],
                (4, false) => [p[0], p[1], p[2], p[3]],
                _ => [p[0], p[1], p[2], 255],
            };
            pixel.0 = [b, g, r, a];
        }
    }
    Some(gpui::RenderImage::new(vec![image::Frame::new(out)]))
}
#[cfg(target_os = "macos")]
impl Drop for Html {
    fn drop(&mut self) {
        if let Some(view) = self.view.borrow_mut().take() {
            view.clip.removeFromSuperview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_markup_cannot_escape_the_opaque_iframe() {
        let html = sandbox("\"><script>window.top.postMessage('x','*')</script>", true);
        assert!(html.contains("sandbox=\"allow-scripts\""));
        assert!(!html.contains("allow-same-origin"));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("&lt;script&gt;"));
    }

    struct Page {
        popover: Option<gpui::Bounds<gpui::Pixels>>,
        covered: Rc<std::cell::Cell<bool>>,
    }
    impl gpui::Render for Page {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let covered = self.covered.clone();
            div()
                .size_full()
                .child(
                    canvas(
                        |bounds, window, _| {
                            window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal)
                        },
                        move |_, hitbox, window, _| covered.set(window.is_hitbox_covered(&hitbox)),
                    )
                    .size(gpui::px(200.)),
                )
                // A later sibling that doesn't overlap must not count.
                .child(div().id("after").size(gpui::px(20.)).on_click(|_, _, _| {}))
                // The app's dropdown card, as used by the model picker and menus.
                .children(self.popover.map(|bounds| {
                    crate::popover::popover(
                        "dropdown",
                        crate::popover::Anchor::Point(bounds.origin),
                    )
                    .width(f32::from(bounds.size.width))
                    .child(div().h(bounds.size.height).child("Opus"))
                }))
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, _| {}))
        }
    }

    #[gpui::test]
    fn only_overlays_that_overlap_the_page_cover_it(cx: &mut gpui::TestAppContext) {
        let covered = Rc::new(std::cell::Cell::new(false));
        let window = cx.update(|cx| {
            let covered = covered.clone();
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Page {
                    popover: None,
                    covered,
                })
            })
            .unwrap()
        });
        let page = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        page.run_until_parked();
        assert!(!covered.get());
        for (left, expected) in [(100., true), (400., false)] {
            window
                .update(page, |view, _, cx| {
                    view.popover = Some(gpui::Bounds::new(
                        gpui::point(gpui::px(left), gpui::px(50.)),
                        gpui::size(gpui::px(80.), gpui::px(80.)),
                    ));
                    cx.notify();
                })
                .unwrap();
            page.run_until_parked();
            assert_eq!(covered.get(), expected);
        }
    }
    #[test]
    fn isolated_pages_load_code_only_from_listed_cdns_and_never_connect_or_fetch_images() {
        let isolated = sandbox("<p>x</p>", false);
        assert!(isolated.contains("script-src 'unsafe-inline' https://cdn.jsdelivr.net"));
        assert!(!isolated.contains("script-src 'unsafe-inline' https:;"));
        assert!(!isolated.contains("https://evil"));
        assert!(isolated.contains("img-src data: blob:;"));
        assert!(isolated.contains("connect-src 'none'"));
        assert!(isolated.contains("frame-src 'none'"));
        assert!(isolated.contains("sandbox=\"allow-scripts\""));
        assert!(!isolated.contains("allow-same-origin"));
        assert!(sandbox("<p>x</p>", true).contains("img-src data: blob: https:"));
    }
    #[test]
    fn open_pages_are_rethemed_in_place_with_the_tokens_a_new_page_starts_with() {
        let dark = super::Palette::forced(false);
        let light = super::Palette::forced(true);
        let page = super::sandbox_on("<p>x</p>", false, false, dark);
        assert!(page.contains(&super::escape(&format!(
            "<style id=mc-theme>{}</style>",
            super::tokens(dark)
        ))));
        assert!(super::tokens(light).contains("color-scheme:light"));
        assert!(super::tokens(dark).contains("color-scheme:dark"));
        let script = super::retheme_script(light);
        assert!(script.starts_with("window.__mcTheme&&__mcTheme(\""));
        assert!(script.contains("--background:#fafafa"));
    }
}
