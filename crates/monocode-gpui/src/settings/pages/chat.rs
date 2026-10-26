//! ChatPage.
use crate::{
    pickers::{ModelList, ModelPicked, model_name},
    popover::{
        Align, Anchor, AnchorSlot, Flyout, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle,
        menu_popover,
    },
    settings::{ModelChoice, Preferences, SettingsView},
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, Role, SharedString, Subscription, Window, div, prelude::*, px,
};
use monocode_core::provider::Kind;

/// The open Text generation model menu, under the trigger it belongs to.
pub(in crate::settings) struct TextModelMenu {
    id: &'static str,
    menu: Entity<Menu>,
    anchor: AnchorSlot,
    _events: Vec<Subscription>,
}

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
    /// The composer's model menu: a "Default" row, then the "Model" row
    /// whose flyout is the composer's ModelList.
    fn text_model_select(
        &self,
        id: &'static str,
        label: &'static str,
        current: &Option<ModelChoice>,
        none: &'static str,
        cx: &mut Context<Self>,
        set: fn(&mut Preferences, Option<ModelChoice>),
    ) -> AnyElement {
        let open = self.text_model_menu.as_ref().is_some_and(|m| m.id == id);
        let slot: AnchorSlot = self
            .select_anchors
            .borrow_mut()
            .entry(id)
            .or_default()
            .clone();
        let picked = current
            .as_ref()
            .map(|c| (Kind::from_id(&c.provider), c.model.clone()));
        let trigger_label = picked
            .as_ref()
            .map_or(SharedString::from(none), |(kind, model)| {
                model_name(*kind, model.as_deref())
            });
        let current = current.clone();
        div()
            .id(SharedString::from(format!("select-{id}")))
            .debug_selector(move || format!("settings-select-{id}"))
            .role(Role::ComboBox)
            .aria_label(format!("{label}: {trigger_label}"))
            .aria_expanded(open)
            .relative()
            .w(px(240.))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.05))
            .px_2()
            .py_1()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(1.))
            .cursor_pointer()
            .hover(|s| s.border_color(content(0.2)))
            .child(slot.probe())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when_some(picked.as_ref(), |d, (kind, _)| {
                        d.child(provider_icon(kind.id(), 16.).flex_shrink_0())
                    })
                    .child(div().min_w_0().truncate().child(trigger_label)),
            )
            .child(
                icon("chevron-down-hi", 14.)
                    .text_color(content(0.5))
                    .when(open, |s| {
                        s.with_transformation(gpui::Transformation::rotate(gpui::percentage(0.5)))
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.text_model_menu.take().is_none() {
                    this.open_text_model_menu(
                        id,
                        slot.clone(),
                        none,
                        current.clone(),
                        set,
                        window,
                        cx,
                    );
                }
                cx.notify();
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn open_text_model_menu(
        &mut self,
        id: &'static str,
        anchor: AnchorSlot,
        none: &'static str,
        current: Option<ModelChoice>,
        set: fn(&mut Preferences, Option<ModelChoice>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_open = None;
        let project = self.ctx.cwd.clone();
        let provider = current
            .as_ref()
            .map(|c| Kind::from_id(&c.provider))
            .or_else(|| self.prefs.default_provider.as_deref().and_then(Kind::parse))
            .unwrap_or(Kind::ALL[0]);
        let model = current.as_ref().and_then(|c| c.model.clone());
        let profile = crate::accounts::get(cx)
            .map(|a| a.read(cx).selected(provider, &project))
            .unwrap_or_default();
        let hidden = self.prefs.hidden_for(&project);
        let list = cx.new(|cx| {
            ModelList::new_for(provider, model.clone(), &project, profile, cx)
                .hide_providers(hidden)
                .allow_provider_switch(true)
        });
        let picked = cx.subscribe_in(
            &list,
            window,
            move |this, _, picked: &ModelPicked, window, cx| {
                let choice = ModelChoice {
                    provider: picked.0.id().into(),
                    model: picked.1.clone(),
                };
                this.update(cx, |p| set(p, Some(choice)));
                this.close_text_model_menu(window, cx);
            },
        );
        let focus = list.read(cx).search_focus(cx);
        let items = vec![
            MenuAction::new("default", none)
                .checked(current.is_none())
                .into(),
            MenuItem::Separator,
            MenuAction::new("model", "Model")
                .value_image(provider.id())
                .value(if current.is_some() {
                    model_name(provider, model.as_deref())
                } else {
                    SharedString::from("Choose")
                })
                .flyout(Flyout {
                    view: list.into(),
                    width: 310.,
                    padding: 0.,
                    height: Some(442.),
                    max_height: None,
                    focus: Some(focus),
                    surface: None,
                })
                .into(),
        ];
        let menu = cx.new(|cx| {
            Menu::new("Model and settings", items, window, cx)
                .width(250.)
                .style(MenuStyle::Settings)
        });
        let events = cx.subscribe_in(
            &menu,
            window,
            move |this, _, event: &MenuEvent, window, cx| match event {
                MenuEvent::Picked(choice, _) if choice.as_ref() == "default" => {
                    this.update(cx, |p| set(p, None));
                    this.close_text_model_menu(window, cx);
                }
                MenuEvent::Picked(..) => {}
                MenuEvent::Dismissed => this.close_text_model_menu(window, cx),
            },
        );
        self.text_model_menu = Some(TextModelMenu {
            id,
            menu,
            anchor,
            _events: vec![picked, events],
        });
    }

    pub(in crate::settings) fn close_text_model_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.text_model_menu.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    pub(in crate::settings) fn text_model_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.text_model_menu.as_ref()?;
        Some(
            menu_popover(&open.menu, Anchor::Slot(open.anchor.clone()), cx)
                .align(Align::End)
                .gap(4.)
                .into_any_element(),
        )
    }
}
