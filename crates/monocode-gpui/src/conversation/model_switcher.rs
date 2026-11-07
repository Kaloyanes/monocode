//! `App: Switch Model` (Cmd+.) and the model pill's right click: every
//! available model under its provider's header, with a search field on top.
//! Typing narrows the list; Up, Down and Enter move through it without the
//! caret leaving the field.
use super::*;

/// The search row above the provider groups.
pub(super) struct SwitcherSearch {
    input: Entity<input::Input>,
}
impl Render for SwitcherSearch {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(40.))
            .mx(px(-4.))
            .mt(px(-4.))
            .mb(px(4.))
            .px(px(12.))
            .border_b_1()
            .border_color(content(0.1))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(icon("search", 14.).text_color(content(0.45)))
            .child(div().flex_1().min_w_0().child(self.input.clone()))
    }
}

/// One selectable model under a provider's header.
struct Choice {
    id: Option<String>,
    name: SharedString,
    current: bool,
}

/// Claude's models read shorter without the product prefix.
fn short_name(kind: Kind, name: SharedString) -> SharedString {
    match name.strip_prefix("Claude ") {
        Some(short) if kind == Kind::Claude => short.to_owned().into(),
        _ => name,
    }
}

impl Conversation {
    /// The switcher's search field and the subscription that re-filters on
    /// every keystroke.
    pub(super) fn switcher_search(
        &mut self,
        events: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> MenuItem {
        let input =
            cx.new(|cx| input::Input::field("Search models", "Search models…", content(0.4), cx));
        events.push(
            cx.subscribe_in(&input, window, |this, _, _: &input::Changed, _, cx| {
                this.refilter_switcher(cx)
            }),
        );
        let focus = input.read(cx).focus_handle(cx);
        let search = cx.new(|_| SwitcherSearch {
            input: input.clone(),
        });
        self.switcher_input = Some(input);
        let item = MenuItem::Custom(search.into(), focus);
        self.switcher_search_item = Some(item.clone());
        item
    }

    fn refilter_switcher(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self
            .menu
            .as_ref()
            .filter(|m| m.kind == ComposerMenu::Recent)
        else {
            return;
        };
        let menu = open.menu.clone();
        let items = self.recent_model_items(cx);
        let searching = self
            .switcher_input
            .as_ref()
            .is_some_and(|input| !input.read(cx).content.trim().is_empty());
        menu.update(cx, |menu, cx| {
            menu.replace_items(items, cx);
            // A search starts at its best match; clearing it returns to the
            // current model.
            if searching {
                menu.highlight_first(cx);
            } else {
                menu.highlight_checked(cx);
            }
        });
    }

    /// Models for one provider: recent picks first, then the rest of its
    /// catalog, with the session's own model always present.
    fn provider_choices(&self, kind: Kind, cx: &gpui::App) -> Vec<Choice> {
        let provider = Kind::from_id(&self.summary.provider);
        let prefs = cx.try_global::<crate::settings::Preferences>();
        let hidden: Vec<_> = prefs
            .map(|p| p.hidden_models_for(&self.summary.cwd))
            .unwrap_or_default();
        let effective = self.model.as_deref().or(self.resolved_model.as_deref());
        let mut choices: Vec<Choice> = Vec::new();
        let mut push = |choices: &mut Vec<Choice>, id: &str, name: SharedString, default: bool| {
            if id == "default" || choices.iter().any(|c| c.id.as_deref() == Some(id)) {
                return;
            }
            let current = kind == provider
                && match effective {
                    Some(model) => model == id,
                    None => default,
                };
            let hidden = hidden
                .iter()
                .any(|c| c.provider == kind.id() && c.model.as_deref() == Some(id));
            if hidden && !current {
                return;
            }
            choices.push(Choice {
                id: Some(id.to_owned()),
                name: short_name(kind, name),
                current,
            });
        };
        let catalog: Option<Vec<Model>> = if kind == provider {
            (!self.catalog.is_empty()).then(|| self.catalog.clone())
        } else if self.is_remote() {
            None
        } else {
            let account = crate::accounts::get(cx)
                .map(|center| center.read(cx).selected(kind, &self.summary.cwd))
                .unwrap_or_default();
            catalog::cached_models_for(kind, &account)
        };
        for recent in prefs.into_iter().flat_map(|p| &p.recent_models) {
            if recent.provider != kind.id() {
                continue;
            }
            let Some(id) = recent.model.as_deref() else {
                continue;
            };
            let live = catalog.iter().flatten().find(|m| m.id == id);
            let name = match live {
                Some(m) => model_display_name(kind, &m.id, &m.name),
                None => cached_model_name(kind, Some(id)),
            };
            push(&mut choices, id, name, live.is_some_and(|m| m.default));
        }
        for model in catalog.iter().flatten() {
            push(
                &mut choices,
                &model.id,
                model_display_name(kind, &model.id, &model.name),
                model.default,
            );
        }
        if kind == provider {
            match choices.iter_mut().find(|c| c.current) {
                // The label already reflects the live catalog entry.
                Some(current) => current.name = self.model_label(),
                None => choices.insert(
                    0,
                    Choice {
                        id: self.model.clone(),
                        name: self.model_label(),
                        current: true,
                    },
                ),
            }
        }
        choices
    }

    /// The session's provider first, then the rest, each under a header and
    /// narrowed by the search text. A search that names a provider keeps all
    /// of its models.
    pub(super) fn recent_model_items(&self, cx: &gpui::App) -> Vec<MenuItem> {
        let provider = Kind::from_id(&self.summary.provider);
        let hidden_providers = cx
            .try_global::<crate::settings::Preferences>()
            .map(|p| p.hidden_for(&self.summary.cwd))
            .unwrap_or_default();
        let query = self
            .switcher_input
            .as_ref()
            .map(|input| input.read(cx).content.trim().to_lowercase())
            .unwrap_or_default();
        let mut items: Vec<MenuItem> = Vec::new();
        if let Some(search) = &self.switcher_search_item {
            items.push(search.clone());
        }
        let mut shown = false;
        let kinds =
            std::iter::once(provider).chain(Kind::ALL.into_iter().filter(|k| *k != provider));
        for kind in kinds {
            if kind != provider && hidden_providers.iter().any(|id| id == kind.id()) {
                continue;
            }
            let by_provider = !query.is_empty() && kind.label().to_lowercase().contains(&query);
            let choices: Vec<_> = self
                .provider_choices(kind, cx)
                .into_iter()
                .filter(|c| {
                    query.is_empty() || by_provider || c.name.to_lowercase().contains(&query)
                })
                .collect();
            if choices.is_empty() {
                continue;
            }
            shown = true;
            items.push(MenuItem::Header(kind.label().into()));
            for choice in choices {
                let id = serde_json::to_string(&(kind.id(), &choice.id)).unwrap_or_default();
                items.push(
                    MenuAction::new(format!("recent-model:{id}"), choice.name)
                        .image(kind.id())
                        .checked(choice.current)
                        .into(),
                );
            }
        }
        if !shown {
            items.push(MenuItem::Note("No matching models".into()));
        }
        items
    }
}
