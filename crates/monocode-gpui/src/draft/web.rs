//! The AppKit side of the Draft panel: a `WKWebView` placed over the window's
//! content view where the panel's canvas is, and the calls that read it.
use gpui::{Context, Window, canvas, prelude::*};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBitmapFormat, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{
    NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL, NSURLRequest,
};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType,
    WKSnapshotConfiguration, WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
    WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

define_class!(
    #[unsafe(super(NSView))]
    #[name = "MonocodeDraftClip"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct DraftClip;
    unsafe impl NSObjectProtocol for DraftClip {}
    impl DraftClip {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool {
            true
        }
    }
);

/// What a draft's web view may navigate to.
pub(super) enum Policy {
    /// The agent's own page: nothing loads but the page itself.
    Page(gpui::AsyncApp),
    /// A dev server on this machine.
    Local(gpui::AsyncApp),
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MonocodeDraftNavigation"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Policy]
    struct Navigation;
    unsafe impl NSObjectProtocol for Navigation {}
    unsafe impl WKNavigationDelegate for Navigation {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide(
            &self,
            _: &WKWebView,
            action: &WKNavigationAction,
            reply: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() }
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string());
            let (Policy::Page(app) | Policy::Local(app)) = self.ivars();
            let web = url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
            let local = url.as_deref().is_some_and(is_local);
            let link = unsafe { action.navigationType() } == WKNavigationType::LinkActivated;
            // Links to anywhere but the draft itself open in the user's browser.
            if link && web && !(matches!(self.ivars(), Policy::Local(_)) && local) {
                let url = url.clone().unwrap_or_default();
                app.clone().update(|cx| cx.open_url(&url));
            }
            let allowed = match url.as_deref() {
                None => true,
                Some(url) if url.starts_with("about:") => true,
                Some(_) => matches!(self.ivars(), Policy::Local(_)) && local,
            };
            reply.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
);

fn is_local(url: &str) -> bool {
    monocode_core::visual_bridge::local_url(url).is_ok()
}

/// What the web view shows.
#[derive(Clone)]
pub(super) enum Load {
    Html(Arc<str>),
    Url(Arc<str>),
}

struct Hosted {
    clip: Retained<DraftClip>,
    web: Retained<WKWebView>,
    _guard: Retained<Navigation>,
    /// The content last handed to the web view.
    loaded: Option<(u64, Load)>,
    /// Clip frame, web frame and zoom as last applied.
    placed: [f64; 9],
}

/// While a popover covers the panel, a still of the page is painted in its
/// place and the native view steps aside.
#[derive(Default)]
pub(super) struct Cover {
    snapshot: Option<Arc<gpui::RenderImage>>,
    pending: bool,
    /// Set once nothing covers the page, so the next render frees the still.
    retired: bool,
}

/// How a draft is laid out in the space the thread gives it.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct Fit {
    /// Display scale: the whole width fits, never larger than life.
    pub scale: f32,
}

/// A draft is as wide as its column allows and as tall as that makes it: the
/// thread scrolls, so height never limits the scale.
pub(crate) fn fit(width: u32, area: (f32, f32)) -> Fit {
    Fit {
        scale: (area.0 / width.max(1) as f32).clamp(0.05, 1.),
    }
}

/// A draft's native view and the epoch it was last painted in.
type Tracked = (
    std::rc::Weak<RefCell<Option<Hosted>>>,
    Rc<std::cell::Cell<u64>>,
);

thread_local! {
    /// Bumped before each paint of a thread, so a draft can tell whether it
    /// was drawn in the paint that just happened.
    static EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static HOSTS: RefCell<Vec<Tracked>> = const { RefCell::new(Vec::new()) };
}

/// Put last inside a thread: a native view stays where it was last placed, so
/// any draft whose row was not painted this time (scrolled far away, or
/// dropped from the list) steps aside.
pub(crate) fn sentinel() -> gpui::AnyElement {
    canvas(
        |_, _, _| EPOCH.with(|epoch| epoch.set(epoch.get() + 1)),
        |_, _, _, _| {
            let now = EPOCH.with(std::cell::Cell::get);
            HOSTS.with(|hosts| {
                hosts.borrow_mut().retain(|(view, painted)| {
                    let Some(view) = view.upgrade() else {
                        return false;
                    };
                    if painted.get() != now
                        && let Some(hosted) = view.borrow().as_ref()
                        && !hosted.clip.isHidden()
                    {
                        hosted.clip.setHidden(true);
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

/// The web view of one draft, shared with the tasks that read it.
#[derive(Clone, Default)]
pub(super) struct Host {
    view: Rc<RefCell<Option<Hosted>>>,
    /// The epoch of the last paint that drew the draft.
    painted: Rc<std::cell::Cell<u64>>,
    cover: Rc<RefCell<Cover>>,
    /// The panel area the draft was last painted in, and the window zoom.
    area: Rc<std::cell::Cell<Option<(f32, f32, f32)>>>,
}

impl Host {
    pub(super) fn web(&self) -> Option<Retained<WKWebView>> {
        self.view.borrow().as_ref().map(|view| view.web.clone())
    }
    /// Where the native view sits in the window, in points: x, y, width,
    /// height, and whether it is hidden.
    #[cfg(debug_assertions)]
    pub(super) fn placement(&self) -> Option<[f64; 5]> {
        self.view.borrow().as_ref().map(|view| {
            let frame = view.clip.frame();
            [
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
                f64::from(u8::from(view.clip.isHidden())),
            ]
        })
    }
    /// The page is on screen and not stepped aside for a popover.
    pub(super) fn visible(&self) -> bool {
        self.view
            .borrow()
            .as_ref()
            .is_some_and(|view| !view.clip.isHidden())
    }
    pub(super) fn hide(&self) {
        if let Some(view) = self.view.borrow().as_ref() {
            view.clip.setHidden(true);
        }
    }
    /// Lays the page out for a new viewport now rather than at the next
    /// frame, so a picture taken straight after a resize is of the new size.
    pub(super) fn resize(&self, width: u32, height: u32) {
        let Some((w, h, zoom)) = self.area.get() else {
            return;
        };
        let mut view = self.view.borrow_mut();
        let Some(view) = view.as_mut() else {
            return;
        };
        let scale = fit(width, (w, h)).scale;
        let size = NSSize::new(
            f64::from(width as f32 * scale * zoom),
            f64::from(height as f32 * scale * zoom),
        );
        let (clip, web) = (view.clip.frame().origin, view.web.frame().origin);
        view.clip.setFrame(NSRect::new(clip, size));
        view.web.setFrame(NSRect::new(web, size));
        unsafe { view.web.setPageZoom(f64::from(scale * zoom)) };
        // The next paint places it again.
        view.placed = [f64::NAN; 9];
    }
    pub(super) fn set_appearance(&self, light: Option<bool>) {
        if let Some(view) = self.view.borrow().as_ref() {
            let name = light.map(|light| unsafe {
                if light {
                    NSAppearanceNameAqua
                } else {
                    NSAppearanceNameDarkAqua
                }
            });
            view.web
                .setAppearance(name.and_then(NSAppearance::appearanceNamed).as_deref());
        }
    }
    /// Whether the web view has been given `generation` of the draft's content.
    pub(super) fn has_loaded(&self, generation: u64) -> bool {
        self.view
            .borrow()
            .as_ref()
            .and_then(|view| view.loaded.as_ref())
            .is_some_and(|(loaded, _)| *loaded == generation)
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.clip.removeFromSuperview();
    }
}

/// Everything the paint callback needs about the draft.
#[derive(Clone)]
pub(super) struct Placement {
    pub load: Load,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    /// A script that restyles the open page in place, when the theme changed
    /// but the content did not.
    pub retheme: Option<String>,
    pub app: gpui::AsyncApp,
}

/// Paints the draft: places the web view where this canvas is and keeps a still
/// of it for when a popover covers it. Returns the element and, via `Host`,
/// the handle other tasks use.
pub(super) fn surface<T: 'static>(
    host: &Host,
    placement: Placement,
    window: &mut Window,
    cx: &mut Context<T>,
) -> gpui::AnyElement {
    let this = cx.entity().downgrade();
    let view = host.view.clone();
    let cover = host.cover.clone();
    let area = host.area.clone();
    let painted = host.painted.clone();
    HOSTS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        if !hosts
            .iter()
            .any(|(view, _)| std::ptr::eq(view.as_ptr(), Rc::as_ptr(&host.view)))
        {
            hosts.push((Rc::downgrade(&host.view), host.painted.clone()));
        }
    });
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
    let paint = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            painted.set(EPOCH.with(std::cell::Cell::get));
            let covered = window.is_hitbox_covered(&hitbox);
            let Ok(handle) = HasWindowHandle::window_handle(window) else {
                return;
            };
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return;
            };
            // GPUI keeps its AppKit content view alive throughout paint.
            let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
            let zoom = window.content_zoom();
            let available = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            area.set(Some((available.0, available.1, zoom)));
            let scale = fit(placement.width, available).scale;
            let shown = gpui::size(
                gpui::px(placement.width as f32 * scale),
                gpui::px(placement.height as f32 * scale),
            );
            let draft = gpui::Bounds::new(
                gpui::point(
                    bounds.left() + (bounds.size.width - shown.width) / 2.,
                    bounds.top(),
                ),
                shown,
            );
            let clipped = draft
                .intersect(&window.content_mask().bounds)
                .map(|v| v * zoom);
            let draft = draft.map(|v| v * zoom);
            let mut view = view.borrow_mut();
            if clipped.size.width <= gpui::px(0.) || clipped.size.height <= gpui::px(0.) {
                if let Some(view) = view.as_ref() {
                    view.clip.setHidden(true);
                }
                return;
            }
            let (x, y) = (f64::from(clipped.origin.x), f64::from(clipped.origin.y));
            let (clip_width, clip_height) = (
                f64::from(clipped.size.width),
                f64::from(clipped.size.height),
            );
            let y = if parent.isFlipped() {
                y
            } else {
                parent.bounds().size.height - y - clip_height
            };
            let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(clip_width, clip_height));
            let web_rect = NSRect::new(
                NSPoint::new(
                    f64::from(draft.left() - clipped.left()),
                    f64::from(draft.top() - clipped.top()),
                ),
                NSSize::new(f64::from(draft.size.width), f64::from(draft.size.height)),
            );
            if view.is_none() {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                *view = Some(unsafe { create(mtm, rect, web_rect, &placement) });
            }
            let Some(hosted) = view.as_mut() else {
                return;
            };
            if hosted
                .loaded
                .as_ref()
                .is_none_or(|(generation, _)| *generation != placement.generation)
            {
                unsafe { load(&hosted.web, &placement.load) };
                hosted.loaded = Some((placement.generation, placement.load.clone()));
            } else if let Some(script) = &placement.retheme {
                unsafe {
                    hosted
                        .web
                        .evaluateJavaScript_completionHandler(&NSString::from_str(script), None);
                }
            }
            // Re-adding a subview pulls it out of the hierarchy, which blanks
            // WebKit; only attach when it is not already there.
            let attached =
                unsafe { hosted.clip.superview() }.is_some_and(|sup| std::ptr::eq(&*sup, parent));
            if !attached {
                parent.addSubview_positioned_relativeTo(
                    &hosted.clip,
                    NSWindowOrderingMode::Above,
                    None,
                );
            }
            let placed = [
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height,
                web_rect.origin.x,
                web_rect.origin.y,
                web_rect.size.width,
                web_rect.size.height,
                f64::from(scale * zoom),
            ];
            if hosted.placed != placed {
                hosted.clip.setFrame(rect);
                hosted.web.setFrame(web_rect);
                // The page lays out at its logical width and is scaled to fit.
                unsafe { hosted.web.setPageZoom(f64::from(scale * zoom)) };
                hosted.placed = placed;
            }
            let mut state = cover.borrow_mut();
            if !covered {
                state.pending = false;
                if state.snapshot.is_some() {
                    state.retired = true;
                    window.request_animation_frame();
                }
            } else if !still && !state.pending {
                // The page stays up until its still arrives, a frame or two,
                // rather than leaving a hole.
                state.pending = true;
                let app = placement.app.clone();
                let this = this.clone();
                still_of(&hosted.web, cover.clone(), move || {
                    let _ = app.update(|cx| this.update(cx, |_, cx| cx.notify()));
                });
            }
            drop(state);
            let hidden = covered && still;
            if hosted.clip.isHidden() != hidden {
                hosted.clip.setHidden(hidden);
            }
        },
    )
    .absolute()
    .size_full();
    gpui::div()
        .relative()
        .size_full()
        .children(snapshot.map(|image| gpui::img(image).absolute().size_full()))
        .child(paint)
        .into_any_element()
}

/// # Safety
/// Main thread only.
unsafe fn create(
    mtm: MainThreadMarker,
    rect: NSRect,
    web_rect: NSRect,
    placement: &Placement,
) -> Hosted {
    let clip = DraftClip::alloc(mtm).set_ivars(());
    let clip: Retained<DraftClip> = unsafe { msg_send![super(clip), initWithFrame: rect] };
    clip.setWantsLayer(true);
    clip.setClipsToBounds(true);
    // No message handlers, file base URL, app credentials or persistent
    // website data: a draft is a page, not part of the app.
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe {
        config
            .preferences()
            .setJavaScriptCanOpenWindowsAutomatically(false);
        config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
    }
    let policy = match placement.load {
        Load::Html(_) => Policy::Page(placement.app.clone()),
        Load::Url(_) => {
            // A dev server's page is not ours to wrap, so the probe goes in
            // beside it as a user script.
            let script = unsafe {
                WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(&super::page::browser_probe()),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                )
            };
            let controller: Retained<WKUserContentController> =
                unsafe { config.userContentController() };
            unsafe { controller.addUserScript(&script) };
            Policy::Local(placement.app.clone())
        }
    };
    let web =
        unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), web_rect, &config) };
    let guard = Navigation::alloc(mtm).set_ivars(policy);
    let guard: Retained<Navigation> = unsafe { msg_send![super(guard), init] };
    unsafe {
        web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*guard)));
    }
    clip.addSubview(&web);
    Hosted {
        clip,
        web,
        _guard: guard,
        loaded: None,
        placed: [f64::NAN; 9],
    }
}

/// # Safety
/// Main thread only.
unsafe fn load(web: &WKWebView, load: &Load) {
    match load {
        Load::Html(html) => unsafe {
            web.loadHTMLString_baseURL(&NSString::from_str(html), None);
        },
        Load::Url(url) => {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
                unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&url)) };
            }
        }
    }
}

/// Runs `script` in a web view's top document and returns its string result.
pub(super) async fn evaluate(web: &WKWebView, script: &str, cx: &gpui::AsyncApp) -> Option<String> {
    let (send, receive) = async_channel::bounded(1);
    let block = block2::RcBlock::new(
        move |value: *mut objc2::runtime::AnyObject, _: *mut NSError| {
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
        if started.elapsed() > Duration::from_secs(3) {
            return None;
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

/// The page as a PNG, `width` points wide.
pub(super) async fn png(
    web: &Retained<WKWebView>,
    width: f64,
    cx: &gpui::AsyncApp,
) -> Result<(Vec<u8>, (u32, u32)), String> {
    let mtm = MainThreadMarker::new().ok_or("Drafts render on the main thread")?;
    let (send, receive) = async_channel::bounded(1);
    let block = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        let encoded = unsafe { image.as_ref() }
            .and_then(|image| image.TIFFRepresentation())
            .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
            .and_then(|rep| {
                let size = (rep.pixelsWide() as u32, rep.pixelsHigh() as u32);
                let data = unsafe {
                    rep.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                }?;
                Some((data.to_vec(), size))
            });
        let _ = send.try_send(encoded);
    });
    unsafe {
        let config = WKSnapshotConfiguration::new(mtm);
        config.setSnapshotWidth(Some(&NSNumber::new_f64(width)));
        web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
    }
    let started = std::time::Instant::now();
    loop {
        if let Ok(value) = receive.try_recv() {
            return value.ok_or_else(|| "The page could not be captured".to_string());
        }
        if started.elapsed() > Duration::from_secs(8) {
            return Err("The page could not be captured".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
}

fn still_of(web: &WKWebView, cover: Rc<RefCell<Cover>>, done_notify: impl Fn() + 'static) {
    let done = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        let mut state = cover.borrow_mut();
        if !std::mem::take(&mut state.pending) {
            return;
        }
        // SAFETY: WebKit passes a live image or null for the duration of the call.
        state.snapshot = unsafe { image.as_ref() }.and_then(pixels).map(Arc::new);
        drop(state);
        done_notify();
    });
    unsafe { web.takeSnapshotWithConfiguration_completionHandler(None, &done) };
}

/// Copies an 8-bit RGB(A) snapshot into the BGRA layout GPUI's sprites use.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_scales_to_its_column_but_never_past_life_size() {
        // A desktop mock in a narrow column shrinks by width.
        assert_eq!(fit(1280, (320., 900.)).scale, 0.25);
        // Height never limits it: the thread scrolls.
        assert!((fit(375, (300., 10.)).scale - 0.8).abs() < 1e-6);
        // Small drafts stay at life size.
        assert_eq!(fit(300, (600., 600.)).scale, 1.);
        // An absurdly small column stays drawable.
        assert!(fit(1600, (1., 1.)).scale > 0.);
    }

    #[test]
    fn dev_server_links_are_local_only() {
        assert!(is_local("http://localhost:5173/x"));
        assert!(is_local("http://127.0.0.1:3000"));
        assert!(!is_local("https://example.com"));
        assert!(!is_local("http://localhost.evil.com"));
    }
}
