use super::*;

impl Conversation {
    /// Whether an agent may give this thread `model` and `effort`, judged
    /// against the model it would end up on.
    pub(crate) fn check_agent_change(
        &self,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), String> {
        if self.driven_by.is_some() {
            return Err("A subagent follows its lead; change the lead instead.".into());
        }
        let catalog = self.agent_models();
        let info = match model {
            Some(id) => Some(catalog::find_model(&catalog, id).ok_or_else(|| {
                let ids: Vec<_> = catalog.iter().take(12).map(|m| m.id.as_str()).collect();
                format!("Unknown model for this harness. Try: {}", ids.join(", "))
            })?),
            None => self.model_info(),
        };
        match (effort, info) {
            (Some(effort), Some(info))
                if !info.efforts.is_empty() && !info.efforts.iter().any(|e| e == effort) =>
            {
                Err(format!(
                    "Unknown effort for this model. Try: {}",
                    info.efforts.join(", ")
                ))
            }
            _ => Ok(()),
        }
    }

    /// The models an agent can pick from, before this thread's list has
    /// loaded too.
    fn agent_models(&self) -> Vec<catalog::Model> {
        if self.catalog.is_empty() {
            let kind = Kind::from_id(&self.summary.provider);
            catalog::cached_models_for(kind, &self.settings.account).unwrap_or_default()
        } else {
            self.catalog.clone()
        }
    }

    /// An agent's change to this thread, already checked. Unlike the pickers,
    /// it leaves the person's defaults for new chats alone.
    pub(crate) fn apply_agent_change(
        &mut self,
        model: Option<&str>,
        effort: Option<&str>,
        access: Option<AccessMode>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(model) = model {
            let kind = Kind::from_id(&self.summary.provider);
            let catalog = self.agent_models();
            let model = catalog::find_model(&catalog, model).map_or(model, |m| m.id.as_str());
            if !self.apply_model_pick(&ModelPicked(kind, Some(model.to_owned())), cx) {
                return Err("This thread can't change model right now.".into());
            }
        }
        if let Some(effort) = effort {
            self.apply_effort(effort);
        }
        if let Some(access) = access {
            self.settings.access = access;
        }
        self.save_settings(cx);
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
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
    /// Tool calls in the subagent's current turn.
    pub(crate) fn turn_steps(&self, cx: &gpui::App) -> usize {
        let rows = self.transcript.read(cx).source_rows();
        let turn = rows
            .iter()
            .rposition(|row| row.first && row.role == "user")
            .unwrap_or(0);
        rows[turn..]
            .iter()
            .filter(|row| row.first && row.role == "tool")
            .count()
    }
    #[cfg(test)]
    pub(crate) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }
    /// How a stop lands once the provider's turn has ended.
    #[cfg(test)]
    pub(crate) fn end_stopped_turn(&mut self) {
        self.stopping = false;
        self.notification_cancelled = true;
    }
    /// The tool call this thread ran last in its current turn.
    pub(crate) fn turn_activity(&self, cx: &gpui::App) -> Option<String> {
        let rows = self.transcript.read(cx).source_rows();
        let turn = rows
            .iter()
            .rposition(|row| row.first && row.role == "user")
            .unwrap_or(0);
        let row = rows[turn..].iter().rev().find(|row| row.role == "tool")?;
        let tool = serde_json::from_str::<monocode_core::tool::ToolCall>(&row.text).ok()?;
        let shown = tool.presentation();
        Some(match shown.file_path {
            Some(path) => format!("{} {path}", shown.action),
            None => shown.action,
        })
    }
    /// Every file this thread's edit tools changed, in any turn.
    pub(crate) fn edited_files(&self, cx: &gpui::App) -> Vec<String> {
        crate::runtime::edited_paths(self.transcript.read(cx).source_rows())
    }
    /// Restoring an evicted history is housekeeping, not work: the thread
    /// stays settled unless an action is waiting on the restore.
    fn restoring_history(&self) -> bool {
        self.loading
            && self.history_unloaded
            && self.history_actions.is_empty()
            && !self.busy
            && !self.stopping
    }
    /// Whether a turn is really under way. Opening a thread loads its history
    /// and reads as pending, sometimes with work deferred, but nothing runs.
    pub(crate) fn thread_turn_active(&self) -> bool {
        self.busy || self.follow_ups.requested.is_some() || self.follow_ups.sending.is_some()
    }
    pub(crate) fn thread_pending(&self) -> bool {
        (self.external_submission_pending() && !self.restoring_history())
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
    /// Starts a turn with a message another thread sent, recording that
    /// thread as the turn's sender.
    pub(crate) fn send_thread_message(
        &mut self,
        prompt: String,
        sender: i64,
        cx: &mut Context<Self>,
    ) {
        if !self.history_ready() {
            self.defer_history_action(
                move |this, cx| this.send_thread_message(prompt, sender, cx),
                cx,
            );
            return;
        }
        self.thread_sender = Some(sender);
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
        // Lists the device tools when the user lets agents drive devices.
        environment.insert(
            monocode_core::device::ENV.into(),
            if monocode_core::device::agents_enabled() && !self.is_remote() {
                "1"
            } else {
                "0"
            }
            .into(),
        );
        environment
    }
    /// Where a subagent's composer would be: who drives it, and Stop.
    /// Its approvals still show above this, so the user can answer them.
    pub(super) fn thread_composer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let lead = if self.thread_lead_title.is_empty() {
            "its lead".to_owned()
        } else {
            self.thread_lead_title.clone()
        };
        div()
            .id("thread-locked-composer")
            .debug_selector(|| "thread-locked-composer".into())
            .role(gpui::Role::Group)
            .aria_label("Subagent")
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2p5()
            .border_t_1()
            .border_color(content(0.08))
            .child(icon("lock", 14.).text_color(content(0.5)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .child(format!(
                        "Subagent of {lead}. Its replies go back to the lead."
                    )),
            )
            .when(self.busy, |d| {
                d.child(
                    crate::settings::widgets::secondary_button("thread-stop", "Stop", true, false)
                        .child("Stop")
                        .on_click(cx.listener(|this, _, _, cx| this.stop_app_session(cx))),
                )
            })
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
                let _ = v.mobile_submit("Mobile submit".into(), None, Vec::new(), cx);
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
    fn composer_modes_survive_a_deferred_launch(cx: &mut TestAppContext) {
        let window = window(cx);
        window
            .update(cx, |v, _, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.settings.plan = true;
                let modes = v.turn_modes();
                // Send resets the composer while compaction makes the chat busy.
                v.clear_submitted_mode(cx);
                let seen = v.with_modes(modes, |v| v.settings.plan);
                assert!(seen, "the deferred turn runs in its submitted mode");
                assert!(!v.settings.plan, "the composer keeps its own");
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
