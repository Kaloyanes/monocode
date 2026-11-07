//! Stationary notification glass, matching GlassBackdrop and the native popover.
use crate::theme::*;
use gpui::{
    Animation, AnimationElement, AnimationExt, AnyView, Context, Div, ElementId, Pixels, Render,
    Size, Task, canvas, div, ease_out_quint, prelude::*, px,
};
use std::{cell::Cell, rc::Rc, time::Duration};

pub(crate) const TOAST_ENTER: Duration = Duration::from_millis(240);
pub(crate) const TOAST_EXIT: Duration = Duration::from_millis(160);
/// Toasts sit against the right edge, so they travel along it.
const TOAST_TRAVEL: f32 = 24.;
pub(crate) const TOAST_GAP: f32 = 8.;

pub(crate) fn backdrop(radius: f32) -> impl IntoElement {
    let (light, opaque) = (is_light(), floating_fill());
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let quad = if light {
                gpui::fill(bounds, opaque)
            } else {
                gpui::fill(bounds, content(0.02))
                    .border_color(gpui::backdrop_blur_marker(24. * window.scale_factor()))
            };
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let _ = (light, opaque);
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let quad = gpui::fill(bounds, pane_fill());
            window.paint_quad(quad.corner_radii(px(radius)));
        },
    )
    .absolute()
    .size_full()
}
pub(crate) fn glass_frame() -> Div {
    div()
        .relative()
        .w(px(340.))
        .max_w_full()
        .overflow_hidden()
        .rounded(px(12.))
        .border_1()
        .border_color(content(0.1))
        .text_color(content(1.))
        .shadow_lg()
        .child(backdrop(12.))
}

/// Shared shell for the compact cards in the top-right notification stack.
pub(crate) fn toast_frame() -> Div {
    glass_frame()
        .w_full()
        .occlude()
        .border_dashed()
        .border_color(content(0.2))
        .bg(content(0.1))
}

/// Slides a toast in from the window edge. Reduced motion shows it in place.
pub(crate) fn toast_enter<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    toast: E,
) -> AnimationElement<E> {
    toast.with_animation(
        id,
        Animation::new(TOAST_ENTER).with_easing(ease_out_quint()),
        |el, t| el.relative().opacity(t).left(px(TOAST_TRAVEL * (1. - t))),
    )
}

/// Slides a toast back out while its slot closes, so the cards below follow
/// it up.
pub(crate) fn toast_exit(
    id: impl Into<ElementId>,
    size: Size<Pixels>,
    toast: impl IntoElement,
) -> AnimationElement<Div> {
    slide_out(id, size, TOAST_GAP, toast)
}

/// `toast_exit` for a row inside a card, which has no gap to close.
pub(crate) fn row_exit(
    id: impl Into<ElementId>,
    size: Size<Pixels>,
    row: impl IntoElement,
) -> AnimationElement<Div> {
    slide_out(id, size, 0., row)
}

/// The cover keeps what is already gone from taking clicks.
fn slide_out(
    id: impl Into<ElementId>,
    size: Size<Pixels>,
    gap: f32,
    content: impl IntoElement,
) -> AnimationElement<Div> {
    div()
        .relative()
        .w(size.width)
        .overflow_hidden()
        .child(content)
        .child(div().absolute().inset_0().occlude())
        .with_animation(
            id,
            Animation::new(TOAST_EXIT).with_easing(ease_out_quint()),
            move |el, t| {
                el.h(size.height * (1. - t))
                    .mb(px(-gap * t))
                    .opacity(1. - t)
                    .left(px(TOAST_TRAVEL * t))
            },
        )
}

/// Reports a toast's painted size. Goes outside the card's border, because
/// the exit has to take up the card's whole slot.
pub(crate) fn size_probe(report: impl Fn(Size<Pixels>) + 'static) -> impl IntoElement {
    canvas(move |bounds, _, _| report(bounds.size), |_, _, _, _| {})
        .absolute()
        .size_full()
}

struct LeavingToast {
    view: AnyView,
    size: Size<Pixels>,
    _timer: Task<()>,
}

/// A toast's fixed place in the stack. Its state may be cleared from anywhere,
/// so this keeps a still copy to play the exit with once the toast is gone.
pub(crate) struct ToastSlot<V> {
    copy: Option<V>,
    size: Rc<Cell<Option<Size<Pixels>>>>,
    leaving: Option<LeavingToast>,
}
impl<V> Default for ToastSlot<V> {
    fn default() -> Self {
        Self {
            copy: None,
            size: Rc::default(),
            leaving: None,
        }
    }
}
impl<V: Render> ToastSlot<V> {
    /// `current` is a copy of the toast, or `None` once it has gone. A toast
    /// that comes back cancels its exit, and one that never painted has
    /// nothing to animate away from.
    pub(crate) fn sync<H: 'static>(
        &mut self,
        current: Option<V>,
        slot: fn(&mut H) -> &mut Self,
        cx: &mut Context<H>,
    ) {
        let last = std::mem::replace(&mut self.copy, current);
        if self.copy.is_some() {
            self.stay();
        } else if let Some(last) = last {
            self.leave(last, slot, cx);
        }
    }
    pub(crate) fn stay(&mut self) {
        self.leaving = None;
    }
    /// Plays the exit with `copy`, for hosts that know when a toast closes.
    pub(crate) fn leave<H: 'static>(
        &mut self,
        copy: V,
        slot: fn(&mut H) -> &mut Self,
        cx: &mut Context<H>,
    ) {
        let Some(size) = self.size.take().filter(|_| !cx.reduce_motion()) else {
            return;
        };
        let timer = cx.spawn(async move |host, cx| {
            cx.background_executor().timer(TOAST_EXIT).await;
            let _ = host.update(cx, |host, cx| {
                slot(host).leaving = None;
                cx.notify();
            });
        });
        self.leaving = Some(LeavingToast {
            view: cx.new(|_| copy).into(),
            size,
            _timer: timer,
        });
    }
    pub(crate) fn enter(&self, id: impl Into<ElementId>, toast: Div) -> AnimationElement<Div> {
        toast_enter(id, self.settled(toast))
    }
    /// The toast at rest, for one that has already slid in.
    pub(crate) fn settled(&self, toast: Div) -> Div {
        let size = self.size.clone();
        toast
            .relative()
            .child(size_probe(move |painted| size.set(Some(painted))))
    }
    pub(crate) fn exit(&self, id: impl Into<ElementId>) -> Option<AnimationElement<Div>> {
        self.leaving
            .as_ref()
            .map(|toast| toast_exit(id, toast.size, toast.view.clone()))
    }
}
