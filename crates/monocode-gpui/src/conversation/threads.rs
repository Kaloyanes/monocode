use super::*;

impl Conversation {
    pub(crate) fn driver_label(&self) -> String {
        self.model_label().to_string()
    }
    pub(crate) fn thread_outcome(&self, cx: &gpui::App) -> (bool, String) {
        let (success, fallback) = self.external_outcome(cx);
        let rows = self.transcript.read(cx).source_rows();
        let turn = rows.iter().rposition(|r| r.role == "user").unwrap_or(0);
        let reply = rows[turn..]
            .iter()
            .rposition(|r| r.role == "assistant" && r.first)
            .map(|start| monocode_core::response::prose(&rows[turn + start..]));
        (
            success && !self.notification_was_cancelled(),
            monocode_core::response::limit(reply.as_deref().unwrap_or(&fallback), 12000),
        )
    }
    pub(crate) fn thread_pending(&self) -> bool {
        self.external_submission_pending()
            || self.follow_ups.requested.is_some()
            || self.follow_ups.sending.is_some()
            || (!self.follow_ups.state.paused
                && (self.follow_ups.state.draft.is_some()
                    || !self.follow_ups.state.messages.is_empty()))
    }
    pub(crate) fn thread_stopping(&self) -> bool {
        self.stopping || self.notification_was_cancelled()
    }
    pub(crate) fn send_thread_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        if !self.history_ready() {
            self.defer_history_action(move |this, cx| this.send_thread_prompt(prompt, cx), cx);
            return;
        }
        self.launch_prompt(prompt, cx);
    }
    pub(super) fn provider_environment(
        &self,
        settings: &Settings,
    ) -> std::collections::HashMap<String, String> {
        let mut environment = settings.environment.clone();
        environment.extend(self.app_environment.clone());
        let enabled = self.driven_by.is_none()
            && !self.managed
            && settings.mono.is_none()
            && !self.is_remote();
        environment.insert(
            "MONOCODE_THREADS".into(),
            if enabled { "1" } else { "0" }.into(),
        );
        environment
    }
    pub(super) fn thread_composer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let lead = self.driven_by.expect("thread composer requires a lead");
        let driver = if self.thread_driver.is_empty() {
            "The lead".to_owned()
        } else {
            self.thread_driver.clone()
        };
        div().id("thread-locked-composer").debug_selector(||"thread-locked-composer".into()).role(gpui::Role::Group)
                .aria_label("Read-only child thread").flex().items_center().gap_2().p_3()
                .border_t_1().border_color(content(0.1))
                .child(icon("lock", 14.).text_color(content(0.5)))
                .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(content(0.6))
                    .child(format!("{driver} is driving this thread from {}. You can read it but not send messages.", self.thread_lead_title)))
                .child(crate::settings::widgets::secondary_button("thread-open-lead", "Open lead", false, false)
                    .child("Open lead").on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenThread(lead)))))
                .when(self.busy, |d| d.child(crate::settings::widgets::secondary_button("thread-stop", "Stop", true, false)
                    .child("Stop").on_click(cx.listener(|this, _, _, cx| this.stop_app_session(cx)))))
                .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::store::Store;
    use std::sync::{Arc, Mutex};

    fn window(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/thread-test").unwrap();
        let store = Arc::new(Mutex::new(store));
        cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Conversation::new(summary, store, cx))
            })
            .unwrap()
        })
    }

    #[gpui::test]
    fn child_threads_reject_every_user_submission_after_the_lead_finishes(cx: &mut TestAppContext) {
        let window = window(cx);
        window
            .update(cx, |v, window, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.driven_by = Some(42);
                v.input
                    .update(cx, |i, cx| i.set_text("User draft".into(), cx));
                let before = v.transcript.read(cx).source_rows().to_vec();
                v.send(cx);
                v.quick_submit("Quick submit".into(), Settings::default(), cx);
                v.mobile_submit("Mobile submit".into(), Vec::new(), cx);
                v.save_app_draft("Operator draft".into(), cx);
                v.submit_shell(cx);
                let settings = v.settings.clone();
                assert!(!v.apply_model_pick(&ModelPicked(Kind::Claude, Some("other".into())), cx));
                v.mobile_configure(
                    Some((Kind::Claude, Some("other".into()))),
                    &Default::default(),
                    Some(AccessMode::FullAccess),
                    cx,
                );
                assert_eq!(v.settings.model, settings.model);
                assert_eq!(v.settings.access, settings.access);
                v.begin_edit_last_turn(0, window, cx);
                assert!(!v.busy);
                assert_eq!(v.transcript.read(cx).source_rows(), before);
                assert_eq!(v.input.read(cx).content.as_str(), "User draft");
                assert!(v.follow_ups.state.draft.is_none());
                assert!(v.follow_ups.state.messages.is_empty());
                assert!(!v.editing_last_turn.pending());
                v.stop_if_busy(cx);
                assert!(
                    !v.notification_was_cancelled(),
                    "an idle child keeps its completed outcome"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn child_threads_hide_thread_tools_and_wait_for_the_initial_queued_submission(
        cx: &mut TestAppContext,
    ) {
        let window = window(cx);
        window
            .update(cx, |v, _, cx| {
                v.install_transfer(Settings::default(), None, cx);
                assert_eq!(v.provider_environment(&v.settings)["MONOCODE_THREADS"], "1");
                v.managed = true;
                assert_eq!(v.provider_environment(&v.settings)["MONOCODE_THREADS"], "0");
                v.managed = false;
                v.driven_by = Some(42);
                assert_eq!(v.provider_environment(&v.settings)["MONOCODE_THREADS"], "0");
                v.follow_ups.requested = Some("initial-prompt".into());
                assert!(!v.busy);
                assert!(
                    v.thread_pending(),
                    "an accepted prompt must not report idle before its provider starts"
                );
                v.follow_ups.requested = None;
                assert!(!v.thread_pending());
            })
            .unwrap();
    }

    #[gpui::test]
    fn read_only_child_approvals_stay_answerable(cx: &mut TestAppContext) {
        let window = window(cx);
        window
            .update(cx, |v, _, cx| {
                v.driven_by = Some(42);
                let (send, receive) = std::sync::mpsc::channel();
                v.controls = Some(send);
                v.approvals.push_back(monocode_core::agent::Approval {
                    id: "approval".into(),
                    title: "Allow inspection?".into(),
                    detail: String::new(),
                    can_remember: false,
                    remember_scope: None,
                });
                assert_eq!(v.mobile_waiting(cx), Some(mobile::Waiting::Approval));
                assert!(v.decide_notification_request("approval", Decision::Allow, cx));
                let control = receive.try_recv().unwrap();
                assert_eq!(control.id, "approval");
                assert!(matches!(control.decision, Decision::Allow));
                assert!(v.approvals.is_empty());
                assert_eq!(v.driven_by, Some(42));
            })
            .unwrap();
    }
}
