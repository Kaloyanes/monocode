//! The sidebar's Working and Archived sections: where they sit, and the
//! archive's view of the list.
use super::*;
use gpui::{Div, Stateful};

/// How many archived sessions the docked section lists before "Show all".
const DOCKED_ARCHIVE_CAP: usize = 30;

/// A section's clickable header; `chevron` leads when the section folds.
pub(super) fn section_header(
    id: &'static str,
    label: &'static str,
    count: usize,
    chevron: Option<&'static str>,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(Role::Button)
        .aria_label(format!("{label}, {count}"))
        .cursor_pointer()
        .h(px(28.))
        .px_2()
        .flex_shrink_0()
        .rounded_md()
        .flex()
        .items_center()
        .gap_1p5()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.5))
        .hover(|s| s.bg(content(0.06)))
        .children(chevron.map(|path| icon(path, 12.)))
}

fn header_label(label: &'static str, count: usize) -> [AnyElement; 2] {
    [
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .child(label)
            .into_any_element(),
        div()
            .flex_shrink_0()
            .text_size(px(11.))
            .text_color(content(0.4))
            .child(count.to_string())
            .into_any_element(),
    ]
}

impl Shell {
    pub(super) fn archive_view_open(&self) -> bool {
        self.archive_view.as_deref() == Some(self.workspace.project.as_str())
    }

    fn docked_sections(&self) -> bool {
        self.workspace.preferences.docked_sections
    }

    /// Working leads this page's list, rather than sitting in the dock.
    pub(super) fn working_in_list(&self) -> bool {
        !self.docked_sections() && !self.sessions_in_working().is_empty()
    }

    /// The sessions Working shows in place of their dated rows. A search or
    /// the archive looks at the whole list, so neither gives any up.
    pub(super) fn sessions_in_working(&self) -> HashSet<i64> {
        if self.archive_view_open() || !self.query.is_empty() {
            return HashSet::new();
        }
        self.sidebar_list.live.iter().copied().collect()
    }

    pub(super) fn refresh_archived_sessions(&mut self, cx: &App) {
        let mut archived = self.listed_sessions(true, cx);
        archived.sort_by_cached_key(|id| std::cmp::Reverse(self.archived_at(*id, cx)));
        self.sidebar_list.archived = archived;
        // Restoring the last archived session leaves nothing to look at.
        if self.archive_view_open()
            && self.sidebar_list.archived.is_empty()
            && self.query.is_empty()
            && !self.space_swipe.drawing_neighbour()
        {
            self.archive_view = None;
        }
    }

    /// The archive's search belongs to the project it opened in.
    pub(super) fn leave_stale_archive_view(&mut self, cx: &mut Context<Self>) {
        if self.archive_view.is_some()
            && !self.archive_view_open()
            && !self.space_swipe.drawing_neighbour()
        {
            self.archive_view = None;
            let shell = cx.entity();
            cx.defer(move |cx| shell.update(cx, |this, cx| this.clear_session_search(cx)));
        }
    }

    fn open_archive_view(&mut self, cx: &mut Context<Self>) {
        self.archive_view = Some(self.workspace.project.clone());
        self.archive_dock_open = false;
        self.close_preview(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    fn close_archive_view(&mut self, cx: &mut Context<Self>) {
        self.archive_view = None;
        self.clear_session_search(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    fn clear_session_search(&mut self, cx: &mut Context<Self>) {
        if !self.classic_session_search.read(cx).content.is_empty() {
            self.classic_session_search
                .update(cx, |input, cx| input.set_text(String::new(), cx));
        }
    }

    /// Replaces the sidebar's top while the archive has the list. Classic
    /// already has its search bar there.
    pub(super) fn archive_view_header(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.archive_view_open() {
            return None;
        }
        let count = self.sidebar_list.archived.len();
        let [label, total] = header_label("Archived", count);
        let back = section_header(
            "archive-view-back",
            "Back from Archived",
            count,
            Some("chevron-left"),
        )
        .text_color(content(1.))
        .child(label)
        .child(total)
        .on_click(cx.listener(|this, _, _, cx| this.close_archive_view(cx)));
        let classic = self.workspace.classic_layout;
        Some(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(if classic { 6. } else { 8. }))
                        .pt_1()
                        .child(back),
                )
                .when(!classic, |d| d.child(self.classic_conversation_search(cx)))
                .into_any_element(),
        )
    }

    pub(super) fn docked_live_agents(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.docked_sections()
            .then(|| self.live_agents_preview(cx))
            .flatten()
    }

    /// What sits below the list. `working` is false where the project rail
    /// holds the docked Working section instead.
    pub(super) fn sidebar_dock(
        &mut self,
        working: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let live = working.then(|| self.docked_live_agents(cx)).flatten();
        let archived = self.archived_section(cx);
        if live.is_none() && archived.is_none() {
            return None;
        }
        Some(
            div()
                .id("sidebar-dock")
                .debug_selector(|| "sidebar-dock".into())
                .flex_shrink_0()
                .py_1()
                .border_t_1()
                .border_color(content(0.06))
                .when(self.workspace.boxy_layout(), |d| d.pb_2())
                .children(live)
                .children(archived)
                .into_any_element(),
        )
    }

    fn archived_section(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.sidebar_list.archived.len();
        if count == 0 || self.archive_view_open() {
            return None;
        }
        let [label, total] = header_label("Archived", count);
        let docked = self.docked_sections();
        let open = docked && self.archive_dock_open;
        let header = if docked {
            section_header(
                "sidebar-archived",
                "Archived",
                count,
                Some(if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                }),
            )
            .aria_expanded(open)
            .child(label)
            .child(total)
            .on_click(cx.listener(|this, _, _, cx| {
                this.archive_dock_open = !this.archive_dock_open;
                this.notify_sidebar(cx);
            }))
        } else {
            section_header("sidebar-archived", "Archived", count, None)
                .child(label)
                .child(total)
                .child(icon("chevron-right", 12.))
                .on_click(cx.listener(|this, _, _, cx| this.open_archive_view(cx)))
        };
        let ids: Vec<_> = if open {
            self.sidebar_list
                .archived
                .iter()
                .copied()
                .take(DOCKED_ARCHIVE_CAP)
                .collect()
        } else {
            Vec::new()
        };
        let cards: Vec<_> = ids.iter().map(|id| self.session_card(*id, cx)).collect();
        Some(
            div()
                .px_2()
                .flex()
                .flex_col()
                .child(header)
                .when(open, |d| {
                    d.child(
                        div()
                            .id("sidebar-archived-list")
                            .debug_selector(|| "sidebar-archived-list".into())
                            .max_h(px(280.))
                            .overflow_y_scroll()
                            .children(cards),
                    )
                })
                .when(open && count > DOCKED_ARCHIVE_CAP, |d| {
                    d.child(
                        section_header("sidebar-archived-all", "Show all archived", count, None)
                            .justify_center()
                            .child(format!("Show all {count}"))
                            .on_click(cx.listener(|this, _, _, cx| this.open_archive_view(cx))),
                    )
                })
                .into_any_element(),
        )
    }
}
