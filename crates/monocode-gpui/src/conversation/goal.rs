//! A `/goal` keeps the agent going until its condition holds. While one is in
//! force it sits on the composer's ledge, so the thread says why the agent
//! keeps working and the person can drop the goal from there.
use super::*;
use crate::tooltip::Tip;

const LIVE: u32 = 0x4ade80;

impl Conversation {
    pub(super) fn goal_shelf(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let goal = self.transcript.read(cx).goal()?.clone();
        // Over the background card or queued messages it tucks in behind them.
        let below = [
            !self.background.tasks.is_empty(),
            !self.follow_ups.state.messages.is_empty(),
        ]
        .into_iter()
        .filter(|&shown| shown)
        .count();
        let inset = px(8. + 12. * below as f32);
        let responding = self.responding();
        let status = if goal.paused {
            "Paused".to_owned()
        } else if responding {
            "Working toward it".to_owned()
        } else {
            "Active".to_owned()
        };
        let status = match goal.checks {
            0 => status,
            1 => format!("{status} · checked once"),
            n => format!("{status} · checked {n} times"),
        };
        // Claude clears its goal with a command; Codex needs its own request.
        let clearable = !responding
            && !self.is_remote()
            && Kind::from_id(&self.summary.provider) == Kind::Claude;
        let card = composer_shelf::card("goal-card", self.classic_layout)
            .role(Role::Group)
            .aria_label(format!("Goal: {}", goal.objective))
            .child(
                div()
                    .id("goal-header")
                    .debug_selector(|| "goal-header".into())
                    .tip(goal.objective.clone())
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon("goal", 14.).text_color(if goal.paused {
                        content(0.45)
                    } else {
                        gpui::rgb(LIVE).into()
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(content(0.8))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Goal"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(content(0.45))
                            .child(goal.objective.replace('\n', " ")),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.35))
                            .child(status),
                    )
                    .when(clearable, |d| {
                        d.child(
                            follow_up::queue_button("goal-clear", "Clear goal", "close")
                                .size(px(20.))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send_thread_prompt("/goal clear".into(), cx);
                                })),
                        )
                    }),
            );
        Some(
            composer_shelf::shelf("goal")
                .px(inset)
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use monocode_core::store::Store;
    use monocode_core::transcript::{GoalChange, Interjection, Transcript};
    use std::sync::Mutex;

    fn note(t: &mut Transcript, change: GoalChange, text: &str) {
        t.begin("interjection");
        t.append(&serde_json::to_string(&Interjection::goal(change, text)).unwrap());
    }

    #[gpui::test]
    fn an_unfinished_goal_sits_on_the_ledge_until_it_is_met(cx: &mut TestAppContext) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("claude", "/tmp").unwrap();
        let window = cx.update(|cx| {
            cx.set_reduce_motion(true);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view.summary.has_messages = true;
                    view
                })
            })
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("goal-header").is_none());

        let mut t = Transcript::default();
        t.begin("user");
        t.append("/goal tests pass");
        note(&mut t, GoalChange::Set, "tests pass");
        t.begin("assistant");
        t.append("Running the tests");
        note(&mut t, GoalChange::Check, "two tests still fail");
        let rows = t.rows.clone();
        window
            .update(visual, |view, _, cx| {
                view.transcript.update(cx, |t, cx| t.patch(0, rows, cx));
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("goal-header").is_some());
        window
            .update(visual, |view, _, cx| {
                let goal = view.transcript.read(cx).goal().unwrap();
                assert_eq!((goal.objective.as_str(), goal.checks), ("tests pass", 1));
            })
            .unwrap();

        let start = t.rows.len();
        note(&mut t, GoalChange::Met, "");
        let rows = t.rows[start..].to_vec();
        window
            .update(visual, |view, _, cx| {
                view.transcript.update(cx, |t, cx| t.patch(start, rows, cx));
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("goal-header").is_none());
    }
}
