//! The title bar's plan rail: the active session's plan as its agent last
//! wrote it with a todo tool, in place of the session tabs. It is its own
//! cached view, so a streaming transcript repaints only the rail.
use crate::conversation::Conversation;
use crate::popover::{self, Anchor, AnchorSlot};
use crate::pulse::{self, Rate};
use crate::theme::*;
use gpui::{
    AnyElement, Context, Entity, Hsla, Role, SharedString, Subscription, WeakEntity, Window, div,
    prelude::*, px, rgb,
};
use monocode_core::tool::{
    plan::{self, Plan, Snapshot},
    tasks::{Task, TaskList, TaskStatus},
};
use std::time::Duration;

/// Tailwind teal-400 and amber-400, as the title tabs' done and attention marks.
const DONE: u32 = 0x2dd4bf;
const ACTIVE: u32 = 0xfbbf24;
const RAIL_HEIGHT: f32 = 10.;
/// Wide enough to read each step and its ticks, narrow enough to glance at;
/// it gives way to the readout and drag area in a small window.
const RAIL_WIDTH: f32 = 440.;
const RAIL_MIN_WIDTH: f32 = 160.;
/// Ask for a plan only once a turn has done real work without one.
const ASK_AFTER_CALLS: usize = 6;
const TICK_PITCH: f32 = 5.;
const HAIR_PITCH: f32 = 7.;

pub(crate) struct PlanRail {
    session: Option<i64>,
    conversation: Option<WeakEntity<Conversation>>,
    snapshot: Snapshot,
    busy: bool,
    attention: bool,
    open: bool,
    anchor: AnchorSlot,
    /// The steer the user last sent, shown until the agent's list changes.
    asked: Option<(SharedString, Option<TaskList>)>,
    _observers: Vec<Subscription>,
}

impl PlanRail {
    pub(crate) fn new() -> Self {
        Self {
            session: None,
            conversation: None,
            snapshot: Snapshot::default(),
            busy: false,
            attention: false,
            open: false,
            anchor: AnchorSlot::default(),
            asked: None,
            _observers: Vec::new(),
        }
    }

    /// Follows the shell's active session; the same session is a no-op.
    pub(crate) fn follow(
        &mut self,
        session: Option<(i64, Entity<Conversation>)>,
        cx: &mut Context<Self>,
    ) {
        if self.session == session.as_ref().map(|(id, _)| *id) {
            return;
        }
        self.open = false;
        self.asked = None;
        self._observers.clear();
        self.session = session.as_ref().map(|(id, _)| *id);
        self.conversation = session.as_ref().map(|(_, c)| c.downgrade());
        if let Some((_, conversation)) = session {
            let transcript = conversation.read(cx).transcript.clone();
            self._observers = vec![
                cx.observe(&conversation, |this, _, cx| this.refresh(cx)),
                cx.observe(&transcript, |this, _, cx| this.refresh(cx)),
            ];
        }
        self.refresh(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(conversation) = self.conversation.as_ref().and_then(|c| c.upgrade()) else {
            return;
        };
        let (snapshot, busy, attention) = {
            let c = conversation.read(cx);
            let rows = c.transcript.read(cx).source_rows();
            (plan::snapshot(rows), c.responding(), c.needs_approval())
        };
        let list = snapshot.plan.as_ref().map(|p| &p.list);
        if self
            .asked
            .as_ref()
            .is_some_and(|(_, at)| at.as_ref() != list)
            || !busy
        {
            self.asked = None;
        }
        if (snapshot.clone(), busy, attention) != (self.snapshot.clone(), self.busy, self.attention)
        {
            self.snapshot = snapshot;
            self.busy = busy;
            self.attention = attention;
            cx.notify();
        }
    }

    /// The agent changes its own list; the rail only asks, between tool calls.
    fn steer(&mut self, label: String, prompt: String, cx: &mut Context<Self>) {
        let Some(conversation) = self.conversation.as_ref().and_then(|c| c.upgrade()) else {
            return;
        };
        conversation.update(cx, |c, cx| c.send_plan_steer(prompt, cx));
        let list = self.snapshot.plan.as_ref().map(|p| p.list.clone());
        self.asked = Some((label.into(), list));
        self.open = false;
        cx.notify();
    }

    fn segment(&self, ix: usize, task: &Task, plan: &Plan) -> AnyElement {
        let base = div()
            .id(("plan-step", ix))
            .relative()
            .h(px(RAIL_HEIGHT))
            .min_w(px(4.))
            .rounded(px(3.))
            .overflow_hidden();
        match task.status {
            TaskStatus::Completed => base.flex_1().bg(rgb(DONE)).into_any_element(),
            TaskStatus::Pending => base.flex_1().bg(content(0.1)).into_any_element(),
            // A short stub keeps the rail honest about what was dropped.
            TaskStatus::Cancelled => base
                .w(px(8.))
                .flex_none()
                .border_1()
                .border_dashed()
                .border_color(content(0.18))
                .into_any_element(),
            TaskStatus::InProgress => {
                // One tick per tool call since the agent last touched its
                // list: real activity, not a guessed percentage.
                let ticks = (0..plan.activity.min(64)).map(|i| {
                    div()
                        .absolute()
                        .top(px(2.))
                        .bottom(px(2.))
                        .left(px(3. + i as f32 * TICK_PITCH))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(rgb(ACTIVE))
                });
                base.flex_1()
                    .bg(amber(0.18))
                    .border_1()
                    .border_color(if self.attention {
                        rgb(ACTIVE).into()
                    } else {
                        amber(0.55)
                    })
                    .children(ticks)
                    .into_any_element()
            }
        }
    }

    fn track(&self) -> AnyElement {
        let snapshot = &self.snapshot;
        let Some(plan) = &snapshot.plan else {
            // No plan: show the turn's tool calls as hairlines, never invented steps.
            let hairs = (0..snapshot.turn_calls.min(96)).map(|i| {
                div()
                    .absolute()
                    .top(px(1.))
                    .bottom(px(1.))
                    .left(px(4. + i as f32 * HAIR_PITCH))
                    .w(px(1.))
                    .bg(content(0.35))
            });
            return div()
                .id("plan-rail-empty")
                .relative()
                .flex_1()
                .h(px(RAIL_HEIGHT))
                .rounded(px(3.))
                .overflow_hidden()
                .border_1()
                .border_dashed()
                .border_color(content(0.1))
                .when(self.busy, |d| d.children(hairs))
                .into_any_element();
        };
        let pulse = self.attention.then(|| {
            pulse::looping(
                "plan-rail-attention",
                Duration::from_millis(1_100),
                Rate::Half,
                |phase| {
                    let wave = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
                    div()
                        .absolute()
                        .inset(px(-3.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(amber(0.15 + 0.6 * wave))
                },
            )
        });
        div()
            .id("plan-rail-track")
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(3.))
            .when(plan.stale, |d| d.opacity(0.35))
            .children(
                plan.list
                    .items
                    .iter()
                    .enumerate()
                    .map(|(ix, task)| self.segment(ix, task, plan)),
            )
            .children(pulse)
            .into_any_element()
    }

    fn readout(&self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = &self.snapshot;
        let text = |s: SharedString, color: Hsla| {
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(color)
                .child(s)
        };
        let count = |s: String| {
            div()
                .flex_shrink_0()
                .font_family(crate::markdown::MONO)
                .text_color(content(0.45))
                .child(s)
        };
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .text_size(px(12.));
        if let Some((asked, _)) = &self.asked {
            return row
                .child(text("Asked:".into(), content(0.45)))
                .child(text(asked.clone(), content(0.85)))
                .into_any_element();
        }
        if self.attention {
            return row
                .child(icon("alert-circle", 13.).text_color(rgb(ACTIVE)))
                .child(text("Needs your approval".into(), rgb(ACTIVE).into()))
                .into_any_element();
        }
        let Some(plan) = &snapshot.plan else {
            let label = if self.busy {
                format!("No plan · {} tool calls", snapshot.turn_calls)
            } else {
                "No plan".into()
            };
            let ask = (self.busy && snapshot.turn_calls >= ASK_AFTER_CALLS).then(|| {
                small_button("plan-rail-ask", "Ask for plan").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.steer(
                            "a plan".into(),
                            "Before you continue, lay out the remaining work as a short \
                             todo list with your todo tool, then keep it updated as you go."
                                .into(),
                            cx,
                        )
                    },
                ))
            });
            return row
                .child(text(label.into(), content(0.45)))
                .children(ask)
                .into_any_element();
        };
        let (done, total) = (plan.done(), plan.actionable());
        if plan.stale {
            return row
                .child(text("Last turn’s plan".into(), content(0.45)))
                .child(count(format!("{done}/{total}")))
                .into_any_element();
        }
        if plan.complete() {
            return row
                .child(icon("check-circle-2", 13.).text_color(rgb(DONE)))
                .child(text("Done".into(), content(0.85)))
                .child(count(format!("{done}/{total}")))
                .into_any_element();
        }
        let current = plan
            .current()
            .map(|t| t.text.clone())
            .unwrap_or_else(|| "Planning".into());
        row.child(text(current.into(), content(0.85)))
            .child(count(format!("{done}/{total}")))
            .into_any_element()
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(plan) = &self.snapshot.plan else {
            return div().into_any_element();
        };
        let steerable = self.busy && !plan.stale && self.conversation.is_some();
        let header = if plan.stale {
            "From the previous turn".to_string()
        } else if plan.list.explanation.trim().is_empty() {
            "Written by the agent with its todo tool".to_string()
        } else {
            plan.list.explanation.trim().to_string()
        };
        let rows = plan.list.items.iter().enumerate().map(|(ix, task)| {
            let mark: AnyElement = match task.status {
                TaskStatus::Completed => icon("check-circle-2", 14.)
                    .text_color(rgb(DONE))
                    .into_any_element(),
                TaskStatus::InProgress => {
                    crate::session_mark::spinner(("plan-row-spinner", ix), Rate::Smooth)
                        .into_any_element()
                }
                TaskStatus::Pending => ring(content(0.4), false),
                TaskStatus::Cancelled => ring(content(0.25), true),
            };
            let done = matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled);
            let actions = (steerable && task.status == TaskStatus::Pending).then(|| {
                let next = task.text.clone();
                let skip = task.text.clone();
                // Over the row's end on hover, so idle rows keep their full width.
                div()
                    .absolute()
                    .right(px(6.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(16.))
                    .bg(floating_fill())
                    .invisible()
                    .group_hover("plan-row", |s| s.visible())
                    .child(
                        small_button(("plan-next", ix), "Do next").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.steer(
                                    format!("do “{next}” next"),
                                    format!(
                                        "Change of order: do “{next}” next, before the other \
                                     remaining steps. Update your todo list to match."
                                    ),
                                    cx,
                                )
                            },
                        )),
                    )
                    .child(
                        small_button(("plan-skip", ix), "Skip").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.steer(
                                    format!("skip “{skip}”"),
                                    format!(
                                        "Skip “{skip}”: don't do that step. Mark it cancelled \
                                     in your todo list and continue with the rest."
                                    ),
                                    cx,
                                )
                            },
                        )),
                    )
            });
            div()
                .id(("plan-row", ix))
                .group("plan-row")
                .relative()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(6.))
                .hover(|s| s.bg(content(0.05)))
                .child(
                    div()
                        .size(px(14.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mark),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(content(if done { 0.45 } else { 0.9 }))
                        .when(task.status == TaskStatus::Cancelled, |d| d.line_through())
                        .child(format!("{}. {}", ix + 1, task.text)),
                )
                .children(actions)
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .mb(px(4.))
                    .border_b_1()
                    .border_color(content(0.08))
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(header),
            )
            .children(rows)
            .into_any_element()
    }
}

impl Render for PlanRail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.session.is_none() {
            return div().into_any_element();
        }
        let has_plan = self.snapshot.plan.is_some();
        let popover = (self.open && has_plan).then(|| {
            let this = cx.entity().downgrade();
            popover::popover("plan-rail-list", Anchor::Slot(self.anchor.clone()))
                .width(400.)
                .padding(6.)
                .gap(6.)
                .on_dismiss(move |_, cx| {
                    let _ = this.update(cx, |rail, cx| {
                        rail.open = false;
                        cx.notify();
                    });
                })
                .child(self.list(cx))
        });
        div()
            .id("plan-rail")
            .debug_selector(|| "plan-rail".into())
            .role(Role::Button)
            .aria_label("Agent plan")
            .h(px(TITLE_BAR_HEIGHT))
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .id("plan-rail-hit")
                    .relative()
                    .w(px(RAIL_WIDTH))
                    .flex_shrink(1.)
                    .min_w(px(RAIL_MIN_WIDTH))
                    .h(px(TAB_PLATE_HEIGHT))
                    .px(px(8.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .when(has_plan, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(content(0.04)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open = !this.open;
                                cx.notify();
                            }))
                    })
                    .child(self.anchor.probe())
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child("Plan"),
                    )
                    .child(self.track()),
            )
            .child(
                div()
                    .flex_shrink_1()
                    .min_w_0()
                    .max_w(px(320.))
                    .child(self.readout(cx)),
            )
            .children(popover)
            .into_any_element()
    }
}

fn amber(alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(ACTIVE).into();
    color.a = alpha;
    color
}

fn ring(color: Hsla, dashed: bool) -> AnyElement {
    div()
        .size(px(11.))
        .rounded_full()
        .border_1()
        .when(dashed, |d| d.border_dashed())
        .border_color(color)
        .into_any_element()
}

fn small_button(id: impl Into<gpui::ElementId>, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .cursor_pointer()
        .flex_shrink_0()
        .px(px(8.))
        .h(px(22.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(content(0.06))
        .hover(|s| s.bg(content(0.1)))
        .text_size(px(12.))
        .text_color(content(0.85))
        .child(label)
}
