//! Pending approval/question cards and request-level announcement bookkeeping.
use crate::theme::*;
use gpui::{
    AnyElement, Context, Div, ElementId, EventEmitter, Pixels, Role, Size, Stateful, div,
    prelude::*, px,
};
use monocode_core::agent::Decision;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::SystemTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum InputKind {
    Approval,
    Question,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingInput {
    pub request_id: String,
    pub kind: InputKind,
    pub label: String,
    /// The provider can save this exact approval as a standing permission.
    pub can_always_allow: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SessionInputs {
    pub session_id: String,
    pub title: String,
    /// Provider key accepted by `theme::provider_icon`.
    pub provider: String,
    pub harness: String,
    /// Approvals in transcript order, followed by the pending question(s).
    pub pending: Vec<PendingInput>,
    pub inbox_ask: bool,
    pub former_worker: bool,
    /// Active tab, focused pane, and focused composer must all match.
    pub conversation_focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InputKey {
    pub session_id: String,
    pub kind: InputKind,
    pub request_id: String,
}
impl SessionInputs {
    fn key(&self, input: &PendingInput) -> InputKey {
        InputKey {
            session_id: self.session_id.clone(),
            kind: input.kind,
            request_id: input.request_id.clone(),
        }
    }
    fn notice(&self, dismissed: &HashSet<InputKey>) -> Option<&PendingInput> {
        let open = |p: &&PendingInput| !dismissed.contains(&self.key(p));
        self.pending
            .iter()
            .filter(open)
            .find(|p| p.kind == InputKind::Question)
            .or_else(|| {
                self.pending
                    .iter()
                    .rev()
                    .filter(open)
                    .find(|p| p.kind == InputKind::Approval)
            })
    }
}

pub(crate) enum Action {
    OpenSession {
        session_id: String,
    },
    Decide {
        session_id: String,
        request_id: String,
        decision: Decision,
    },
}

/// Mirrors useInputNotifications: coalesce each update per session, retain
/// unannounced requests, and forget resolved keys. Mark before policy checks.
#[derive(Default)]
pub(crate) struct InputAnnouncements {
    notified: HashSet<InputKey>,
}
impl InputAnnouncements {
    pub(crate) fn reconcile(
        &mut self,
        sessions: &[SessionInputs],
    ) -> Vec<(SessionInputs, PendingInput)> {
        let pending: HashSet<_> = sessions
            .iter()
            .filter(|s| !s.inbox_ask)
            .flat_map(|s| s.pending.iter().map(|p| s.key(p)))
            .collect();
        self.notified.retain(|key| pending.contains(key));
        let mut announcements = Vec::new();
        for session in sessions.iter().filter(|s| !s.inbox_ask) {
            if let Some(input) = session
                .pending
                .iter()
                .find(|p| !self.notified.contains(&session.key(p)))
            {
                self.notified.insert(session.key(input));
                announcements.push((session.clone(), input.clone()));
            }
        }
        announcements
    }
}

/// A card playing its exit, kept in its old slot until the animation ends.
struct Leaving {
    token: u64,
    index: usize,
    session: SessionInputs,
    input: PendingInput,
    size: Size<Pixels>,
}

#[derive(Default)]
pub(crate) struct ApprovalNotices {
    notices: Vec<(SessionInputs, PendingInput)>,
    occurred: HashMap<InputKey, SystemTime>,
    /// Hidden cards. The requests stay pending in their threads, and a new
    /// request from the same session brings a card back.
    dismissed: HashSet<InputKey>,
    leaving: Vec<Leaving>,
    next_token: u64,
    /// Painted card sizes, which let a leaving card collapse its slot.
    sizes: Rc<RefCell<HashMap<InputKey, Size<Pixels>>>>,
}
impl EventEmitter<Action> for ApprovalNotices {}
impl ApprovalNotices {
    /// Observe pending requests from session events even while the window is
    /// occluded. Policy must retain this first observation until resolution.
    pub(crate) fn track_pending(&mut self, sessions: &[SessionInputs]) {
        let now = SystemTime::now();
        let pending: HashSet<_> = sessions
            .iter()
            .flat_map(|session| session.pending.iter().map(|input| session.key(input)))
            .collect();
        self.occurred.retain(|key, _| pending.contains(key));
        self.dismissed.retain(|key| pending.contains(key));
        for key in pending {
            self.occurred.entry(key).or_insert(now);
        }
    }
    /// `allows` must apply known-project + agentInput project policy using the
    /// original occurrence timestamp, so resuming cannot revive muted requests.
    pub(crate) fn sync(
        &mut self,
        sessions: &[SessionInputs],
        mut allows: impl FnMut(&SessionInputs, SystemTime) -> bool,
        cx: &mut Context<Self>,
    ) {
        self.track_pending(sessions);
        let previous = std::mem::take(&mut self.notices);
        for session in sessions {
            // Subagents ask here too: their lead's strip only shows that they
            // wait.
            if session.inbox_ask || session.conversation_focused {
                continue;
            }
            let Some(input) = session.notice(&self.dismissed) else {
                continue;
            };
            let key = session.key(input);
            let occurred = *self.occurred.entry(key).or_insert_with(SystemTime::now);
            if allows(session, occurred) {
                self.notices.push((session.clone(), input.clone()));
            }
        }
        // Shell syncs on every render; notifying unchanged notices would
        // dirty the shell again and redraw the window every frame.
        if self.notices != previous {
            let shown: HashSet<_> = self.notices.iter().map(|(s, i)| s.key(i)).collect();
            self.leaving
                .retain(|card| !shown.contains(&card.session.key(&card.input)));
            for (index, (session, input)) in previous.into_iter().enumerate() {
                if !shown.contains(&session.key(&input)) {
                    self.leave(index, session, input, cx);
                }
            }
            cx.notify();
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.notices.is_empty() && self.leaving.is_empty()
    }
    /// Hides the session's card until it has a request not pending now.
    fn dismiss(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(index) = self
            .notices
            .iter()
            .position(|(session, _)| session.session_id == session_id)
        else {
            return;
        };
        let (session, input) = self.notices.remove(index);
        self.dismissed
            .extend(session.pending.iter().map(|p| session.key(p)));
        self.leave(index, session, input, cx);
        cx.notify();
    }
    /// A card that never painted has nothing to animate away from.
    fn leave(
        &mut self,
        index: usize,
        session: SessionInputs,
        input: PendingInput,
        cx: &mut Context<Self>,
    ) {
        let size = self.sizes.borrow_mut().remove(&session.key(&input));
        let Some(size) = size.filter(|_| !cx.reduce_motion()) else {
            return;
        };
        let token = self.next_token;
        self.next_token += 1;
        self.leaving.push(Leaving {
            token,
            index,
            session,
            input,
            size,
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::surface::TOAST_EXIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.leaving.retain(|card| card.token != token);
                cx.notify();
            });
        })
        .detach();
    }
    fn card(
        &self,
        session: &SessionInputs,
        input: &PendingInput,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open_id = session.session_id.clone();
        let allow_session = session.session_id.clone();
        let deny_session = session.session_id.clone();
        let allow_request = input.request_id.clone();
        let deny_request = input.request_id.clone();
        let always_session = session.session_id.clone();
        let always_request = input.request_id.clone();
        let dismiss_session = session.session_id.clone();
        super::surface::toast_frame()
            .id(card_id("approval-toast", &session.key(input)))
            .role(Role::Status)
            .child(
                div()
                    .id("approval-toast-open")
                    .role(Role::Button)
                    .cursor_pointer()
                    .px(px(14.))
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .hover(|s| s.bg(content(0.05)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Action::OpenSession {
                            session_id: open_id.clone(),
                        })
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pr(px(24.))
                            .child(provider_icon(&session.provider, 16.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(session.title.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(11.))
                                    .text_color(gpui::rgb(0xffb900))
                                    .child(
                                        icon("circle-alert", 14.).text_color(gpui::rgb(0xffb900)),
                                    )
                                    .child(if input.kind == InputKind::Question {
                                        "Question"
                                    } else {
                                        "Approval"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(19.5))
                            .line_clamp(3)
                            .text_color(content(0.7))
                            .child(input.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(session.harness.clone()),
                    ),
            )
            .child(
                // Occluded so a click here stays off the row underneath.
                div().absolute().top(px(10.)).right(px(6.)).occlude().child(
                    icon_button("approval-toast-dismiss", "Dismiss", "close")
                        .debug_selector(|| "approval-toast-dismiss".into())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.dismiss(&dismiss_session, cx)),
                        ),
                ),
            )
            .when(input.kind == InputKind::Approval, |card| {
                card.child(
                    div()
                        .flex()
                        .gap_2()
                        .border_t_1()
                        .border_color(content(0.1))
                        .px(px(14.))
                        .py(px(10.))
                        .child(
                            div()
                                .id("approval-toast-allow")
                                .role(Role::Button)
                                .aria_label("Allow")
                                .cursor_pointer()
                                .flex_1()
                                .rounded_md()
                                .px(px(10.))
                                .py_1()
                                .text_center()
                                .text_size(px(11.))
                                .bg(content(1.))
                                .text_color({
                                    let mut color = pane_fill();
                                    color.a = 1.;
                                    color
                                })
                                .hover(|s| s.bg(content(0.8)))
                                .child("Allow")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(Action::Decide {
                                        session_id: allow_session.clone(),
                                        request_id: allow_request.clone(),
                                        decision: Decision::Allow,
                                    })
                                })),
                        )
                        .when(input.can_always_allow, |row| {
                            row.child(
                                div()
                                    .id("approval-toast-always")
                                    .role(Role::Button)
                                    .aria_label("Always allow")
                                    .cursor_pointer()
                                    .flex_1()
                                    .rounded_md()
                                    .px(px(10.))
                                    .py_1()
                                    .text_center()
                                    .text_size(px(11.))
                                    .bg(content(0.1))
                                    .text_color(content(0.7))
                                    .hover(|s| s.bg(content(0.2)))
                                    .child("Always allow")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(Action::Decide {
                                            session_id: always_session.clone(),
                                            request_id: always_request.clone(),
                                            decision: Decision::Always,
                                        })
                                    })),
                            )
                        })
                        .child(
                            div()
                                .id("approval-toast-deny")
                                .role(Role::Button)
                                .aria_label("Deny")
                                .cursor_pointer()
                                .flex_1()
                                .rounded_md()
                                .px(px(10.))
                                .py_1()
                                .text_center()
                                .text_size(px(11.))
                                .bg(content(0.1))
                                .text_color(content(0.7))
                                .hover(|s| s.bg(content(0.2)))
                                .child("Deny")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(Action::Decide {
                                        session_id: deny_session.clone(),
                                        request_id: deny_request.clone(),
                                        decision: Decision::Deny,
                                    })
                                })),
                        ),
                )
            })
    }
}
impl Render for ApprovalNotices {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut cards: Vec<AnyElement> = self
            .notices
            .iter()
            .map(|(session, input)| {
                let key = session.key(input);
                let id = card_id("approval-toast-enter", &key);
                let sizes = self.sizes.clone();
                let probe = super::surface::size_probe(move |size| {
                    sizes.borrow_mut().insert(key.clone(), size);
                });
                let card = div()
                    .relative()
                    .w_full()
                    .child(self.card(session, input, cx))
                    .child(probe);
                super::surface::toast_enter(id, card).into_any_element()
            })
            .collect();
        for card in &self.leaving {
            let leaving = super::surface::toast_exit(
                ("approval-toast-exit", card.token),
                card.size,
                self.card(&card.session, &card.input, cx),
            )
            .into_any_element();
            cards.insert(card.index.min(cards.len()), leaving);
        }
        div()
            .flex()
            .flex_col()
            .gap(px(super::surface::TOAST_GAP))
            .w_full()
            .children(cards)
    }
}

fn card_id(name: &str, key: &InputKey) -> ElementId {
    ElementId::Name(
        format!(
            "{name}-{}-{:?}-{}",
            key.session_id, key.kind, key.request_id
        )
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::cell::Cell;

    fn input(id: &str, kind: InputKind) -> PendingInput {
        PendingInput {
            request_id: id.into(),
            kind,
            label: id.into(),
            can_always_allow: false,
        }
    }

    fn session(pending: Vec<PendingInput>) -> SessionInputs {
        SessionInputs {
            session_id: "1".into(),
            title: "Thread".into(),
            provider: "claude".into(),
            harness: "Claude Code".into(),
            pending,
            inbox_ask: false,
            former_worker: false,
            conversation_focused: false,
        }
    }

    fn shown(notices: &ApprovalNotices) -> Vec<&str> {
        notices
            .notices
            .iter()
            .map(|(_, input)| input.request_id.as_str())
            .collect()
    }

    fn painted(notices: &ApprovalNotices) {
        for (session, input) in &notices.notices {
            notices
                .sizes
                .borrow_mut()
                .insert(session.key(input), gpui::size(px(360.), px(80.)));
        }
    }

    #[gpui::test]
    fn dismissing_hides_every_pending_request_until_a_new_one_arrives(cx: &mut TestAppContext) {
        let notices = cx.new(|_| ApprovalNotices::default());
        let older = input("older", InputKind::Approval);
        let question = input("question", InputKind::Question);
        let newer = input("newer", InputKind::Approval);
        notices.update(cx, |notices, cx| {
            let pending = vec![older.clone(), question.clone()];
            notices.sync(&[session(pending.clone())], |_, _| true, cx);
            assert_eq!(shown(notices), ["question"]);

            notices.dismiss("1", cx);
            notices.sync(&[session(pending)], |_, _| true, cx);
            assert!(shown(notices).is_empty());

            let pending = vec![older.clone(), newer.clone(), question.clone()];
            notices.sync(&[session(pending)], |_, _| true, cx);
            assert_eq!(shown(notices), ["newer"]);
        });
    }

    #[gpui::test]
    fn a_resolved_request_forgets_its_dismissal(cx: &mut TestAppContext) {
        let notices = cx.new(|_| ApprovalNotices::default());
        let question = input("question", InputKind::Question);
        notices.update(cx, |notices, cx| {
            notices.sync(&[session(vec![question.clone()])], |_, _| true, cx);
            notices.dismiss("1", cx);
            notices.sync(&[session(Vec::new())], |_, _| true, cx);
            assert!(notices.dismissed.is_empty());

            notices.sync(&[session(vec![question.clone()])], |_, _| true, cx);
            assert_eq!(shown(notices), ["question"]);
        });
    }

    #[gpui::test]
    fn a_removed_card_stays_until_its_exit_ends(cx: &mut TestAppContext) {
        let notices = cx.new(|_| ApprovalNotices::default());
        let question = input("question", InputKind::Question);
        notices.update(cx, |notices, cx| {
            notices.sync(&[session(vec![question.clone()])], |_, _| true, cx);
            painted(notices);
            notices.dismiss("1", cx);
            assert!(!notices.is_empty());
        });
        cx.executor()
            .advance_clock(super::super::surface::TOAST_EXIT);
        cx.run_until_parked();
        notices.read_with(cx, |notices, _| assert!(notices.is_empty()));
    }

    #[gpui::test]
    fn a_card_that_returns_cancels_its_exit(cx: &mut TestAppContext) {
        let notices = cx.new(|_| ApprovalNotices::default());
        let question = input("question", InputKind::Question);
        notices.update(cx, |notices, cx| {
            let visible = session(vec![question.clone()]);
            notices.sync(std::slice::from_ref(&visible), |_, _| true, cx);
            painted(notices);

            let focused = SessionInputs {
                conversation_focused: true,
                ..visible.clone()
            };
            notices.sync(&[focused], |_, _| true, cx);
            assert_eq!(notices.leaving.len(), 1);

            notices.sync(&[visible], |_, _| true, cx);
            assert!(notices.leaving.is_empty());
            assert_eq!(shown(notices), ["question"]);
        });
    }

    #[gpui::test]
    fn the_close_button_does_not_open_the_thread(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| ApprovalNotices::default())
            })
            .unwrap()
        });
        let opened = Rc::new(Cell::new(false));
        let seen = opened.clone();
        let _events = window
            .update(cx, |notices, _, cx| {
                let pending = vec![input("question", InputKind::Question)];
                notices.sync(&[session(pending)], |_, _| true, cx);
                cx.subscribe(&cx.entity(), move |_, _, action: &Action, _| {
                    seen.set(matches!(action, Action::OpenSession { .. }))
                })
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        let close = visual.debug_bounds("approval-toast-dismiss").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("approval-toast-dismiss").is_none());
        assert!(!opened.get());
    }

    #[gpui::test]
    fn reduced_motion_removes_a_card_at_once(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let notices = cx.new(|_| ApprovalNotices::default());
        let question = input("question", InputKind::Question);
        notices.update(cx, |notices, cx| {
            notices.sync(&[session(vec![question.clone()])], |_, _| true, cx);
            painted(notices);
            notices.dismiss("1", cx);
            assert!(notices.is_empty());
        });
    }
}
