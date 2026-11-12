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
use super::stream::{Input, InputSender, Queued, Sink};
use crate::draft::{Overlay, Painted, mark_painted, track_overlay};
use gpui::{Window, canvas, prelude::*};
use monocode_core::device::protocol::{Modifiers, ParameterSets, Phase, Rotation};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::AnyObject, sel,
};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSEventPhase, NSView, NSWindowOcclusionState,
    NSWindowOrderingMode,
};
use objc2_av_foundation::{AVLayerVideoGravityResize, AVSampleBufferDisplayLayer};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFMutableDictionary, CFRetained, CGAffineTransform, CGPoint, CGRect,
    CGSize, kCFBooleanTrue,
};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer,
    CMVideoFormatDescriptionCreateFromH264ParameterSets, CMVideoFormatDescriptionGetDimensions,
    kCMSampleAttachmentKey_DisplayImmediately,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CATransaction};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
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
    /// Whether anyone can see the layer; set by its view on the main thread.
    showing: Arc<AtomicBool>,
    /// Samples given to the layer that it still holds, counted down as
    /// CoreMedia frees each one's data (`free_block`).
    held: Arc<AtomicUsize>,
}

/// The most samples the layer may hold before frames are dropped. A layer
/// that keeps up lets each go once it decodes, so this is a backlog: the
/// layer takes everything it is given and `isReadyForMoreMediaData` stays
/// true (measured: 240 queued at once), and its queue's upkeep grows with
/// its length, so a backlog left alone only grows.
const MAX_HELD: usize = 30;

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
            showing: Arc::new(AtomicBool::new(false)),
            held: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The frame on screen, for a still while a popover covers the view.
    pub(super) fn displayed(&self) -> Option<CFRetained<CVPixelBuffer>> {
        let responds: bool =
            unsafe { msg_send![&*self.target, respondsToSelector: sel!(copyDisplayedPixelBuffer)] };
        if !responds {
            return None;
        }
        let buffer: *mut CVPixelBuffer =
            unsafe { msg_send![&*self.target, copyDisplayedPixelBuffer] };
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
        Ok((
            dimensions.width.max(1) as u32,
            dimensions.height.max(1) as u32,
        ))
    }

    fn frame(&self, avcc: &[u8]) -> Queued {
        let status: isize = unsafe { msg_send![&*self.target, status] };
        if status == STATUS_FAILED {
            return Queued::Failed;
        }
        // The layer queues whatever it is given; past this it isn't keeping
        // up, and every frame more is memory it holds.
        let ready: bool = unsafe { msg_send![&*self.target, isReadyForMoreMediaData] };
        if !ready || self.held.load(Ordering::Relaxed) >= MAX_HELD {
            return Queued::Full;
        }
        let format = self
            .format
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(format) = format else {
            return Queued::Failed;
        };
        let Some(sample) = (unsafe { sample(&format, avcc, &self.held, None) }) else {
            return Queued::Failed;
        };
        let _: () = unsafe { msg_send![&*self.target, enqueueSampleBuffer: &*sample] };
        #[cfg(debug_assertions)]
        super::counters::count(&super::counters::FRAMES_QUEUED);
        Queued::Yes
    }

    fn flush(&self) {
        let _: () = unsafe { msg_send![&*self.target, flush] };
    }

    fn showing(&self) -> bool {
        self.showing.load(Ordering::Relaxed)
    }
}

/// Whether GPUI's surface pipeline paints this pixel format; it asserts on
/// any other.
fn paintable(format: u32) -> bool {
    format == kCVPixelFormatType_32BGRA || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTPixelTransferSessionCreate(allocator: *const c_void, session: *mut *mut c_void) -> i32;
    fn VTPixelTransferSessionTransferImage(
        session: *mut c_void,
        source: *const CVPixelBuffer,
        destination: *const CVPixelBuffer,
    ) -> i32;
    fn VTPixelTransferSessionInvalidate(session: *mut c_void);
    fn VTPixelRotationSessionCreate(allocator: *const c_void, session: *mut *mut c_void) -> i32;
    fn VTPixelRotationSessionRotateImage(
        session: *mut c_void,
        source: *const CVPixelBuffer,
        destination: *const CVPixelBuffer,
    ) -> i32;
    fn VTPixelRotationSessionInvalidate(session: *mut c_void);
    fn VTSessionSetProperty(session: *mut c_void, key: *const c_void, value: *const c_void) -> i32;
    static kVTPixelRotationPropertyKey_Rotation: *const c_void;
    static kVTRotation_CW90: *const c_void;
    static kVTRotation_180: *const c_void;
    static kVTRotation_CCW90: *const c_void;
}

unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// `frame` in a format GPUI can paint. The decoder hands back video-range
/// YCbCr, which GPUI's surface shader does not take, so VideoToolbox
/// converts it to BGRA using the frame's own colour matrix and range.
fn paintable_frame(frame: CFRetained<CVPixelBuffer>) -> Option<CFRetained<CVPixelBuffer>> {
    if paintable(CVPixelBufferGetPixelFormatType(&frame)) {
        return Some(frame);
    }
    let converted = bgra_buffer(
        CVPixelBufferGetWidth(&frame),
        CVPixelBufferGetHeight(&frame),
    )?;
    unsafe {
        let mut session: *mut c_void = std::ptr::null_mut();
        if VTPixelTransferSessionCreate(std::ptr::null(), &mut session) != 0 || session.is_null() {
            return None;
        }
        let status = VTPixelTransferSessionTransferImage(
            session,
            CFRetained::as_ptr(&frame).as_ptr(),
            CFRetained::as_ptr(&converted).as_ptr(),
        );
        VTPixelTransferSessionInvalidate(session);
        CFRelease(session);
        (status == 0).then_some(converted)
    }
}

/// `frame` turned by `rotation`, for a still of a turned picture: GPUI
/// paints a surface only upright. `frame` is BGRA, as `paintable_frame`
/// leaves it.
fn turned_frame(
    frame: CFRetained<CVPixelBuffer>,
    rotation: Rotation,
) -> Option<CFRetained<CVPixelBuffer>> {
    let angle = unsafe {
        match rotation {
            Rotation::None => return Some(frame),
            Rotation::Clockwise => kVTRotation_CW90,
            Rotation::Half => kVTRotation_180,
            Rotation::CounterClockwise => kVTRotation_CCW90,
        }
    };
    let (width, height) = rotation.turned(
        CVPixelBufferGetWidth(&frame),
        CVPixelBufferGetHeight(&frame),
    );
    let turned = bgra_buffer(width, height)?;
    unsafe {
        let mut session: *mut c_void = std::ptr::null_mut();
        if VTPixelRotationSessionCreate(std::ptr::null(), &mut session) != 0 || session.is_null() {
            return None;
        }
        let status =
            if VTSessionSetProperty(session, kVTPixelRotationPropertyKey_Rotation, angle) == 0 {
                VTPixelRotationSessionRotateImage(
                    session,
                    CFRetained::as_ptr(&frame).as_ptr(),
                    CFRetained::as_ptr(&turned).as_ptr(),
                )
            } else {
                -1
            };
        VTPixelRotationSessionInvalidate(session);
        CFRelease(session);
        (status == 0).then_some(turned)
    }
}

/// An empty BGRA buffer GPUI can paint.
pub(super) fn bgra_buffer(width: usize, height: usize) -> Option<CFRetained<CVPixelBuffer>> {
    // GPUI makes a Metal texture of it, which needs IOSurface backing.
    let surface =
        CFDictionary::<objc2_core_foundation::CFString, objc2_core_foundation::CFType>::empty();
    let attributes = unsafe {
        CFDictionary::<objc2_core_foundation::CFString, objc2_core_foundation::CFType>::from_slices(
            &[
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVPixelBufferMetalCompatibilityKey,
            ],
            &[surface.as_ref(), kCFBooleanTrue?.as_ref()],
        )
    };
    let mut converted: *mut CVPixelBuffer = std::ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreate(
            None,
            width,
            height,
            kCVPixelFormatType_32BGRA,
            Some(attributes.as_opaque()),
            NonNull::from(&mut converted),
        )
    };
    // SAFETY: a `Create` function returns a reference the caller owns.
    Some(unsafe {
        CFRetained::from_raw(NonNull::new(converted).filter(|_| status == kCVReturnSuccess)?)
    })
}

/// Frees a sample's data once CoreMedia lets go of it, and counts it off
/// the renderer's `held`, whose `Arc` it was given as `refcon`.
unsafe extern "C-unwind" fn free_block(refcon: *mut c_void, memory: NonNull<c_void>, _: usize) {
    unsafe {
        libc::free(memory.as_ptr());
        let held = Arc::from_raw(refcon.cast_const().cast::<AtomicUsize>());
        held.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One access unit as a sample marked to show the moment it decodes: no
/// timebase, no buffering, so a frame waits only for the next display
/// refresh. `held` counts it until CoreMedia frees it. `timing` is for
/// measuring the queue only.
unsafe fn sample(
    format: &CMFormatDescription,
    data: &[u8],
    held: &Arc<AtomicUsize>,
    timing: Option<objc2_core_media::CMSampleTimingInfo>,
) -> Option<CFRetained<CMSampleBuffer>> {
    unsafe {
        let memory = libc::malloc(data.len().max(1));
        let memory = NonNull::new(memory)?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), memory.as_ptr().cast::<u8>(), data.len());
        let refcon = Arc::into_raw(held.clone());
        let source = objc2_core_media::CMBlockBufferCustomBlockSource {
            version: 0,
            AllocateBlock: None,
            FreeBlock: Some(free_block),
            refCon: refcon.cast_mut().cast(),
        };
        let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
        let status = CMBlockBuffer::create_with_memory_block(
            None,
            memory.as_ptr(),
            data.len(),
            None,
            &source,
            0,
            data.len(),
            0,
            NonNull::from(&mut block),
        );
        let Some(block) = NonNull::new(block).filter(|_| status == 0) else {
            // CoreMedia took nothing; the data and the count are still ours.
            libc::free(memory.as_ptr());
            drop(Arc::from_raw(refcon));
            return None;
        };
        held.fetch_add(1, Ordering::Relaxed);
        let block = CFRetained::from_raw(block);
        let size = data.len();
        let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
        let status = CMSampleBuffer::create_ready(
            None,
            Some(&block),
            Some(format),
            1,
            timing.is_some() as _,
            timing.as_ref().map_or(std::ptr::null(), |t| t as *const _),
            1,
            &size,
            NonNull::from(&mut sample),
        );
        let sample = CFRetained::from_raw(NonNull::new(sample).filter(|_| status == 0)?);
        if let Some(attachments) = sample.sample_attachments_array(true)
            && attachments.count() > 0
        {
            let dictionary = attachments
                .value_at_index(0)
                .cast::<CFDictionary>()
                .cast_mut();
            let dictionary = dictionary.cast::<CFMutableDictionary>();
            if let (Some(dictionary), Some(yes)) = (dictionary.as_ref(), kCFBooleanTrue) {
                CFMutableDictionary::set_value(
                    Some(dictionary),
                    (kCMSampleAttachmentKey_DisplayImmediately
                        as *const objc2_core_foundation::CFString)
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
    /// The renderer's: placed, unhidden, in a window that is on a lit
    /// screen.
    showing: Arc<AtomicBool>,
    /// Whether the view's window can be seen at all, as last told to the
    /// panel.
    window_seen: Cell<Option<bool>>,
    seen: async_channel::Sender<bool>,
}

/// Every display is asleep. AppKit may not mark a window occluded for it.
static SCREENS_ASLEEP: AtomicBool = AtomicBool::new(false);

const OCCLUSION_CHANGED: &str = "NSWindowDidChangeOcclusionStateNotification";
const SCREENS_SLEPT: &str = "NSWorkspaceScreensDidSleepNotification";
const SCREENS_WOKE: &str = "NSWorkspaceScreensDidWakeNotification";

fn notification_center() -> Retained<AnyObject> {
    unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] }
}

fn workspace_center() -> Retained<AnyObject> {
    let workspace: Retained<AnyObject> =
        unsafe { msg_send![objc2::class!(NSWorkspace), sharedWorkspace] };
    unsafe { msg_send![&*workspace, notificationCenter] }
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
        /// Follows the window's occlusion: minimised, covered, on another
        /// Space or with the app hidden, nothing composites the picture.
        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            let center = notification_center();
            let name = objc2_foundation::NSString::from_str(OCCLUSION_CHANGED);
            let none: Option<&AnyObject> = None;
            let _: () =
                unsafe { msg_send![&*center, removeObserver: self, name: &*name, object: none] };
            if let Some(window) = self.window() {
                let _: () = unsafe {
                    msg_send![&*center, addObserver: self, selector: sel!(monocodeSeenChanged:), name: &*name, object: &*window]
                };
            }
            self.refresh_seen();
        }
        #[unsafe(method(viewDidHide))]
        fn did_hide(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidHide] };
            self.refresh_seen();
        }
        #[unsafe(method(viewDidUnhide))]
        fn did_unhide(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidUnhide] };
            self.refresh_seen();
        }
        #[unsafe(method(monocodeSeenChanged:))]
        fn seen_changed(&self, _notification: &AnyObject) {
            self.refresh_seen();
        }
        #[unsafe(method(monocodeScreensSlept:))]
        fn screens_slept(&self, _notification: &AnyObject) {
            SCREENS_ASLEEP.store(true, Ordering::Relaxed);
            self.refresh_seen();
        }
        #[unsafe(method(monocodeScreensWoke:))]
        fn screens_woke(&self, _notification: &AnyObject) {
            SCREENS_ASLEEP.store(false, Ordering::Relaxed);
            self.refresh_seen();
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
    text.map(|text| text.to_string())
        .filter(|text| !text.is_empty())
}

impl ScreenView {
    /// Whether the window is on a lit screen and not occluded.
    fn window_seen(&self) -> bool {
        !SCREENS_ASLEEP.load(Ordering::Relaxed)
            && self.window().is_some_and(|window| {
                window
                    .occlusionState()
                    .contains(NSWindowOcclusionState::Visible)
            })
    }
    /// Tells the renderer whether anyone can see the picture, and the panel
    /// whether anyone can see the window. Coming into view asks the stream
    /// for a keyframe if it dropped frames meanwhile.
    fn refresh_seen(&self) {
        let state = self.ivars();
        let window = self.window().is_some().then(|| self.window_seen());
        if let Some(window) = window
            && state.window_seen.replace(Some(window)) != Some(window)
        {
            let _ = state.seen.try_send(window);
        }
        let showing = window == Some(true) && !self.isHiddenOrHasHiddenAncestor();
        if state.showing.swap(showing, Ordering::Relaxed) != showing
            && showing
            && let Some(input) = state.input.borrow().as_ref()
        {
            input.resync();
        }
    }
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
        if phase.contains(NSEventPhase::Began)
            || (state.get().is_none() && phase.contains(NSEventPhase::Changed))
        {
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
    /// The view's own layer, clipped to the picture's rounded rect; the
    /// video layer sits inside it, turned when the device's interface is
    /// turned against its framebuffer.
    container: Retained<CALayer>,
    renderer: Arc<Renderer>,
    painted: Painted,
    /// Frame, corner radius and turn as last applied, so an unchanged paint
    /// touches nothing native.
    placed: Cell<[f64; 6]>,
    /// The frame painted in the view's place while something covers it,
    /// taken once when the cover starts.
    still: RefCell<Option<CFRetained<CVPixelBuffer>>>,
    /// Whether the window can be seen, each time that changes.
    seen: async_channel::Receiver<bool>,
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
        let (seen_tx, seen) = async_channel::unbounded();
        let view = ScreenView::alloc(mtm).set_ivars(ViewState {
            input: RefCell::new(None),
            scroll: Cell::new(None),
            showing: renderer.showing.clone(),
            window_seen: Cell::new(None),
            seen: seen_tx,
        });
        let view: Retained<ScreenView> = unsafe {
            msg_send![super(view), initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(1., 1.))]
        };
        // A layer-hosting view: the container is the view's own layer.
        let container = CALayer::new();
        container.setMasksToBounds(true);
        container.addSublayer(&renderer.layer);
        let _: () = unsafe { msg_send![&*view, setLayer: &*container] };
        view.setWantsLayer(true);
        view.setHidden(true);
        let workspace = workspace_center();
        let none: Option<&AnyObject> = None;
        for (name, selector) in [
            (SCREENS_SLEPT, sel!(monocodeScreensSlept:)),
            (SCREENS_WOKE, sel!(monocodeScreensWoke:)),
        ] {
            let name = objc2_foundation::NSString::from_str(name);
            let _: () = unsafe {
                msg_send![&*workspace, addObserver: &*view, selector: selector, name: &*name, object: none]
            };
        }
        let screen = Rc::new(Self {
            view,
            container,
            renderer,
            painted: Default::default(),
            placed: Cell::new([f64::NAN; 6]),
            still: RefCell::new(None),
            seen,
        });
        let overlay: Rc<dyn Overlay> = screen.clone();
        track_overlay(&overlay, &screen.painted);
        Some(screen)
    }
    pub(crate) fn sink(&self) -> Arc<dyn Sink> {
        self.renderer.clone()
    }
    /// The layer's renderer, for the 3D model to take frames from.
    pub(super) fn renderer(&self) -> Arc<Renderer> {
        self.renderer.clone()
    }
    pub(super) fn view(&self) -> &NSView {
        &self.view
    }
    /// Places the view at `bounds` under whatever covers it there, as the
    /// 3D model does, so its layer keeps showing frames. False while the
    /// view would be clipped, when it steps aside.
    pub(super) fn place_beneath(
        &self,
        bounds: gpui::Bounds<gpui::Pixels>,
        rotation: Rotation,
        window: &mut Window,
    ) -> bool {
        self.still.take();
        self.place(bounds, 0., rotation, window)
    }
    pub(crate) fn set_input(&self, sender: Option<InputSender>) {
        *self.view.ivars().input.borrow_mut() = sender;
    }
    /// Whether the window can be seen, each time that changes: minimised,
    /// covered, on another Space, with the app hidden or the display asleep,
    /// it can't.
    pub(crate) fn window_seen(&self) -> async_channel::Receiver<bool> {
        self.seen.clone()
    }
    /// The stream stopped: lets go of the frames the layer still holds,
    /// keeping the one on screen.
    pub(crate) fn release(&self) {
        self.set_input(None);
        self.renderer.flush();
    }
    /// Clears the picture, as when the device changes.
    pub(crate) fn clear(&self) {
        self.still.take();
        #[allow(deprecated)]
        unsafe {
            self.renderer.layer.flushAndRemoveImage();
        }
    }

    /// Fills its parent with the largest `aspect` (width over height, as
    /// shown) rect that fits, paints `backdrop` there for while no frame has
    /// arrived, and places the device's view over it, its picture turned by
    /// `rotation`. While a popover covers it, the view steps aside and the
    /// frame it was showing is painted instead. `live` false keeps the view
    /// aside, as while the device is off.
    pub(crate) fn element(
        self: &Rc<Self>,
        aspect: f32,
        rotation: Rotation,
        corner: f32,
        backdrop: gpui::Hsla,
        live: bool,
    ) -> gpui::AnyElement {
        let screen = self.clone();
        canvas(
            move |bounds, window, _| {
                let fitted = fit(bounds, aspect);
                (
                    fitted,
                    window.insert_hitbox(fitted, gpui::HitboxBehavior::Normal),
                )
            },
            move |_, (fitted, hitbox), window, _| {
                #[cfg(debug_assertions)]
                super::counters::count(&super::counters::SCREEN_PAINTS);
                mark_painted(&screen.painted, window);
                window.paint_quad(gpui::fill(fitted, backdrop).corner_radii(gpui::px(corner)));
                if !live {
                    screen.step_aside();
                    screen.still.take();
                    return;
                }
                if window.is_hitbox_covered(&hitbox) {
                    screen.step_aside();
                    let mut still = screen.still.borrow_mut();
                    if still.is_none() {
                        *still = screen
                            .renderer
                            .displayed()
                            .and_then(paintable_frame)
                            .and_then(|frame| turned_frame(frame, rotation));
                    }
                    if let Some(still) = still.clone() {
                        window.paint_surface(fitted, still);
                    }
                    return;
                }
                screen.still.take();
                screen.place(fitted, corner, rotation, window);
            },
        )
        .size_full()
        .into_any_element()
    }

    fn place(
        &self,
        bounds: gpui::Bounds<gpui::Pixels>,
        corner: f32,
        rotation: Rotation,
        window: &mut Window,
    ) -> bool {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return false;
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
            return false;
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
        let placed = [
            x,
            y,
            width,
            height,
            f64::from(corner * zoom),
            rotation.radians(),
        ];
        if self.placed.get() != placed {
            self.view
                .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
            // The video layer is a sublayer, which Core Animation would
            // animate into place.
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            self.container.setCornerRadius(placed[4]);
            let (video_width, video_height) = rotation.turned(width, height);
            let video = &self.renderer.layer;
            video.setBounds(CGRect::new(
                CGPoint::new(0., 0.),
                CGSize::new(video_width, video_height),
            ));
            video.setPosition(CGPoint::new(width / 2., height / 2.));
            video.setAffineTransform(turn(rotation, self.container.contentsAreFlipped()));
            CATransaction::commit();
            self.placed.set(placed);
        }
        if self.view.isHidden() {
            self.view.setHidden(false);
        }
        true
    }
}

/// `rotation`, clockwise as seen, in a layer space that is `flipped` (y
/// down) or not (y up, where a positive angle turns counterclockwise).
fn turn(rotation: Rotation, flipped: bool) -> CGAffineTransform {
    let angle = if flipped {
        rotation.radians()
    } else {
        -rotation.radians()
    };
    let (sin, cos) = angle.sin_cos();
    // Exact quarter turns, so the picture lands on whole pixels.
    let (sin, cos) = (sin.round(), cos.round());
    CGAffineTransform {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        tx: 0.,
        ty: 0.,
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
        let _: () = unsafe { msg_send![&*workspace_center(), removeObserver: &*self.view] };
        let _: () = unsafe { msg_send![&*notification_center(), removeObserver: &*self.view] };
        self.view.removeFromSuperview();
        self.renderer.flush();
    }
}

#[cfg(test)]
mod still_tests {
    use super::*;
    use objc2_core_foundation::{CFString, CFType};
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetIOSurface,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    };

    /// A frame like the decoder's: video-range YCbCr, IOSurface-backed,
    /// filled with one luma and chroma value.
    fn video_range_frame(luma: u8, chroma: u8) -> CFRetained<CVPixelBuffer> {
        let attributes = CFDictionary::<CFString, CFType>::from_slices(
            &[unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
            &[CFDictionary::<CFString, CFType>::empty().as_ref()],
        );
        let mut frame: *mut CVPixelBuffer = std::ptr::null_mut();
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                64,
                128,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                Some(attributes.as_opaque()),
                NonNull::from(&mut frame),
            )
        };
        assert_eq!(status, kCVReturnSuccess);
        let frame = unsafe { CFRetained::from_raw(NonNull::new(frame).unwrap()) };
        unsafe {
            CVPixelBufferLockBaseAddress(&frame, CVPixelBufferLockFlags::empty());
            for (plane, rows, value) in [(0, 128, luma), (1, 64, chroma)] {
                let base = CVPixelBufferGetBaseAddressOfPlane(&frame, plane).cast::<u8>();
                let row = CVPixelBufferGetBytesPerRowOfPlane(&frame, plane);
                std::ptr::write_bytes(base, value, row * rows);
            }
            CVPixelBufferUnlockBaseAddress(&frame, CVPixelBufferLockFlags::empty());
        }
        frame
    }

    fn first_pixel(frame: &CVPixelBuffer) -> [u8; 4] {
        unsafe {
            CVPixelBufferLockBaseAddress(frame, CVPixelBufferLockFlags::ReadOnly);
            let base = CVPixelBufferGetBaseAddress(frame).cast::<u8>();
            let row = CVPixelBufferGetBytesPerRow(frame);
            let middle = base.add(row * 64 + 32 * 4);
            let pixel = [*middle, *middle.add(1), *middle.add(2), *middle.add(3)];
            CVPixelBufferUnlockBaseAddress(frame, CVPixelBufferLockFlags::ReadOnly);
            pixel
        }
    }

    #[test]
    fn decoder_frames_become_bgra_gpui_can_paint() {
        let frame = video_range_frame(235, 128);
        assert!(!paintable(CVPixelBufferGetPixelFormatType(&frame)));
        let still = paintable_frame(frame).expect("converted");
        assert_eq!(
            CVPixelBufferGetPixelFormatType(&still),
            kCVPixelFormatType_32BGRA
        );
        assert_eq!(
            (
                CVPixelBufferGetWidth(&still),
                CVPixelBufferGetHeight(&still)
            ),
            (64, 128)
        );
        // GPUI makes a Metal texture of it, which needs an IOSurface.
        assert!(CVPixelBufferGetIOSurface(Some(&still)).is_some());
        // Video-range white is white, not the grey a full-range reading gives.
        let [b, g, r, a] = first_pixel(&still);
        assert!(
            b >= 250 && g >= 250 && r >= 250 && a == 255,
            "{:?}",
            [b, g, r, a]
        );
        // Video-range black is black.
        let [b, g, r, _] = first_pixel(&paintable_frame(video_range_frame(16, 128)).unwrap());
        assert!(b <= 3 && g <= 3 && r <= 3, "{:?}", [b, g, r]);
    }

    /// The BGRA pixel at `(x, y)`, top-left first.
    fn pixel_at(frame: &CVPixelBuffer, x: usize, y: usize) -> [u8; 4] {
        unsafe {
            CVPixelBufferLockBaseAddress(frame, CVPixelBufferLockFlags::ReadOnly);
            let base = CVPixelBufferGetBaseAddress(frame).cast::<u8>();
            let at = base.add(CVPixelBufferGetBytesPerRow(frame) * y + x * 4);
            let pixel = [*at, *at.add(1), *at.add(2), *at.add(3)];
            CVPixelBufferUnlockBaseAddress(frame, CVPixelBufferLockFlags::ReadOnly);
            pixel
        }
    }

    /// A still of a turned picture is turned the way the live layer is: a
    /// clockwise turn puts the raw frame's top-left corner at the top right.
    #[test]
    fn stills_turn_with_the_picture() {
        let (width, height) = (8, 4);
        let frame = bgra_buffer(width, height).unwrap();
        unsafe {
            CVPixelBufferLockBaseAddress(&frame, CVPixelBufferLockFlags::empty());
            let base = CVPixelBufferGetBaseAddress(&frame).cast::<u8>();
            let row = CVPixelBufferGetBytesPerRow(&frame);
            std::ptr::write_bytes(base, 0, row * height);
            // A red top-left pixel.
            std::ptr::copy_nonoverlapping([0u8, 0, 255, 255].as_ptr(), base, 4);
            CVPixelBufferUnlockBaseAddress(&frame, CVPixelBufferLockFlags::empty());
        }
        let red = [0, 0, 255, 255];
        for (rotation, size, corner) in [
            (Rotation::Clockwise, (4, 8), (3, 0)),
            (Rotation::CounterClockwise, (4, 8), (0, 7)),
            (Rotation::Half, (8, 4), (7, 3)),
            (Rotation::None, (8, 4), (0, 0)),
        ] {
            let turned = turned_frame(frame.clone(), rotation).expect("turned");
            assert_eq!(
                (
                    CVPixelBufferGetWidth(&turned),
                    CVPixelBufferGetHeight(&turned)
                ),
                size,
                "{rotation:?}"
            );
            assert_eq!(pixel_at(&turned, corner.0, corner.1), red, "{rotation:?}");
        }
    }

    #[test]
    fn turns_are_exact_quarter_turns_either_way_up() {
        let t = turn(Rotation::Clockwise, true);
        assert_eq!((t.a, t.b, t.c, t.d), (0., 1., -1., 0.));
        // In y-up space the same on-screen turn is the opposite angle.
        let t = turn(Rotation::Clockwise, false);
        assert_eq!((t.a, t.b, t.c, t.d), (0., -1., 1., 0.));
        let t = turn(Rotation::Half, false);
        assert_eq!((t.a, t.b, t.c, t.d), (-1., 0., 0., -1.));
    }
}

/// What the video layer does with frames when nothing composites it, with
/// real H.264 from VideoToolbox's encoder: no hub or device needed.
#[cfg(test)]
mod queue_tests {
    use super::*;
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };

    #[link(name = "VideoToolbox", kind = "framework")]
    unsafe extern "C" {
        fn VTCompressionSessionCreate(
            allocator: *const c_void,
            width: i32,
            height: i32,
            codec: u32,
            specification: *const c_void,
            attributes: *const c_void,
            data_allocator: *const c_void,
            callback: extern "C" fn(*mut c_void, *mut c_void, i32, u32, *mut CMSampleBuffer),
            refcon: *mut c_void,
            session: *mut *mut c_void,
        ) -> i32;
        fn VTCompressionSessionEncodeFrame(
            session: *mut c_void,
            image: *const CVPixelBuffer,
            pts: CMTime,
            duration: CMTime,
            properties: *const c_void,
            refcon: *mut c_void,
            flags: *mut u32,
        ) -> i32;
        fn VTCompressionSessionCompleteFrames(session: *mut c_void, until: CMTime) -> i32;
        fn VTCompressionSessionInvalidate(session: *mut c_void);
    }
    #[link(name = "CoreMedia", kind = "framework")]
    unsafe extern "C" {
        fn CMSampleBufferGetFormatDescription(sample: *mut CMSampleBuffer) -> *const c_void;
        fn CMSampleBufferGetDataBuffer(sample: *mut CMSampleBuffer) -> *const c_void;
        fn CMBlockBufferGetDataLength(block: *const c_void) -> usize;
        fn CMBlockBufferCopyDataBytes(
            block: *const c_void,
            offset: usize,
            length: usize,
            destination: *mut c_void,
        ) -> i32;
        fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format: *const c_void,
            index: usize,
            pointer: *mut *const u8,
            size: *mut usize,
            count: *mut usize,
            header: *mut i32,
        ) -> i32;
    }
    struct Encoded {
        sets: Option<ParameterSets>,
        units: Vec<Vec<u8>>,
    }

    extern "C" fn encoded(
        refcon: *mut c_void,
        _: *mut c_void,
        status: i32,
        _: u32,
        sample: *mut CMSampleBuffer,
    ) {
        if status != 0 || sample.is_null() {
            return;
        }
        let out = unsafe { &*(refcon as *const Mutex<Encoded>) };
        let mut out = out.lock().unwrap();
        unsafe {
            if out.sets.is_none() {
                let format = CMSampleBufferGetFormatDescription(sample);
                let mut sets = ParameterSets {
                    sps: Vec::new(),
                    pps: Vec::new(),
                };
                let mut count = 0;
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut count,
                    std::ptr::null_mut(),
                );
                for index in 0..count {
                    let (mut pointer, mut size) = (std::ptr::null(), 0);
                    CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        format,
                        index,
                        &mut pointer,
                        &mut size,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                    let unit = std::slice::from_raw_parts(pointer, size).to_vec();
                    if unit.first().is_some_and(|b| b & 0x1f == 7) {
                        sets.sps.push(unit);
                    } else {
                        sets.pps.push(unit);
                    }
                }
                out.sets = Some(sets);
            }
            let block = CMSampleBufferGetDataBuffer(sample);
            let length = CMBlockBufferGetDataLength(block);
            let mut unit = vec![0u8; length];
            CMBlockBufferCopyDataBytes(block, 0, length, unit.as_mut_ptr().cast());
            out.units.push(unit);
        }
    }

    /// `count` frames of a picture that changes every frame, as AVCC access
    /// units (the first a keyframe), with their parameter sets.
    pub(super) fn h264(count: usize) -> (ParameterSets, Vec<Vec<u8>>) {
        let (width, height) = (320, 640);
        let out = Box::new(Mutex::new(Encoded {
            sets: None,
            units: Vec::new(),
        }));
        unsafe {
            let mut session = std::ptr::null_mut();
            let status = VTCompressionSessionCreate(
                std::ptr::null(),
                width,
                height,
                u32::from_be_bytes(*b"avc1"),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                encoded,
                (&*out as *const Mutex<Encoded>).cast_mut().cast(),
                &mut session,
            );
            assert_eq!(status, 0, "an H.264 encoder");
            for frame in 0..count {
                let buffer = bgra_buffer(width as usize, height as usize).unwrap();
                CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
                let row = CVPixelBufferGetBytesPerRow(&buffer);
                for y in 0..height as usize {
                    // A band that moves down a row each frame.
                    let value = if (y + frame) % 64 < 8 { 230 } else { 30 };
                    std::ptr::write_bytes(base.add(row * y), value, row);
                }
                CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                let pts = CMTime {
                    value: frame as i64,
                    timescale: 60,
                    flags: objc2_core_media::CMTimeFlags::Valid,
                    epoch: 0,
                };
                let status = VTCompressionSessionEncodeFrame(
                    session,
                    CFRetained::as_ptr(&buffer).as_ptr(),
                    pts,
                    objc2_core_media::kCMTimeInvalid,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                assert_eq!(status, 0);
            }
            VTCompressionSessionCompleteFrames(session, objc2_core_media::kCMTimeInvalid);
            VTCompressionSessionInvalidate(session);
            CFRelease(session);
        }
        let out = out.into_inner().unwrap();
        (out.sets.expect("parameter sets"), out.units)
    }

    fn ready(renderer: &Renderer) -> bool {
        unsafe { msg_send![&*renderer.target, isReadyForMoreMediaData] }
    }

    fn held(renderer: &Renderer) -> usize {
        renderer.held.load(Ordering::Relaxed)
    }

    /// Waits up to `limit` for the layer to let go of everything.
    fn drained(renderer: &Renderer, limit: std::time::Duration) -> Option<std::time::Duration> {
        let started = std::time::Instant::now();
        while held(renderer) > 0 {
            if started.elapsed() > limit {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Some(started.elapsed())
    }

    /// The layer holds each sample only until it decodes; a burst it can't
    /// keep up with is refused past `MAX_HELD`, and a flush lets go of what
    /// it holds.
    #[test]
    fn the_layer_holds_a_bounded_backlog_and_flush_lets_it_go() {
        let (sets, units) = h264(240);
        // SAFETY: the layer is made here and never put in a window.
        let renderer = Renderer::new(unsafe { MainThreadMarker::new_unchecked() });
        renderer.configure(&sets).unwrap();
        let (mut queued, mut full, mut most) = (0, 0, 0);
        for unit in &units {
            match renderer.frame(unit) {
                Queued::Yes => queued += 1,
                Queued::Full => full += 1,
                Queued::Failed => panic!("the stream decodes"),
            }
            most = most.max(held(&renderer));
        }
        let drain = drained(&renderer, std::time::Duration::from_secs(5));
        eprintln!(
            "{} frames in a burst: {queued} queued, {full} refused, at most {most} held, drained in {drain:?}, ready {}",
            units.len(),
            ready(&renderer)
        );
        assert!(most <= MAX_HELD, "never more than the cap: {most}");
        assert!(queued >= MAX_HELD.min(units.len()));
        assert!(drain.is_some(), "an unseen layer still decodes and lets go");
        // A keyframe and deltas, then a flush before they decode.
        for unit in &units[..MAX_HELD] {
            renderer.frame(unit);
        }
        renderer.flush();
        let drain = drained(&renderer, std::time::Duration::from_secs(2));
        eprintln!("after flush: drained in {drain:?}");
        assert!(drain.is_some(), "flush lets go of what is queued");
    }

    /// Measures how long the layer takes to work through a backlog, as the
    /// 1.2 million samples found on 2026-11-11 were: if each sample's upkeep
    /// grows with the queue, a layer that falls behind never catches up.
    /// `cargo test --profile native-dev --bins backlog_drain -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement; encodes several thousand frames"]
    fn backlog_drain_times() {
        let most = std::env::var("BACKLOG_MAX")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(4000);
        let (sets, units) = h264(most);
        for timed in [false, true] {
            let mut size = 250;
            while size <= units.len() {
                // SAFETY: the layer is made here and never put in a window.
                let renderer = Renderer::new(unsafe { MainThreadMarker::new_unchecked() });
                renderer.configure(&sets).unwrap();
                let format = renderer.format.lock().unwrap().clone().unwrap();
                let started = std::time::Instant::now();
                for (index, unit) in units[..size].iter().enumerate() {
                    let timing = timed.then(|| objc2_core_media::CMSampleTimingInfo {
                        duration: CMTime {
                            value: 1,
                            timescale: 60,
                            flags: objc2_core_media::CMTimeFlags::Valid,
                            epoch: 0,
                        },
                        presentationTimeStamp: CMTime {
                            value: index as i64,
                            timescale: 60,
                            flags: objc2_core_media::CMTimeFlags::Valid,
                            epoch: 0,
                        },
                        decodeTimeStamp: unsafe { objc2_core_media::kCMTimeInvalid },
                    });
                    let sample = unsafe { sample(&format, unit, &renderer.held, timing) }.unwrap();
                    let _: () =
                        unsafe { msg_send![&*renderer.target, enqueueSampleBuffer: &*sample] };
                }
                let enqueued = started.elapsed();
                let ready_after = ready(&renderer);
                let drain = drained(&renderer, std::time::Duration::from_secs(120));
                let status: isize = unsafe { msg_send![&*renderer.target, status] };
                eprintln!(
                    "timed {timed:5} backlog {size:5}: enqueue {:7.1} ms, drain {:?} ({:.3} ms a frame), status {status}, ready with the backlog queued {ready_after}, left {}",
                    enqueued.as_secs_f64() * 1000.,
                    drain,
                    drain.map_or(f64::NAN, |d| d.as_secs_f64() * 1000. / size as f64),
                    held(&renderer)
                );
                renderer.flush();
                drained(&renderer, std::time::Duration::from_secs(10));
                size *= 2;
            }
        }
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
        fn frame(&self, avcc: &[u8]) -> Queued {
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
            let queued = self.renderer.frame(avcc);
            let counter = if queued == Queued::Yes {
                &self.frames
            } else {
                &self.refused
            };
            counter.fetch_add(1, Ordering::Relaxed);
            queued
        }
        fn flush(&self) {
            self.renderer.flush()
        }
        /// Never on screen, but every frame is checked.
        fn showing(&self) -> bool {
            true
        }
    }

    /// Streams the booted iOS Simulator for a few seconds while swiping its
    /// home screen: `cargo test live_stream -- --ignored --nocapture`. Set
    /// `MONOCODE_DEVICE_CAPTURE` to keep the stream as an `.h264` file.
    #[test]
    #[ignore = "runs the real Device Hub against a booted simulator"]
    fn live_stream_reaches_the_video_layer() {
        device::set_enabled(true, false);
        let stage = Instant::now();
        let endpoint = hub::ensure().expect("the hub starts");
        eprintln!("timing: hub start {:?}", stage.elapsed());
        let stage = Instant::now();
        let list = hub::list().expect("the hub lists devices");
        eprintln!("timing: device list {:?}", stage.elapsed());
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
            eprintln!(
                "booted {} as {} in {:?}",
                target.name,
                target.id,
                booting.elapsed()
            );
        }
        if platform == Platform::Ios {
            let stage = Instant::now();
            hub::attach(&endpoint, &target.id).expect("the simulator attaches");
            eprintln!("timing: attach {:?}", stage.elapsed());
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
        live.send(stream::Input::Button(
            monocode_core::device::protocol::Button::Home,
        ));
        std::thread::sleep(Duration::from_millis(1500));
        let before = sink.frames.load(Ordering::Relaxed);
        let window = Instant::now();
        let mut latencies = Vec::new();
        for round in 0..4 {
            let (from, to) = if round % 2 == 0 {
                (0.85, 0.15)
            } else {
                (0.15, 0.85)
            };
            live.send(stream::Input::Touch(Phase::Begin, from, 0.5));
            for step in 1..=12 {
                let x = from + (to - from) * step as f64 / 12.;
                std::thread::sleep(Duration::from_millis(16));
                let sent = Instant::now();
                live.send(stream::Input::Touch(Phase::Move, x, 0.5));
                if step == 1 {
                    // The screen was still; the first frame after the first
                    // move is the touch reaching the picture.
                    let deadline = sent + Duration::from_millis(1000);
                    loop {
                        let after = sink
                            .times
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|t| **t > sent)
                            .copied();
                        if let Some(at) = after {
                            latencies.push(at.duration_since(sent).as_secs_f64() * 1000.);
                            break;
                        }
                        if Instant::now() > deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            live.send(stream::Input::Touch(Phase::End, to, 0.5));
            std::thread::sleep(Duration::from_millis(600));
        }
        let elapsed = window.elapsed().as_secs_f64();
        // The busiest quarter second: the rate while the page animates.
        let times = sink.times.lock().unwrap().clone();
        let peak = times
            .iter()
            .map(|start| {
                times
                    .iter()
                    .filter(|t| {
                        **t >= *start && t.duration_since(*start) < Duration::from_millis(250)
                    })
                    .count()
            })
            .max()
            .unwrap_or(0);
        let gaps: Vec<f64> = times
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.)
            .filter(|gap| *gap < 100.)
            .collect();
        let mean_gap = gaps.iter().sum::<f64>() / gaps.len().max(1) as f64;
        eprintln!(
            "peak {} fps over 250 ms; mean gap between consecutive frames while moving {mean_gap:.1} ms",
            peak * 4
        );
        // A finger held down and moved every 4 ms for 3 s: the screen changes
        // on every frame the device draws, so this is the pipeline's ceiling.
        if std::env::var_os("MONOCODE_DEVICE_DRAG").is_some() {
            let begin = Instant::now();
            live.send(stream::Input::Touch(Phase::Begin, 0.5, 0.5));
            let mut x: f64 = 0.5;
            let mut dir = -1.0;
            while begin.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(4));
                x += dir * 0.006;
                if !(0.2..=0.8).contains(&x) {
                    dir = -dir;
                }
                live.send(stream::Input::Touch(Phase::Move, x, 0.5));
            }
            live.send(stream::Input::Touch(Phase::End, x, 0.5));
            let drag: Vec<Instant> = sink
                .times
                .lock()
                .unwrap()
                .iter()
                .copied()
                .filter(|t| {
                    *t > begin + Duration::from_millis(300) && *t < begin + Duration::from_secs(3)
                })
                .collect();
            let gaps: Vec<f64> = drag
                .windows(2)
                .map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.)
                .collect();
            let mut sorted = gaps.clone();
            sorted.sort_by(f64::total_cmp);
            let stalls: Vec<String> = drag
                .windows(2)
                .filter(|w| w[1].duration_since(w[0]) > Duration::from_millis(50))
                .map(|w| {
                    format!(
                        "{:.0}ms@{:.2}s",
                        w[1].duration_since(w[0]).as_secs_f64() * 1000.,
                        w[0].duration_since(begin).as_secs_f64()
                    )
                })
                .collect();
            eprintln!("drag stalls: {stalls:?}");
            eprintln!(
                "drag: {} frames in 2.7 s = {:.0} fps; gap median {:.1} p90 {:.1} max {:.1} ms",
                drag.len(),
                drag.len() as f64 / 2.7,
                sorted.get(sorted.len() / 2).copied().unwrap_or(0.),
                sorted.get(sorted.len() * 9 / 10).copied().unwrap_or(0.),
                sorted.last().copied().unwrap_or(0.)
            );
        }
        latencies.sort_by(f64::total_cmp);
        eprintln!("timing: touch to new frame (ms) {latencies:.0?}");
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
        assert_eq!(
            sink.refused.load(Ordering::Relaxed),
            0,
            "the layer takes every frame"
        );
        assert!(frames > 30, "swiping streams frames");
    }
}
