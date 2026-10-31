//! The device's picture: an `AVSampleBufferDisplayLayer` in a native view
//! placed over the window where the panel's canvas is.
//!
//! This is what keeps the app at the display's full rate while a device
//! streams. Frames go from the stream thread straight to the layer, the
//! media engine decodes them, and Core Animation composites the layer beside
//! GPUI's own. A new frame never asks GPUI to draw; GPUI draws only when its
//! own content changes, and then this canvas only re-places the view (and
//! skips even that when nothing moved).
//!
//! Input is read by the view itself and sent to the stream's input thread,
//! so touches don't wait on GPUI's event loop either.
use super::stream::{Input, InputSender, Sink};
use crate::draft::{Overlay, Painted, mark_painted, track_overlay};
use gpui::{Window, canvas, prelude::*};
use monocode_core::device::protocol::{Modifiers, ParameterSets, Phase};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::AnyObject, sel,
};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventPhase, NSView, NSWindowOrderingMode};
use objc2_av_foundation::{AVLayerVideoGravityResize, AVSampleBufferDisplayLayer};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFMutableDictionary, CFRetained, kCFBooleanTrue};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    CMVideoFormatDescriptionGetDimensions, kCMBlockBufferAssureMemoryNowFlag,
    kCMSampleAttachmentKey_DisplayImmediately,
};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The layer and the format of the stream it is showing. Fed from the
/// stream thread: `AVQueuedSampleBufferRendering` is meant to be fed from a
/// queue of the caller's choosing.
pub(crate) struct Renderer {
    layer: Retained<AVSampleBufferDisplayLayer>,
    /// The layer's `AVSampleBufferVideoRenderer` where there is one (macOS
    /// 14), which can hand back the frame on screen; the layer otherwise.
    target: Retained<AnyObject>,
    format: Mutex<Option<CFRetained<CMFormatDescription>>>,
}

// SAFETY: the layer is created on the main thread and only its sample-buffer
// methods, which AVFoundation allows off the main thread, are called elsewhere.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

/// `AVQueuedSampleBufferRenderingStatus.failed`.
const STATUS_FAILED: isize = 2;

impl Renderer {
    fn new(_: MainThreadMarker) -> Self {
        let layer: Retained<AVSampleBufferDisplayLayer> =
            unsafe { msg_send![AVSampleBufferDisplayLayer::class(), new] };
        if let Some(gravity) = unsafe { AVLayerVideoGravityResize } {
            unsafe { layer.setVideoGravity(gravity) };
        }
        let target: Retained<AnyObject> = if layer.respondsToSelector(sel!(sampleBufferRenderer)) {
            unsafe { msg_send![&*layer, sampleBufferRenderer] }
        } else {
            unsafe { Retained::cast_unchecked(layer.clone()) }
        };
        Self {
            layer,
            target,
            format: Mutex::new(None),
        }
    }

    /// The frame on screen, for a still while a popover covers the view.
    fn displayed(&self) -> Option<CFRetained<CVPixelBuffer>> {
        let responds: bool =
            unsafe { msg_send![&*self.target, respondsToSelector: sel!(copyDisplayedPixelBuffer)] };
        if !responds {
            return None;
        }
        let buffer: *mut CVPixelBuffer = unsafe { msg_send![&*self.target, copyDisplayedPixelBuffer] };
        // A `copy` method returns a reference the caller owns.
        NonNull::new(buffer).map(|buffer| unsafe { CFRetained::from_raw(buffer) })
    }
}

impl Sink for Renderer {
    fn configure(&self, sets: &ParameterSets) -> Result<(u32, u32), String> {
        let units: Vec<&Vec<u8>> = sets.sps.iter().chain(&sets.pps).collect();
        let mut pointers: Vec<NonNull<u8>> = units
            .iter()
            .map(|unit| NonNull::new(unit.as_ptr().cast_mut()).expect("vec pointer"))
            .collect();
        let mut sizes: Vec<usize> = units.iter().map(|unit| unit.len()).collect();
        let mut format: *const CMFormatDescription = std::ptr::null();
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                units.len(),
                NonNull::new(pointers.as_mut_ptr()).expect("vec pointer"),
                NonNull::new(sizes.as_mut_ptr()).expect("vec pointer"),
                4,
                NonNull::from(&mut format),
            )
        };
        let format = NonNull::new(format.cast_mut())
            .filter(|_| status == 0)
            .ok_or_else(|| format!("This device's video cannot be decoded (error {status})."))?;
        // SAFETY: a `Create` function returns a reference the caller owns.
        let format = unsafe { CFRetained::from_raw(format) };
        let dimensions = unsafe { CMVideoFormatDescriptionGetDimensions(&format) };
        *self.format.lock().unwrap_or_else(|e| e.into_inner()) = Some(format);
        Ok((dimensions.width.max(1) as u32, dimensions.height.max(1) as u32))
    }

    fn frame(&self, avcc: &[u8]) -> bool {
        let status: isize = unsafe { msg_send![&*self.target, status] };
        if status == STATUS_FAILED {
            return false;
        }
        let format = self.format.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(format) = format else {
            return false;
        };
        let Some(sample) = (unsafe { sample(&format, avcc) }) else {
            return false;
        };
        let _: () = unsafe { msg_send![&*self.target, enqueueSampleBuffer: &*sample] };
        #[cfg(debug_assertions)]
        super::counters::count(&super::counters::FRAMES_QUEUED);
        true
    }

    fn flush(&self) {
        let _: () = unsafe { msg_send![&*self.target, flush] };
    }
}

/// One access unit as a sample marked to show the moment it decodes: no
/// timebase, no buffering, so a frame waits only for the next display
/// refresh.
unsafe fn sample(format: &CMFormatDescription, data: &[u8]) -> Option<CFRetained<CMSampleBuffer>> {
    unsafe {
        let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
        let status = CMBlockBuffer::create_with_memory_block(
            None,
            std::ptr::null_mut(),
            data.len(),
            None,
            std::ptr::null(),
            0,
            data.len(),
            kCMBlockBufferAssureMemoryNowFlag,
            NonNull::from(&mut block),
        );
        let block = CFRetained::from_raw(NonNull::new(block).filter(|_| status == 0)?);
        if CMBlockBuffer::replace_data_bytes(
            NonNull::new(data.as_ptr().cast_mut().cast::<c_void>())?,
            &block,
            0,
            data.len(),
        ) != 0
        {
            return None;
        }
        let size = data.len();
        let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
        let status = CMSampleBuffer::create_ready(
            None,
            Some(&block),
            Some(format),
            1,
            0,
            std::ptr::null(),
            1,
            &size,
            NonNull::from(&mut sample),
        );
        let sample = CFRetained::from_raw(NonNull::new(sample).filter(|_| status == 0)?);
        if let Some(attachments) = sample.sample_attachments_array(true)
            && attachments.count() > 0
        {
            let dictionary = attachments.value_at_index(0).cast::<CFDictionary>().cast_mut();
            let dictionary = dictionary.cast::<CFMutableDictionary>();
            if let (Some(dictionary), Some(yes)) = (dictionary.as_ref(), kCFBooleanTrue) {
                CFMutableDictionary::set_value(
                    Some(dictionary),
                    (kCMSampleAttachmentKey_DisplayImmediately as *const objc2_core_foundation::CFString)
                        .cast(),
                    (yes as *const CFBoolean).cast(),
                );
            }
        }
        Some(sample)
    }
}

pub(crate) struct ViewState {
    input: RefCell<Option<InputSender>>,
    /// Where the current trackpad scroll's drag has got to, 0–1.
    scroll: Cell<Option<(f64, f64)>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "MonocodeDeviceScreen"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewState]
    struct ScreenView;
    unsafe impl NSObjectProtocol for ScreenView {}
    impl ScreenView {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool {
            true
        }
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }
        /// A click in an inactive window still reaches the device.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            self.touch(event, Phase::Begin);
        }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.touch(event, Phase::Move);
        }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.touch(event, Phase::End);
        }
        /// Two fingers on a trackpad drag the device's content, as a finger
        /// would; momentum is the device's own.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.scroll(event);
        }
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.key(event);
        }
        #[unsafe(method(keyUp:))]
        fn key_up(&self, _event: &NSEvent) {}
        /// Command shortcuts stay the app's, except ⌘V while the device has
        /// focus, which types the clipboard into it.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            let flags = event.modifierFlags();
            let focused = self.window().is_some_and(|window| {
                window
                    .firstResponder()
                    .is_some_and(|responder| std::ptr::eq(&*responder as *const _ as *const c_void, self as *const _ as *const c_void))
            });
            let paste = focused
                && flags.contains(NSEventModifierFlags::Command)
                && !flags.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option)
                && event
                    .charactersIgnoringModifiers()
                    .is_some_and(|key| key.to_string().eq_ignore_ascii_case("v"));
            if paste && let Some(text) = clipboard_text() {
                self.send(Input::Text(text));
            }
            paste
        }
    }
);

fn clipboard_text() -> Option<String> {
    let kind = objc2_foundation::NSString::from_str("public.utf8-plain-text");
    let board: Option<Retained<AnyObject>> =
        unsafe { msg_send![objc2::class!(NSPasteboard), generalPasteboard] };
    let text: Option<Retained<objc2_foundation::NSString>> =
        unsafe { msg_send![&*board?, stringForType: &*kind] };
    text.map(|text| text.to_string()).filter(|text| !text.is_empty())
}

impl ScreenView {
    fn send(&self, input: Input) {
        if let Some(sender) = self.ivars().input.borrow().as_ref() {
            sender.send(input);
        }
    }
    fn point(&self, event: &NSEvent) -> (f64, f64) {
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let bounds = self.bounds();
        (
            local.x / bounds.size.width.max(1.),
            local.y / bounds.size.height.max(1.),
        )
    }
    fn touch(&self, event: &NSEvent, phase: Phase) {
        let (x, y) = self.point(event);
        self.send(Input::Touch(phase, x, y));
    }
    fn scroll(&self, event: &NSEvent) {
        if !event.hasPreciseScrollingDeltas() || event.momentumPhase() != NSEventPhase::empty() {
            return;
        }
        let phase = event.phase();
        let bounds = self.bounds();
        let state = &self.ivars().scroll;
        if phase.contains(NSEventPhase::Began) || (state.get().is_none() && phase.contains(NSEventPhase::Changed)) {
            let start = self.point(event);
            state.set(Some(start));
            self.send(Input::Touch(Phase::Begin, start.0, start.1));
        }
        if let Some((x, y)) = state.get() {
            let next = (
                x + event.scrollingDeltaX() / bounds.size.width.max(1.),
                y + event.scrollingDeltaY() / bounds.size.height.max(1.),
            );
            if phase.contains(NSEventPhase::Changed) {
                state.set(Some(next));
                self.send(Input::Touch(Phase::Move, next.0, next.1));
            }
            if phase.intersects(NSEventPhase::Ended | NSEventPhase::Cancelled) {
                state.set(None);
                self.send(Input::Touch(Phase::End, next.0, next.1));
            }
        }
    }
    fn key(&self, event: &NSEvent) {
        let flags = event.modifierFlags();
        let modifiers = Modifiers {
            shift: flags.contains(NSEventModifierFlags::Shift),
            control: flags.contains(NSEventModifierFlags::Control),
            alt: flags.contains(NSEventModifierFlags::Option),
            command: flags.contains(NSEventModifierFlags::Command),
        };
        let special = match event.keyCode() {
            36 | 76 => Some("enter"),
            53 => Some("escape"),
            51 => Some("backspace"),
            48 => Some("tab"),
            49 => Some("space"),
            117 => Some("delete"),
            123 => Some("left"),
            124 => Some("right"),
            125 => Some("down"),
            126 => Some("up"),
            115 => Some("home"),
            119 => Some("end"),
            116 => Some("pageup"),
            121 => Some("pagedown"),
            _ => None,
        };
        let key = match special {
            Some(name) => name.to_owned(),
            None => event
                .charactersIgnoringModifiers()
                .map(|text| text.to_string().to_lowercase())
                .unwrap_or_default(),
        };
        if key.is_empty() {
            return;
        }
        let character = event.characters().map(|text| text.to_string());
        self.send(Input::Key {
            key,
            character,
            modifiers,
        });
    }
}

/// The view, its layer and the bookkeeping that places it.
pub(crate) struct Screen {
    view: Retained<ScreenView>,
    renderer: Arc<Renderer>,
    painted: Painted,
    /// Frame and corner radius as last applied, so an unchanged paint
    /// touches nothing native.
    placed: Cell<[f64; 5]>,
}

impl Overlay for Screen {
    fn step_aside(&self) {
        if !self.view.isHidden() {
            self.view.setHidden(true);
        }
    }
}

impl Screen {
    pub(crate) fn new() -> Option<Rc<Self>> {
        let mtm = MainThreadMarker::new()?;
        let renderer = Arc::new(Renderer::new(mtm));
        let view = ScreenView::alloc(mtm).set_ivars(ViewState {
            input: RefCell::new(None),
            scroll: Cell::new(None),
        });
        let view: Retained<ScreenView> = unsafe {
            msg_send![super(view), initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(1., 1.))]
        };
        // A layer-hosting view: the video layer is the view's own layer.
        let _: () = unsafe { msg_send![&*view, setLayer: &*renderer.layer] };
        view.setWantsLayer(true);
        renderer.layer.setMasksToBounds(true);
        view.setHidden(true);
        let screen = Rc::new(Self {
            view,
            renderer,
            painted: Default::default(),
            placed: Cell::new([f64::NAN; 5]),
        });
        let overlay: Rc<dyn Overlay> = screen.clone();
        track_overlay(&overlay, &screen.painted);
        Some(screen)
    }
    pub(crate) fn sink(&self) -> Arc<dyn Sink> {
        self.renderer.clone()
    }
    pub(crate) fn set_input(&self, sender: Option<InputSender>) {
        *self.view.ivars().input.borrow_mut() = sender;
    }
    /// Clears the picture, as when the device changes.
    pub(crate) fn clear(&self) {
        #[allow(deprecated)]
        unsafe {
            self.renderer.layer.flushAndRemoveImage();
        }
    }

    /// Fills its parent with the largest `aspect` (width over height) rect
    /// that fits, paints `backdrop` there for while no frame has arrived,
    /// and places the device's view over it. While a popover covers it, the
    /// view steps aside and the frame it was showing is painted instead.
    /// `live` false keeps the view aside, as while the device is off.
    pub(crate) fn element(
        self: &Rc<Self>,
        aspect: f32,
        corner: f32,
        backdrop: gpui::Hsla,
        live: bool,
    ) -> gpui::AnyElement {
        let screen = self.clone();
        canvas(
            move |bounds, window, _| {
                let fitted = fit(bounds, aspect);
                (fitted, window.insert_hitbox(fitted, gpui::HitboxBehavior::Normal))
            },
            move |_, (fitted, hitbox), window, _| {
                #[cfg(debug_assertions)]
                super::counters::count(&super::counters::SCREEN_PAINTS);
                mark_painted(&screen.painted, window);
                window.paint_quad(gpui::fill(fitted, backdrop).corner_radii(gpui::px(corner)));
                if !live {
                    screen.step_aside();
                    return;
                }
                if window.is_hitbox_covered(&hitbox) {
                    screen.step_aside();
                    if let Some(still) = screen.renderer.displayed() {
                        window.paint_surface(fitted, still);
                    }
                    return;
                }
                screen.place(fitted, corner, window);
            },
        )
        .size_full()
        .into_any_element()
    }

    fn place(&self, bounds: gpui::Bounds<gpui::Pixels>, corner: f32, window: &mut Window) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // GPUI keeps its AppKit content view alive throughout paint.
        let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let zoom = window.content_zoom();
        // The view is clipped to what is visible, as the panel scrolls or
        // slides; the picture keeps its size, so a clipped view would
        // squeeze it. Step aside instead until it is whole again.
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.size.width < bounds.size.width - gpui::px(0.5)
            || visible.size.height < bounds.size.height - gpui::px(0.5)
        {
            self.step_aside();
            return;
        }
        let bounds = bounds.map(|v| v * zoom);
        let (x, y, width, height) = (
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.y),
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
        );
        let y = if parent.isFlipped() {
            y
        } else {
            parent.bounds().size.height - y - height
        };
        let attached =
            unsafe { self.view.superview() }.is_some_and(|sup| std::ptr::eq(&*sup, parent));
        if !attached {
            parent.addSubview_positioned_relativeTo(&self.view, NSWindowOrderingMode::Above, None);
        }
        let placed = [x, y, width, height, f64::from(corner * zoom)];
        if self.placed.get() != placed {
            self.view
                .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
            self.renderer.layer.setCornerRadius(placed[4]);
            self.placed.set(placed);
        }
        if self.view.isHidden() {
            self.view.setHidden(false);
        }
    }
}

/// The largest rect of `aspect` centred in `bounds`, on whole points so the
/// native view lands on the same pixels GPUI paints.
pub(crate) fn fit(bounds: gpui::Bounds<gpui::Pixels>, aspect: f32) -> gpui::Bounds<gpui::Pixels> {
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let aspect = aspect.max(0.01);
    let (w, h) = if width / height.max(1.) > aspect {
        (height * aspect, height)
    } else {
        (width, width / aspect)
    };
    let (w, h) = (w.floor(), h.floor());
    gpui::Bounds::new(
        gpui::point(
            bounds.origin.x + gpui::px(((width - w) / 2.).floor()),
            bounds.origin.y + gpui::px(((height - h) / 2.).floor()),
        ),
        gpui::size(gpui::px(w), gpui::px(h)),
    )
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.set_input(None);
        self.view.removeFromSuperview();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::stream::{self, Event};
    use monocode_core::device::{self, Platform, hub};
    use std::{
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    /// The real video layer, plus the stream written out as Annex B so
    /// ffmpeg can check every frame decodes.
    struct Capture {
        renderer: Renderer,
        file: Mutex<std::fs::File>,
        frames: AtomicUsize,
        refused: AtomicUsize,
        bytes: AtomicUsize,
        times: Mutex<Vec<Instant>>,
    }
    impl Sink for Capture {
        fn configure(&self, sets: &ParameterSets) -> Result<(u32, u32), String> {
            let mut file = self.file.lock().unwrap();
            for unit in sets.sps.iter().chain(&sets.pps) {
                file.write_all(&[0, 0, 0, 1]).unwrap();
                file.write_all(unit).unwrap();
            }
            self.renderer.configure(sets)
        }
        fn frame(&self, avcc: &[u8]) -> bool {
            let mut file = self.file.lock().unwrap();
            let mut rest = avcc;
            while rest.len() >= 4 {
                let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
                file.write_all(&[0, 0, 0, 1]).unwrap();
                file.write_all(&rest[4..4 + length]).unwrap();
                rest = &rest[4 + length..];
            }
            self.bytes.fetch_add(avcc.len(), Ordering::Relaxed);
            self.times.lock().unwrap().push(Instant::now());
            let shown = self.renderer.frame(avcc);
            let counter = if shown { &self.frames } else { &self.refused };
            counter.fetch_add(1, Ordering::Relaxed);
            shown
        }
        fn flush(&self) {
            self.renderer.flush()
        }
    }

    /// Streams the booted iOS Simulator for a few seconds while swiping its
    /// home screen: `cargo test live_stream -- --ignored --nocapture`. Set
    /// `MONOCODE_DEVICE_CAPTURE` to keep the stream as an `.h264` file.
    #[test]
    #[ignore = "runs the real Device Hub against a booted simulator"]
    fn live_stream_reaches_the_video_layer() {
        device::set_enabled(true, false);
        let endpoint = hub::ensure().expect("the hub starts");
        let list = hub::list().expect("the hub lists devices");
        let platform = match std::env::var("MONOCODE_DEVICE_PLATFORM").as_deref() {
            Ok("android") => Platform::Android,
            _ => Platform::Ios,
        };
        let mut target = list
            .devices
            .iter()
            .filter(|d| d.platform == platform && d.supported)
            .max_by_key(|d| d.booted)
            .expect("a device on that platform")
            .clone();
        if !target.booted {
            let booting = Instant::now();
            target.id = hub::boot(&target).expect("the device boots");
            eprintln!("booted {} as {} in {:?}", target.name, target.id, booting.elapsed());
        }
        if platform == Platform::Ios {
            hub::attach(&endpoint, &target.id).expect("the simulator attaches");
        }
        let path = std::env::var_os("MONOCODE_DEVICE_CAPTURE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("monocode-device-capture.h264"));
        let sink = Arc::new(Capture {
            // SAFETY: a test thread stands in for the main thread; the layer
            // is never put on screen here.
            renderer: Renderer::new(unsafe { MainThreadMarker::new_unchecked() }),
            file: Mutex::new(std::fs::File::create(&path).unwrap()),
            frames: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            times: Mutex::new(Vec::new()),
        });
        let started = Instant::now();
        let live = stream::start(endpoint, platform, target.id.clone(), sink.clone())
            .expect("the stream starts");
        let mut first_frame = None;
        let mut input = None;
        let mut size = None;
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline && (first_frame.is_none() || input.is_none()) {
            while let Ok(event) = live.events.try_recv() {
                eprintln!("{:>7.0?} {event:?}", started.elapsed());
                match event {
                    Event::Streaming => first_frame = Some(started.elapsed()),
                    Event::Input(true, _) => input = Some(started.elapsed()),
                    Event::Size(width, height) => size = Some((width, height)),
                    _ => (),
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(first_frame.is_some(), "a frame arrives");
        assert!(input.is_some(), "input is admitted");
        // Go home, then swipe between its pages so the picture keeps changing.
        live.send(stream::Input::Button(monocode_core::device::protocol::Button::Home));
        std::thread::sleep(Duration::from_millis(1500));
        let before = sink.frames.load(Ordering::Relaxed);
        let window = Instant::now();
        for round in 0..4 {
            let (from, to) = if round % 2 == 0 { (0.85, 0.15) } else { (0.15, 0.85) };
            live.send(stream::Input::Touch(Phase::Begin, from, 0.5));
            for step in 1..=12 {
                let x = from + (to - from) * step as f64 / 12.;
                std::thread::sleep(Duration::from_millis(16));
                live.send(stream::Input::Touch(Phase::Move, x, 0.5));
            }
            live.send(stream::Input::Touch(Phase::End, to, 0.5));
            std::thread::sleep(Duration::from_millis(600));
        }
        let elapsed = window.elapsed().as_secs_f64();
        // The busiest quarter second: the rate while the page animates.
        let times = sink.times.lock().unwrap().clone();
        let peak = times
            .iter()
            .map(|start| times.iter().filter(|t| **t >= *start && t.duration_since(*start) < Duration::from_millis(250)).count())
            .max()
            .unwrap_or(0);
        let gaps: Vec<f64> = times.windows(2).map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.).filter(|gap| *gap < 100.).collect();
        let mean_gap = gaps.iter().sum::<f64>() / gaps.len().max(1) as f64;
        eprintln!("peak {} fps over 250 ms; mean gap between consecutive frames while moving {mean_gap:.1} ms", peak * 4);
        let frames = sink.frames.load(Ordering::Relaxed) - before;
        drop(live);
        hub::stop();
        eprintln!(
            "size {size:?}, first frame {first_frame:?}, input {input:?}, {frames} frames in {elapsed:.1}s ({:.0} fps while moving), {} refused, {} KB, capture at {}",
            frames as f64 / elapsed,
            sink.refused.load(Ordering::Relaxed),
            sink.bytes.load(Ordering::Relaxed) / 1024,
            path.display()
        );
        assert!(size.is_some(), "the picture has a size");
        assert_eq!(sink.refused.load(Ordering::Relaxed), 0, "the layer takes every frame");
        assert!(frames > 30, "swiping streams frames");
    }
}
