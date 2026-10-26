//! ChatPage.
use crate::settings::{ModelChoice, Preferences, SettingsView};
use gpui::{AnyElement, Context, SharedString};

impl SettingsView {
    pub(super) fn chat_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
        let transcript = vec![
            self.row(
                Some("transcript-layout"),
                true,
                "Transcript layout",
                Some("Show prompts at full width or as right-aligned chat bubbles.".into()),
                vec![self.segmented(
                    "transcript-layout",
                    "Transcript layout",
                    &p.transcript_layout,
                    &[("full", "Full width"), ("chat", "Chat")],
                    cx,
                    |this, v, cx| this.update(cx, |p| p.transcript_layout = v.into()),
                )],
            ),
            self.row(
                Some("anchor-prompts"),
                false,
                "Anchor prompts to top",
                Some("Place new prompts at the top, with replies below.".into()),
                vec![self.toggle(
                    "anchor-prompts",
                    "Anchor prompts to top",
                    p.anchor_prompts,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.anchor_prompts = on),
                )],
            ),
            self.row(
                Some("paced-reveal"),
                false,
                "Smooth text reveal",
                Some("Reveal incoming text gradually.".into()),
                vec![self.toggle(
                    "paced-reveal",
                    "Smooth text reveal",
                    p.paced_reveal,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.paced_reveal = on),
                )],
            ),
        ];
        let composer = vec![
            self.row(
                Some("follow-up"),
                true,
                "Follow-up behavior",
                Some("Queue follow-ups until the active turn finishes, or steer the active turn immediately.".into()),
                vec![self.segmented(
                    "follow-up",
                    "Follow-up behavior",
                    &p.follow_up,
                    &[("queue", "Queue"), ("steer", "Steer")],
                    cx,
                    |this, v, cx| this.update(cx, |p| p.follow_up = v.into()),
                )],
            ),
            self.row(
                Some("model-controls"),
                false,
                "Model controls",
                Some("Show model options beside the picker instead of inside the model menu.".into()),
                vec![self.segmented(
                    "model-controls",
                    "Model controls",
                    &p.model_controls,
                    &[("menu", "Menu"), ("beside", "Beside")],
                    cx,
                    |this, v, cx| this.update(cx, |p| p.model_controls = v.into()),
                )],
            ),
            self.row(
                Some("composer-autocorrect"),
                false,
                "Autocorrect",
                Some("Spell check and autocorrect prompts.".into()),
                vec![self.toggle(
                    "composer-autocorrect",
                    "Autocorrect",
                    p.autocorrect,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.autocorrect = on),
                )],
            ),
        ];
        let generated = vec![
            self.row(
                Some("text-model"),
                true,
                "Text generation model",
                Some("Write commit messages, pull request text and session titles with this model instead of the default.".into()),
                vec![self.text_model_select(
                    "text-model",
                    "Text generation model",
                    &p.text_model,
                    "Default",
                    cx,
                    |p, choice| p.text_model = choice,
                )],
            ),
            self.row(
                Some("commit-model"),
                false,
                "Commit messages",
                Some("Use a different model for commit messages.".into()),
                vec![self.text_model_select(
                    "commit-model",
                    "Commit message model",
                    &p.commit_model,
                    "Same as above",
                    cx,
                    |p, choice| p.commit_model = choice,
                )],
            ),
            self.row(
                Some("pr-model"),
                false,
                "Pull requests",
                Some("Use a different model for pull request titles and descriptions.".into()),
                vec![self.text_model_select(
                    "pr-model",
                    "Pull request model",
                    &p.pr_model,
                    "Same as above",
                    cx,
                    |p, choice| p.pr_model = choice,
                )],
            ),
            self.row(
                Some("title-model"),
                false,
                "Session titles",
                Some("Use a different model for naming sessions.".into()),
                vec![self.text_model_select(
                    "title-model",
                    "Session title model",
                    &p.title_model,
                    "Same as above",
                    cx,
                    |p, choice| p.title_model = choice,
                )],
            ),
        ];
        let editor = vec![self.row(
            Some("format-on-save"),
            true,
            "Format on save",
            Some("Format supported files with Prettier before saving.".into()),
            vec![self.toggle(
                "format-on-save",
                "Format on save",
                p.format_on_save,
                false,
                cx,
                |this, on, cx| this.update(cx, |p| p.format_on_save = on),
            )],
        )];
        let review = vec![self.row(
            Some("diff-view"),
            true,
            "Diff view",
            Some("Review changes in the editor or in a combined diff.".into()),
            vec![self.segmented(
                "diff-view",
                "Diff view",
                &p.diff_view,
                &[("editor", "Editor"), ("unified", "Unified")],
                cx,
                |this, v, cx| this.update(cx, |p| p.diff_view = v.into()),
            )],
        )];
        let extras = vec![
            self.row(
                Some("composer-mascot"),
                true,
                "Composer mascot",
                Some("Show the project mascot while an agent works.".into()),
                vec![self.toggle(
                    "composer-mascot",
                    "Composer mascot",
                    p.composer_mascot,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.composer_mascot = on),
                )],
            ),
            self.row(
                Some("empty-session-games"),
                false,
                "Empty session games",
                Some("Show playable games in empty sessions.".into()),
                vec![self.toggle(
                    "empty-session-games",
                    "Empty session games",
                    p.empty_session_games,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.empty_session_games = on),
                )],
            ),
        ];
        vec![
            self.group(None, true, "Transcript", None, None, transcript),
            self.group(None, false, "Composer", None, None, composer),
            self.group(None, false, "Generated text", None, None, generated),
            self.group(None, false, "Editor", None, None, editor),
            self.group(None, false, "Code review", None, None, review),
            self.group(None, false, "Extras", None, None, extras),
        ]
    }
}

impl SettingsView {
    /// A select over every provider's models, with `none` for no choice.
    fn text_model_select(
        &self,
        id: &'static str,
        label: &'static str,
        current: &Option<ModelChoice>,
        none: &'static str,
        cx: &mut Context<Self>,
        set: fn(&mut Preferences, Option<ModelChoice>),
    ) -> AnyElement {
        let value = current
            .as_ref()
            .map(|c| format!("{}:{}", c.provider, c.model.as_deref().unwrap_or_default()))
            .unwrap_or_default();
        let mut options: Vec<(String, SharedString, Option<AnyElement>)> =
            vec![(String::new(), none.into(), None)];
        // The model picker's own rules: installed CLIs, and the providers and
        // models the Providers page leaves visible.
        for kind in monocode_core::provider::Kind::ALL {
            let installed = self
                .discovery
                .binaries
                .iter()
                .any(|(k, path)| *k == kind && path.is_some());
            if !installed || self.prefs.hidden_providers.iter().any(|id| id == kind.id()) {
                continue;
            }
            let models = self
                .ctx
                .catalogs
                .get(kind.id())
                .filter(|models| !models.is_empty())
                .or_else(|| self.model_catalogs.get(&kind));
            for (model, name) in models.into_iter().flatten() {
                let hidden = self
                    .prefs
                    .hidden_models
                    .iter()
                    .any(|c| c.provider == kind.id() && c.model.as_deref() == Some(model));
                if hidden {
                    continue;
                }
                options.push((
                    format!("{}:{model}", kind.id()),
                    format!("{} · {name}", kind.label()).into(),
                    None,
                ));
            }
        }
        self.select(
            id,
            label,
            &value,
            options,
            Some(240.),
            cx,
            move |this, v, cx| {
                let choice = v.split_once(':').map(|(provider, model)| ModelChoice {
                    provider: provider.into(),
                    model: (!model.is_empty()).then(|| model.into()),
                });
                this.update(cx, |p| set(p, choice))
            },
        )
    }
}
