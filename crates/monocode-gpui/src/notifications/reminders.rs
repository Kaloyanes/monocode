//! ReminderNotices.tsx. Persistence and opening sessions belong to the host.
use crate::{
    popover::{Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, menu_popover},
    theme::*,
};
use chrono::{Local, TimeZone};
use gpui::{
    Context, Div, Entity, EventEmitter, Pixels, Role, Size, Stateful, Subscription, Window, div,
    prelude::*, px,
};
use monocode_core::session_groups::{reminder_at, reminder_presets};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub session_id: i64,
    pub due_at: u64,
    pub title: String,
    pub project: String,
    pub time_label: String,
}
#[derive(Clone)]
pub(crate) enum Action {
    Open {
        session_id: i64,
        due_at: u64,
    },
    Dismiss {
        session_id: i64,
        due_at: u64,
    },
    Snooze {
        session_id: i64,
        expected_due_at: u64,
        due_at: u64,
    },
    Retry,
    OpenSettings,
}
struct Snooze {
    target: (i64, u64),
    anchor: AnchorSlot,
    menu: Entity<Menu>,
    _events: Subscription,
}
/// A row playing its exit, kept in its old place until the animation ends.
struct LeavingRow {
    token: u64,
    index: usize,
    reminder: Snapshot,
    size: Size<Pixels>,
}
type RowKey = (i64, u64);
fn row_key(reminder: &Snapshot) -> RowKey {
    (reminder.session_id, reminder.due_at)
}
#[derive(Default)]
pub(crate) struct View {
    reminders: Vec<Snapshot>,
    error: Option<String>,
    desktop_alerts: bool,
    snooze: Option<Snooze>,
    leaving: Vec<LeavingRow>,
    next_token: u64,
    /// Painted row sizes, which let a leaving row collapse its place.
    sizes: Rc<RefCell<HashMap<RowKey, Size<Pixels>>>>,
}
impl EventEmitter<Action> for View {}
impl View {
    pub(crate) fn sync(
        &mut self,
        reminders: Vec<Snapshot>,
        error: Option<String>,
        desktop_alerts: bool,
        cx: &mut Context<Self>,
    ) {
        let mut unchanged = self.reminders == reminders
            && self.error == error
            && self.desktop_alerts == desktop_alerts;
        if self.snooze.as_ref().is_some_and(|s| {
            !reminders
                .iter()
                .any(|r| (r.session_id, r.due_at) == s.target)
        }) {
            self.snooze = None;
            unchanged = false;
        }
        let previous = std::mem::replace(&mut self.reminders, reminders);
        self.error = error;
        self.desktop_alerts = desktop_alerts;
        // Shell syncs on every render; only a change may redraw.
        if !unchanged {
            self.leave_rows(previous, cx);
            cx.notify();
        }
    }
    /// The last row goes with the card, which plays its own exit.
    fn leave_rows(&mut self, previous: Vec<Snapshot>, cx: &mut Context<Self>) {
        let shown: HashSet<_> = self.reminders.iter().map(row_key).collect();
        self.leaving
            .retain(|row| !shown.contains(&row_key(&row.reminder)));
        for (index, reminder) in previous.into_iter().enumerate() {
            let key = row_key(&reminder);
            if shown.contains(&key) {
                continue;
            }
            let size = self.sizes.borrow_mut().remove(&key);
            let Some(size) = size.filter(|_| !self.is_empty() && !cx.reduce_motion()) else {
                continue;
            };
            let token = self.next_token;
            self.next_token += 1;
            self.leaving.push(LeavingRow {
                token,
                index,
                reminder,
                size,
            });
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(super::surface::TOAST_EXIT)
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.leaving.retain(|row| row.token != token);
                    cx.notify();
                });
            })
            .detach();
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.reminders.is_empty() && self.error.is_none()
    }
    /// A still copy of the card, for its exit.
    pub(crate) fn frozen(&self) -> Self {
        Self {
            reminders: self.reminders.clone(),
            error: self.error.clone(),
            desktop_alerts: self.desktop_alerts,
            ..Self::default()
        }
    }
    fn snooze(
        &mut self,
        target: (i64, u64),
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = reminder_presets(Local::now().naive_local())
            .into_iter()
            .map(|(id, label, enabled)| {
                let mut action = MenuAction::new(id, label);
                action.disabled = !enabled;
                action.into()
            })
            .collect();
        let menu = cx.new(|cx| Menu::new("Snooze reminder", items, window, cx));
        let events = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Picked(id, _) = event
                && this
                    .reminders
                    .iter()
                    .any(|r| (r.session_id, r.due_at) == target)
                && let Some(at) = reminder_at(id.as_ref(), Local::now().naive_local())
                    .and_then(|at| Local.from_local_datetime(&at).earliest())
            {
                cx.emit(Action::Snooze {
                    session_id: target.0,
                    expected_due_at: target.1,
                    due_at: at.timestamp_millis().max(0) as u64,
                });
            }
            this.snooze = None;
            cx.notify();
        });
        self.snooze = Some(Snooze {
            target,
            anchor,
            menu,
            _events: events,
        });
        cx.notify();
    }
    fn row(&self, ix: usize, reminder: &Snapshot, cx: &mut Context<Self>) -> Stateful<Div> {
        let target = (reminder.session_id, reminder.due_at);
        let anchor = AnchorSlot::default();
        let trigger = anchor.clone();
        div()
            .id(("due-reminder", ix))
            .px_3()
            .py(px(10.))
            .when(ix > 0, |d| d.border_t_1().border_color(content(0.1)))
            .child(
                div()
                    .id(("due-reminder-title", ix))
                    .role(Role::Button)
                    .cursor_pointer()
                    .w_full()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Action::Open {
                            session_id: target.0,
                            due_at: target.1,
                        })
                    }))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(reminder.title.clone()),
                    )
                    .child(
                        div()
                            .mt_1()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child(format!("{} · {}", reminder.project, reminder.time_label)),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .child(
                        div()
                            .id(("due-reminder-open", ix))
                            .role(Role::Button)
                            .cursor_pointer()
                            .rounded_md()
                            .bg(content(0.1))
                            .hover(|s| s.bg(content(0.15)))
                            .px_2()
                            .py_1()
                            .child("Open session")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(Action::Open {
                                    session_id: target.0,
                                    due_at: target.1,
                                })
                            })),
                    )
                    .child(
                        div()
                            .id(("due-reminder-snooze", ix))
                            .role(Role::Button)
                            .aria_label("Snooze")
                            .cursor_pointer()
                            .relative()
                            .rounded_md()
                            .text_color(content(0.65))
                            .hover(|s| s.bg(content(0.1)))
                            .px_2()
                            .py_1()
                            .child("Snooze")
                            .child(anchor.probe())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.snooze(target, trigger.clone(), window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(("due-reminder-dismiss", ix))
                            .role(Role::Button)
                            .cursor_pointer()
                            .rounded_md()
                            .text_color(content(0.5))
                            .hover(|s| s.bg(content(0.1)))
                            .px_2()
                            .py_1()
                            .child("Dismiss")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(Action::Dismiss {
                                    session_id: target.0,
                                    due_at: target.1,
                                })
                            })),
                    ),
            )
    }
}
impl Render for View {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let max_height = (f32::from(window.viewport_size().height) * 0.5).min(320.);
        let mut entries: Vec<_> = self.reminders.iter().map(|r| (r, None)).collect();
        for row in &self.leaving {
            let place = row.index.min(entries.len());
            entries.insert(place, (&row.reminder, Some((row.token, row.size))));
        }
        let rows = entries
            .into_iter()
            .enumerate()
            .map(|(ix, (reminder, leaving))| {
                let row = self.row(ix, reminder, cx);
                if let Some((token, size)) = leaving {
                    let id = ("due-reminder-exit", token);
                    return super::surface::row_exit(id, size, row).into_any_element();
                }
                let (sizes, key) = (self.sizes.clone(), row_key(reminder));
                div()
                    .relative()
                    .child(row)
                    .child(super::surface::size_probe(move |size| {
                        sizes.borrow_mut().insert(key, size);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let mut fill = pane_fill();
        fill.a = 0.95;
        div()
            .relative()
            .w_full()
            .child(
                div()
                    .id("reminder-accessible-1")
                    .relative()
                    .role(Role::Region)
                    .aria_label("Due reminders")
                    .overflow_hidden()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(content(0.15))
                    .bg(fill)
                    .text_color(content(1.))
                    .shadow_xl()
                    .child(super::surface::backdrop(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(content(0.1))
                            .px_3()
                            .py(px(10.))
                            .child(icon("clock", 14.).text_color(gpui::rgb(0xffb900)))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Due reminders"),
                            )
                            .child(
                                div()
                                    .id("reminder-accessible-2")
                                    .role(Role::Status)
                                    .text_size(px(11.))
                                    .text_color(content(0.5))
                                    .child(if self.reminders.is_empty() {
                                        String::new()
                                    } else {
                                        self.reminders.len().to_string()
                                    }),
                            ),
                    )
                    .when(self.error.is_some(), |d| {
                        d.child(
                            div()
                                .id("reminder-accessible-3")
                                .role(Role::Alert)
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(content(0.7))
                                .flex()
                                .gap_1()
                                .child("Couldn’t load reminders.")
                                .child(
                                    div()
                                        .id("reminder-retry")
                                        .role(Role::Button)
                                        .cursor_pointer()
                                        .underline()
                                        .child("Retry")
                                        .on_click(
                                            cx.listener(|_, _, _, cx| cx.emit(Action::Retry)),
                                        ),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("due-reminder-list")
                            .max_h(px(max_height))
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .when(!self.desktop_alerts && !self.reminders.is_empty(), |d| {
                        d.child(
                            div()
                                .id("reminder-settings")
                                .role(Role::Button)
                                .cursor_pointer()
                                .w_full()
                                .border_t_1()
                                .border_color(content(0.1))
                                .px_3()
                                .py_2()
                                .text_size(px(11.))
                                .text_color(content(0.5))
                                .hover(|s| s.text_color(content(1.)))
                                .child("Desktop alerts are off. Enable in Settings.")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(Action::OpenSettings))),
                        )
                    }),
            )
            .children(
                self.snooze
                    .as_ref()
                    .map(|s| menu_popover(&s.menu, Anchor::Slot(s.anchor.clone()), cx).gap(4.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::surface::TOAST_EXIT;
    use gpui::{TestAppContext, size};

    fn rows(ids: &[i64]) -> Vec<Snapshot> {
        ids.iter()
            .map(|id| Snapshot {
                session_id: *id,
                due_at: 1,
                title: format!("Reminder {id}"),
                project: "monocode".into(),
                time_label: "Now".into(),
            })
            .collect()
    }
    fn painted(view: &View) {
        for reminder in &view.reminders {
            view.sizes
                .borrow_mut()
                .insert(row_key(reminder), size(px(320.), px(90.)));
        }
    }
    fn showing(ids: &[i64], cx: &mut TestAppContext) -> Entity<View> {
        let view = cx.new(|cx| {
            let mut view = View::default();
            view.sync(rows(ids), None, true, cx);
            painted(&view);
            view
        });
        cx.run_until_parked();
        view
    }
    fn show(view: &Entity<View>, ids: &[i64], cx: &mut TestAppContext) {
        view.update(cx, |view, cx| view.sync(rows(ids), None, true, cx));
    }
    fn leaving(view: &Entity<View>, cx: &mut TestAppContext) -> Vec<i64> {
        view.read_with(cx, |view, _| {
            let rows = view.leaving.iter();
            rows.map(|row| row.reminder.session_id).collect()
        })
    }

    #[gpui::test]
    fn a_removed_row_stays_until_its_exit_ends(cx: &mut TestAppContext) {
        let view = showing(&[1, 2, 3], cx);
        show(&view, &[1, 3], cx);
        assert_eq!(leaving(&view, cx), [2]);
        assert_eq!(view.read_with(cx, |view, _| view.leaving[0].index), 1);

        cx.executor().advance_clock(TOAST_EXIT);
        cx.run_until_parked();
        assert!(leaving(&view, cx).is_empty());
    }

    #[gpui::test]
    fn a_row_coming_back_cancels_its_exit(cx: &mut TestAppContext) {
        let view = showing(&[1, 2], cx);
        show(&view, &[1], cx);
        show(&view, &[1, 2], cx);
        assert!(leaving(&view, cx).is_empty());
    }

    #[gpui::test]
    fn the_last_row_leaves_with_the_card(cx: &mut TestAppContext) {
        let view = showing(&[1], cx);
        show(&view, &[], cx);
        assert!(leaving(&view, cx).is_empty());
    }

    #[gpui::test]
    fn the_last_row_leaves_alone_while_an_error_keeps_the_card(cx: &mut TestAppContext) {
        let view = showing(&[1], cx);
        view.update(cx, |view, cx| {
            view.sync(Vec::new(), Some("Failed".into()), true, cx)
        });
        assert_eq!(leaving(&view, cx), [1]);
    }

    #[gpui::test]
    fn reduced_motion_removes_rows_at_once(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let view = showing(&[1, 2], cx);
        show(&view, &[2], cx);
        assert!(leaving(&view, cx).is_empty());
    }
}
