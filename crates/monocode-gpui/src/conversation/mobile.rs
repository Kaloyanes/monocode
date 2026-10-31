//! A paired phone's view of and commands for this chat, through the same paths
//! the composer and its pickers use.
use super::*;
use crate::notifications::approvals::InputKind;

/// What a chat waits on the user for, so the phone can say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    Approval,
    Question,
}

impl Conversation {
    /// A phone's message, as if typed: it starts a turn, or while one runs it
    /// steers or queues. The desktop composer's own draft and files stay put.
    /// `mode` is the phone's Plan, Operator or Orchestrator pick; without one,
    /// a leading `/plan`, `/operator` or `/orchestrator` in the text picks it.
    pub(crate) fn mobile_submit(
        &mut self,
        prompt: String,
        mode: Option<&'static str>,
        attachments: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.driven_by.is_some() {
            return Ok(());
        }
        let (mode, prompt) = match mode {
            Some(mode) => (Some(mode), prompt),
            None => split_mode_command(prompt),
        };
        if self.is_remote() && matches!(mode, Some("operator" | "orchestrator")) {
            return Err("This mode is not available on remote projects.".into());
        }
        if !self.history_ready() {
            self.defer_history_action(
                move |this, cx| {
                    let _ = this.mobile_submit(prompt, mode, attachments, cx);
                },
                cx,
            );
            return Ok(());
        }
        let composer_busy = self.attaching
            || !self.input.read(cx).content.is_empty()
            || !self.attachments.is_empty();
        if self.busy
            || self.stopping
            || self.preparing_worktree
            || composer_busy
            || self.editing_last_turn.pending()
            || self.settings.pending_provider_switch.is_some()
            || (!self.follow_ups.state.messages.is_empty() && !self.follow_ups.state.paused)
        {
            let prompt = if prompt.is_empty() && mode == Some("operator") {
                composer_features::OPERATOR_INTRO.into()
            } else {
                prompt
            };
            let composer = std::mem::replace(&mut self.attachments, attachments);
            self.enqueue_follow_up(prompt, mode.map(str::to_owned), cx);
            self.attachments = composer;
            if !self.busy {
                self.pump_follow_ups(cx);
            }
            cx.emit(SessionChanged);
            cx.notify();
            return Ok(());
        }
        self.attachments = attachments;
        // Sent as the composer's mode command, so it starts the turn in that
        // mode exactly as a typed one does.
        let text = match mode {
            Some(mode) => format!("/{mode} {prompt}"),
            None => prompt,
        };
        self.input.update(cx, |i, cx| i.set_text(text, cx));
        self.send(cx);
        cx.emit(SessionChanged);
        Ok(())
    }

    /// The phone's model and access sheets. A provider switch is armed as the
    /// desktop picker arms it, and hands off on the next send.
    pub(crate) fn mobile_configure(
        &mut self,
        model: Option<(Kind, Option<String>)>,
        model_settings: &std::collections::HashMap<String, String>,
        access: Option<AccessMode>,
        cx: &mut Context<Self>,
    ) {
        if self.driven_by.is_some() {
            return;
        }
        if let Some((kind, model)) = model
            && (self.summary.provider != kind.id() || self.model != model)
        {
            let picked = ModelPicked(kind, model);
            if self.apply_model_pick(&picked, cx) {
                remember_model(kind, picked.1, cx);
            }
        }
        for (key, value) in model_settings {
            match key.as_str() {
                "fast" => self.menu_choice(
                    if value == "true" {
                        "fast:on"
                    } else {
                        "fast:off"
                    },
                    cx,
                ),
                "thinking" => {
                    self.settings.thinking = Some(value == "true");
                    self.save_settings(cx);
                }
                key if is_effort_key(key) => self.menu_choice(&format!("effort:{value}"), cx),
                _ => {
                    if let Ok(choice) = serde_json::to_string(&(key, value)) {
                        self.menu_choice(&format!("model-setting:{choice}"), cx);
                    }
                }
            }
        }
        if let Some(access) = access {
            self.menu_choice(&format!("access:{}", access.id()), cx);
        }
        cx.emit(SessionChanged);
        cx.notify();
    }

    /// The current model id, as a picker would select it.
    pub(crate) fn mobile_model(&self) -> Option<&str> {
        self.model.as_deref().or(self.resolved_model.as_deref())
    }

    /// The model sheet's settings as the phone names them.
    pub(crate) fn mobile_model_settings(&self) -> std::collections::HashMap<String, String> {
        let mut values = self.settings.model_settings.clone();
        if let Some(effort) = &self.settings.effort {
            values.insert("effort".into(), effort.clone());
        }
        values.insert("fast".into(), self.settings.fast.to_string());
        if let Some(thinking) = self.settings.thinking {
            values.insert("thinking".into(), thinking.to_string());
        }
        values
    }

    /// The approvals and questions waiting on the desktop, for the phone's
    /// transcript; saved rows never record them.
    pub(crate) fn mobile_pending(&self, cx: &gpui::App) -> Vec<crate::mobile::transcript::Pending> {
        self.pending_notification_inputs(cx)
            .into_iter()
            .map(|input| crate::mobile::transcript::Pending {
                id: input.request_id,
                question: input.kind == InputKind::Question,
                label: input.label,
            })
            .collect()
    }

    pub(crate) fn mobile_waiting(&self, cx: &gpui::App) -> Option<Waiting> {
        let inputs = self.pending_notification_inputs(cx);
        if inputs.iter().any(|input| input.kind == InputKind::Approval) {
            Some(Waiting::Approval)
        } else if inputs.iter().any(|input| input.kind == InputKind::Question) {
            Some(Waiting::Question)
        } else {
            None
        }
    }
}

/// The permission modes a phone offers, with the desktop picker's hints.
pub(crate) fn mobile_access_modes()
-> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    ACCESS_MODES
        .iter()
        .map(|(id, label, hint, _)| (*id, *label, *hint))
}

/// The composer modes a phone can send a message in.
pub(crate) const MOBILE_MODES: [&str; 3] = ["plan", "operator", "orchestrator"];

/// A message's leading mode command, and the message without it.
fn split_mode_command(prompt: String) -> (Option<&'static str>, String) {
    MOBILE_MODES
        .into_iter()
        .find_map(|mode| {
            monocode_core::composer::consume_command(&prompt, mode)
                .map(|rest| (Some(mode), rest.to_owned()))
        })
        .unwrap_or((None, prompt))
}

#[cfg(test)]
mod tests {
    use super::split_mode_command;

    #[test]
    fn a_leading_mode_command_picks_the_mode() {
        assert_eq!(
            split_mode_command("/Operator  open the logs".into()),
            (Some("operator"), "open the logs".into())
        );
        assert_eq!(
            split_mode_command("/orchestrator".into()),
            (Some("orchestrator"), String::new())
        );
        assert_eq!(
            split_mode_command("/planet is not a mode".into()),
            (None, "/planet is not a mode".into())
        );
    }
}
