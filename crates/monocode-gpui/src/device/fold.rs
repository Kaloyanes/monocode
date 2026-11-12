//! A foldable's controls under its picture: the flat stream or the 3D
//! model, the poses, and the hinge's angle, which the hub's own viewer
//! shows only while Option is held.
use super::{DevicePanel, stream::Input};
use crate::{popover::AnchorSlot, theme::*, tooltip::Tip};
use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, Pixels, Role, SharedString, div, prelude::*,
    px, relative,
};
use monocode_core::device::protocol::Pose;
use std::time::{Duration, Instant};

/// How often a dragged slider tells the hub, which takes a moment to move
/// the simulator's hinge.
const SEND_EVERY: Duration = Duration::from_millis(90);
/// How long the panel shows its own angle before the hub's comes back.
const TRUST_FOR: Duration = Duration::from_secs(4);

#[derive(Default)]
pub(super) struct Fold {
    /// The 3D model in the picture's place.
    pub(super) three_d: bool,
    /// What was last asked for, until the hub reports it.
    asked: Option<Asked>,
    dragging: bool,
    sent: Option<(Instant, f64)>,
    track: AnchorSlot,
}

#[derive(Clone, Copy)]
struct Asked {
    angle: f64,
    pose: Option<Pose>,
    at: Instant,
}

pub(super) fn pose_icon(pose: Pose) -> &'static str {
    match pose {
        Pose::Open => "fold-open",
        Pose::Book => "fold-book",
        Pose::Laptop => "fold-laptop",
        Pose::Tent => "fold-tent",
        Pose::Closed => "fold-closed",
    }
}

impl Fold {
    /// The hub reported the hinge: what was asked for is done once it
    /// matches, or forgotten once it is stale.
    pub(super) fn reported(&mut self, angle: Option<f64>) {
        if self.dragging {
            return;
        }
        if let Some(asked) = self.asked
            && (angle.is_some_and(|a| (a - asked.angle).abs() < 1.5)
                || asked.at.elapsed() > TRUST_FOR)
        {
            self.asked = None;
        }
    }
    pub(super) fn failed(&mut self) {
        self.asked = None;
        self.dragging = false;
    }
    pub(super) fn dragging(&self) -> bool {
        self.dragging
    }
}

impl DevicePanel {
    /// The hinge's angle and pose as shown: what was just asked for, or what
    /// the hub last said.
    pub(super) fn hinge(&self) -> (f64, Option<Pose>) {
        if let Some(asked) = self.fold.asked {
            return (asked.angle, asked.pose);
        }
        let screen = self.live.as_ref().and_then(|live| live.screen);
        let pose = screen.and_then(|s| s.hinge_pose);
        let angle = screen
            .and_then(|s| s.hinge_angle)
            .or(pose.map(Pose::angle))
            .unwrap_or(180.);
        (angle, pose)
    }

    pub(super) fn fold_to(&mut self, pose: Pose, cx: &mut Context<Self>) {
        if let Some(live) = self.live.as_mut() {
            live.fold_error = None;
        }
        self.fold.asked = Some(Asked {
            angle: pose.angle(),
            pose: Some(pose),
            at: Instant::now(),
        });
        self.send(Input::Fold(pose));
        cx.notify();
    }

    fn slide(&mut self, x: Pixels, done: bool, cx: &mut Context<Self>) {
        let Some(bounds) = self.fold.track.get() else {
            return;
        };
        let travel = (bounds.size.width.as_f32() - THUMB).max(1.);
        let fraction = ((x - bounds.left()).as_f32() - THUMB / 2.).clamp(0., travel) / travel;
        let angle = (fraction as f64 * 180.).round();
        self.fold.asked = Some(Asked {
            angle,
            pose: None,
            at: Instant::now(),
        });
        let due = self
            .fold
            .sent
            .is_none_or(|(at, sent)| sent != angle && (done || at.elapsed() >= SEND_EVERY));
        if due {
            if let Some(live) = self.live.as_mut() {
                live.fold_error = None;
            }
            self.send(Input::FoldAngle(angle));
            self.fold.sent = Some((Instant::now(), angle));
        }
        cx.notify();
    }

    /// The panel's mouse moves, for a slider being dragged.
    pub(super) fn fold_drag(&mut self, x: Pixels, pressed: bool, cx: &mut Context<Self>) {
        if !self.fold.dragging {
            return;
        }
        if pressed {
            self.slide(x, false, cx);
        } else {
            self.fold_release(x, cx);
        }
    }

    pub(super) fn fold_release(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if !self.fold.dragging {
            return;
        }
        self.slide(x, true, cx);
        self.fold.dragging = false;
        self.fold.sent = None;
        cx.notify();
    }

    /// 2D or 3D, the poses, and the hinge's slider.
    pub(super) fn fold_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let (angle, pose) = self.hinge();
        let three_d = self.fold.three_d;
        let poses = div()
            .id("device-poses")
            .role(Role::RadioGroup)
            .aria_label("Pose")
            .flex()
            .flex_shrink_0()
            .gap(px(2.))
            .p(px(2.))
            .rounded_full()
            .bg(content(0.06))
            .children(Pose::ALL.into_iter().map(|p| {
                let selected = pose == Some(p);
                div()
                    .id(SharedString::from(format!("device-pose-{}", p.name())))
                    .role(Role::RadioButton)
                    .aria_label(p.label())
                    .aria_selected(selected)
                    .tip(p.label())
                    .w(px(30.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .map(|d| {
                        if selected {
                            d.bg(content(0.14))
                        } else {
                            d.hover(|s| s.bg(content(0.07)))
                        }
                    })
                    .child(icon(pose_icon(p), 16.).text_color(content(if selected {
                        1.
                    } else {
                        0.6
                    })))
                    .on_click(cx.listener(move |this, _, _, cx| this.fold_to(p, cx)))
            }));
        let fraction = (angle / 180.).clamp(0., 1.) as f32;
        let slider = div()
            .id("device-hinge")
            .role(Role::Slider)
            .aria_label("Hinge angle")
            .min_w(px(80.))
            .flex_1()
            .h(px(THUMB))
            .relative()
            .flex()
            .items_center()
            .cursor_pointer()
            .child(self.fold.track.layout_probe())
            .child(
                div()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(content(0.15))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .w(relative(fraction))
                            .bg(accent_color()),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(relative(fraction))
                    .ml(px(-THUMB / 2. + THUMB * (0.5 - fraction)))
                    .size(px(THUMB))
                    .rounded_full()
                    .bg(gpui::white())
                    .shadow_sm(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.fold.dragging = true;
                    this.fold.sent = None;
                    this.slide(event.position.x, false, cx);
                    cx.stop_propagation();
                }),
            );
        let view = super::tools::segmented(
            "device-view",
            &[("2d", "2D"), ("3d", "3D")],
            if three_d { "3d" } else { "2d" },
            cx,
            |this, value, cx| {
                this.fold.three_d = value == "3d";
                cx.notify();
            },
        );
        div()
            .flex_shrink_0()
            .px_4()
            .pb_3()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(view)
                    .child(poses),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        icon("hinge-angle", 16.)
                            .flex_shrink_0()
                            .text_color(content(0.55)),
                    )
                    .child(slider)
                    .child(
                        div()
                            .w(px(34.))
                            .flex_shrink_0()
                            .text_right()
                            .text_size(px(12.))
                            .text_color(content(0.85))
                            .child(format!("{}°", angle.round() as i64)),
                    ),
            )
            .into_any_element()
    }
}

const THUMB: f32 = 14.;
