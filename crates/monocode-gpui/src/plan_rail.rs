//! The title bar's plan rail: the active session's plan as its agent last
//! wrote it with a todo tool, in place of the session tabs. It is its own
//! cached view, so a streaming transcript repaints only the rail.
use crate::conversation::Conversation;
use crate::popover::{self, Anchor, AnchorSlot};
use crate::pulse::{self, Rate};
use crate::theme::*;
use crate::tooltip::Tip;
use crate::transcript::TranscriptView;
use gpui::{
    AnyElement, Context, Entity, EventEmitter, Hsla, Role, SharedString, Subscription, WeakEntity,
    Window, div, prelude::*, px, rgb,
};
use monocode_core::tool::{
    plan::{self, Plan, Proposal, Snapshot},
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
/// Ask for a plan once a turn has done some real work without one. A small
/// list is still worth having, so this is only a few calls in.
const ASK_AFTER_CALLS: usize = 3;
const ASK_FOR_PLAN: &str = "Before you continue, lay out the remaining work as a short \
                            todo list with your todo tool, then keep it updated as you go.";
const TICK_PITCH: f32 = 5.;
const HAIR_PITCH: f32 = 7.;
const PROPOSAL_HEIGHT: f32 = 360.;
/// A pending step's two hover actions, which its text wraps clear of.
const ACTIONS_WIDTH: f32 = 48.;

pub(crate) enum PreviewEvent {
    Open,
    Close,
}

impl EventEmitter<PreviewEvent> for PlanRail {}

pub(crate) struct PlanSidebar {
    rail: Entity<PlanRail>,
    _observer: Subscription,
}

impl PlanSidebar {
    pub(crate) fn new(rail: Entity<PlanRail>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(&rail, |_, _, cx| cx.notify());
        Self {
            rail,
            _observer: observer,
        }
    }
}

impl Render for PlanSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rail.update(cx, |rail, cx| rail.sidebar(cx))
    }
}

pub(crate) struct PlanRail {
    session: Option<i64>,
    conversation: Option<WeakEntity<Conversation>>,
    snapshot: Snapshot,
    busy: bool,
    /// The running turn is in Plan mode: its answer will be a plan to review.
    planning: bool,
    attention: bool,
    open: bool,
    /// The proposal rendered as Markdown, kept while its text is unchanged.
    reader: Option<(String, Entity<TranscriptView>)>,
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
            planning: false,
            attention: false,
            open: false,
            reader: None,
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
        let (snapshot, busy, planning, attention) = {
            let c = conversation.read(cx);
            let rows = c.transcript.read(cx).source_rows();
            let busy = c.responding();
            (
                plan::snapshot(rows, c.plan_turn),
                busy,
                busy && c.plan_turn,
                c.needs_approval(),
            )
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
        if (snapshot.clone(), busy, planning, attention)
            != (
                self.snapshot.clone(),
                self.busy,
                self.planning,
                self.attention,
            )
        {
            self.snapshot = snapshot;
            self.busy = busy;
            self.planning = planning;
            self.attention = attention;
            if self.ready().is_none() && self.snapshot.plan.is_none() {
                self.open = false;
            }
            cx.notify();
        }
        self.ask_unprompted(&conversation, cx);
    }

    /// A turn that has worked a few calls with no list gets asked, once, to
    /// write one. The agent keeps it from there; nothing is asked of the user.
    fn ask_unprompted(&mut self, conversation: &Entity<Conversation>, cx: &mut Context<Self>) {
        let c = conversation.read(cx);
        let due = self.busy
            && !self.planning
            && self.asked.is_none()
            && !c.plan_asked
            && self.snapshot.plan.is_none()
            && self.snapshot.proposal.is_none()
            && self.snapshot.turn_calls >= ASK_AFTER_CALLS
            && c.can_steer_plan();
        if due {
            self.steer("a plan".into(), ASK_FOR_PLAN.into(), cx);
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

    /// A proposal waiting for review: the agent has stopped and nothing built it yet.
    fn ready(&self) -> Option<&Proposal> {
        self.snapshot
            .proposal
            .as_ref()
            .filter(|p| !p.building && !self.busy)
    }

    /// Building the approved plan before the agent has written its todo list.
    fn building(&self) -> bool {
        self.snapshot.proposal.as_ref().is_some_and(|p| p.building)
            && self.snapshot.plan.as_ref().is_none_or(|p| p.stale)
    }

    fn build(&mut self, cx: &mut Context<Self>) {
        let Some(conversation) = self.conversation.as_ref().and_then(|c| c.upgrade()) else {
            return;
        };
        conversation.update(cx, |c, cx| c.build_proposed_plan(cx));
        self.open = false;
        cx.notify();
    }

    fn reader(&mut self, text: &str, cx: &mut Context<Self>) -> Entity<TranscriptView> {
        if let Some((cached, view)) = &self.reader
            && cached == text
        {
            return view.clone();
        }
        let view = cx.new(|cx| TranscriptView::document(text.to_owned(), cx));
        self.reader = Some((text.to_owned(), view.clone()));
        view
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
        if self.ready().is_some() {
            // One whole document, not steps: the plan has none until it is built.
            return div()
                .id("plan-rail-ready")
                .flex_1()
                .h(px(RAIL_HEIGHT))
                .rounded(px(3.))
                .bg(teal(0.16))
                .border_1()
                .border_color(teal(0.6))
                .into_any_element();
        }
        let plan = snapshot
            .plan
            .as_ref()
            .filter(|p| !(self.planning || self.building()) || !p.stale);
        let Some(plan) = plan else {
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
                .font_family(crate::fonts::mono())
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
        if let Some(proposal) = self.ready() {
            return row
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(DONE))
                        .child("Plan ready:"),
                )
                .child(text(proposal.title().into(), content(0.85)))
                .child(
                    small_button("plan-rail-review", "Review")
                        .debug_selector(|| "plan-rail-review".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open = false;
                            cx.emit(PreviewEvent::Open);
                        })),
                )
                .child(
                    small_button("plan-rail-build", "Build").on_click(cx.listener(
                        |this, _, _, cx| {
                            cx.stop_propagation();
                            this.build(cx)
                        },
                    )),
                )
                .into_any_element();
        }
        if self.planning {
            return row
                .child(text("Planning".into(), content(0.85)))
                .child(count(format!("{} tool calls", snapshot.turn_calls)))
                .into_any_element();
        }
        if self.building() {
            // Once the turn ends the rail cannot tell whether the build worked,
            // so it names the plan rather than claiming it was built.
            let Some(proposal) = snapshot.proposal.as_ref().filter(|_| !self.busy) else {
                return row
                    .child(text("Building the approved plan".into(), content(0.85)))
                    .child(count(format!("{} tool calls", snapshot.turn_calls)))
                    .into_any_element();
            };
            return row
                .child(text(proposal.title().into(), content(0.45)))
                .into_any_element();
        }
        let Some(plan) = &snapshot.plan else {
            // No list to place the agent on, but its newest call says what it is doing.
            let label = match (&snapshot.now, self.busy) {
                (Some(now), true) => format!("{} {}", now.action, now.target).trim().to_owned(),
                (None, true) => format!("No plan · {} tool calls", snapshot.turn_calls),
                _ => "No plan".into(),
            };
            let ask = (self.busy && snapshot.turn_calls >= ASK_AFTER_CALLS).then(|| {
                small_button("plan-rail-ask", "Ask for plan").on_click(cx.listener(
                    |this, _, _, cx| this.steer("a plan".into(), ASK_FOR_PLAN.into(), cx),
                ))
            });
            return row
                .child(text(label.into(), content(0.6)))
                .when(self.busy && snapshot.now.is_some(), |r| {
                    r.child(count(format!("{} calls", snapshot.turn_calls)))
                })
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
        // The steps are the agent's own, but where it is on them is a guess.
        row.when(plan.inferred, |r| r.child(count("≈".into())))
            .child(text(current.into(), content(0.85)))
            .child(count(format!("{done}/{total}")))
            .into_any_element()
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(plan) = &self.snapshot.plan else {
            return div().into_any_element();
        };
        let steerable = self.busy && !plan.stale && !plan.inferred && self.conversation.is_some();
        let header = if plan.inferred {
            "The approved plan's steps. Where the agent is on them is estimated from its tool calls"
                .to_string()
        } else if plan.stale {
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
                // Over the row's end on hover, so idle rows keep their full width;
                // the step's text makes room for it then (see `ACTIONS_WIDTH`).
                div()
                    .absolute()
                    .right(px(4.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .invisible()
                    .group_hover("plan-row", |s| s.visible())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                step_action(("plan-next", ix), "move-to-top", "Do next").on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.steer(
                                        format!("do “{next}” next"),
                                        format!(
                                            "Change of order: do “{next}” next, before the other \
                                         remaining steps. Update your todo list to match."
                                        ),
                                        cx,
                                    )
                                    }),
                                ),
                            )
                            .child(
                                step_action(("plan-skip", ix), "skip-over", "Skip").on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.steer(
                                        format!("skip “{skip}”"),
                                        format!(
                                            "Skip “{skip}”: don't do that step. Mark it cancelled \
                                         in your todo list and continue with the rest."
                                        ),
                                        cx,
                                    )
                                    }),
                                ),
                            ),
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
                        .when(actions.is_some(), |d| {
                            d.group_hover("plan-row", |s| s.pr(px(ACTIONS_WIDTH)))
                        })
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

impl PlanRail {
    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let proposal = self.snapshot.proposal.as_ref().map(|p| p.text.clone());
        let ready = self.ready().is_some();
        let body = if let Some(text) = proposal {
            div()
                .flex_1()
                .min_h_0()
                .child(self.reader(&text, cx))
                .into_any_element()
        } else if self.snapshot.plan.is_some() {
            div()
                .id("plan-sidebar-tasks")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(12.))
                .child(self.list(cx))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .p(px(16.))
                .text_size(px(13.))
                .text_color(content(0.6))
                .child("No plan in this session yet.")
                .into_any_element()
        };
        div()
            .id("plan-sidebar")
            .debug_selector(|| "plan-sidebar".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                crate::right_panel::pane_header("plan-sidebar-header", "Plan")
                    .child(icon("list-bullet", 14.))
                    .child("Plan")
                    .child(div().flex_1())
                    .child(
                        crate::right_panel::icon_action(
                            "close-plan-preview",
                            "Close plan",
                            "x",
                            true,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PreviewEvent::Close))),
                    ),
            )
            .child(body)
            .when(ready, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .p(px(12.))
                        .border_t_1()
                        .border_color(crate::right_panel::stroke())
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(content(0.6))
                                .child("Reply in chat to revise this plan."),
                        )
                        .child(
                            small_button("plan-sidebar-build", "Build this plan")
                                .on_click(cx.listener(|this, _, _, cx| this.build(cx))),
                        ),
                )
            })
            .into_any_element()
    }

    /// The proposal as the agent wrote it, with the one action it waits on.
    fn proposal(&mut self, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let reader = self.reader(text, cx);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(content(0.08))
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child("Proposed in Plan mode · nothing has changed yet"),
            )
            .child(div().h(px(PROPOSAL_HEIGHT)).child(reader))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(8.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(content(0.08))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child("Or reply in the composer to revise it"),
                    )
                    .child(
                        small_button("plan-proposal-build", "Build this plan")
                            .on_click(cx.listener(|this, _, _, cx| this.build(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for PlanRail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.session.is_none() {
            return div().into_any_element();
        }
        let ready = self.ready().map(|p| p.text.clone());
        let has_plan = ready.is_some() || self.snapshot.plan.is_some();
        let popover = (self.open && has_plan).then(|| {
            let this = cx.entity().downgrade();
            let body = match &ready {
                Some(text) => self.proposal(text, cx),
                None => self.list(cx),
            };
            popover::popover("plan-rail-list", Anchor::Slot(self.anchor.clone()))
                .width(if ready.is_some() { 560. } else { 400. })
                .padding(6.)
                .gap(6.)
                .on_dismiss(move |_, cx| {
                    let _ = this.update(cx, |rail, cx| {
                        rail.open = false;
                        cx.notify();
                    });
                })
                .child(body)
        });
        div()
            .id("plan-rail")
            .debug_selector(|| "plan-rail".into())
            .role(Role::Button)
            .aria_label("Agent plan")
            .h(px(TITLE_BAR_HEIGHT))
            .w_full()
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
            .child(crate::shell::titlebar_drag_area().flex_1().min_w(px(24.)))
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

fn teal(alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(DONE).into();
    color.a = alpha;
    color
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

/// An icon-only step action, named by its tooltip.
fn step_action(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .group(SharedString::from(format!("{id}")))
        .role(Role::Button)
        .aria_label(label)
        .tip(label)
        .cursor_pointer()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(|s| s.bg(content(0.08)))
        .child(
            icon(path, 13.)
                .text_color(content(0.55))
                .group_hover(SharedString::from(format!("{id}")), |s| {
                    s.text_color(content(0.95))
                }),
        )
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
