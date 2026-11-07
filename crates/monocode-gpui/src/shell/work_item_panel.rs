//! Work items followed from a thread open in the right panel, so the thread
//! stays on screen. The panel shows an Inbox that has no list of its own, the
//! same view a Mono's detail panel uses.
use super::*;
use crate::{
    inbox::{self, Inbox},
    right_panel::{RightTab, icon_action, pane_header},
};

impl Shell {
    /// Whether a thread is the main view, so a link followed now was followed
    /// from it. The Inbox, Notes, Settings, Search and Monos cover the thread.
    fn thread_in_view(&self, cx: &App) -> bool {
        self.workspace.active.is_some()
            && !self.app_view_open()
            && self.settings.is_none()
            && self.app_search.is_none()
            && !self.mono_active(cx)
    }

    /// The tab's name: "PR #7", "Issue #8", or a Linear identifier.
    fn work_item_label(url: &str) -> Option<SharedString> {
        Some(match monocode_core::inbox::link_target(url)? {
            monocode_core::inbox::LinkTarget::GitHub(item) => {
                let kind = if item.kind == "pr" { "PR" } else { "Issue" };
                format!("{kind} #{}", item.number).into()
            }
            monocode_core::inbox::LinkTarget::Linear(id) => id.into(),
        })
    }

    /// Show `url` in the right panel when a thread is in view. False leaves
    /// the link to the Inbox view or the browser.
    pub(super) fn open_work_item_panel(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(label) = Self::work_item_label(url).filter(|_| self.thread_in_view(cx)) else {
            return false;
        };
        self.ensure_right_panel(window, cx);
        let Some(panel) = self.right_panel.clone() else {
            return false;
        };
        let view = match self.work_item_panel.clone() {
            Some(view) => view,
            None => {
                let projects = self.workspace.projects.iter().map(Into::into).collect();
                let cwd = self.workspace.project.clone().into();
                let store = self.store.clone();
                let view = cx.new(|cx| Inbox::detail(store, projects, cwd, cx));
                self.observations.push(cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &inbox::Event, window, cx| match event {
                        inbox::Event::Close => this.close_work_item_panel(window, cx),
                        inbox::Event::OpenSession(id) => this.open(*id, window, cx),
                        inbox::Event::StartSession { item, prompt } => {
                            this.start_inbox_session(item.as_ref().clone(), prompt.clone(), cx)
                        }
                        inbox::Event::FindAsk(item) => {
                            this.find_pr_ask_session(item.as_ref().clone(), cx)
                        }
                        inbox::Event::AskPr {
                            item,
                            question,
                            prompt,
                            title,
                        } => this.start_pr_ask_session(
                            item.as_ref().clone(),
                            question.clone(),
                            prompt.clone(),
                            title.clone(),
                            cx,
                        ),
                        inbox::Event::ArchiveThreads(ids) => {
                            // Its thread is leaving, and the panel belongs to it.
                            let followed =
                                this.workspace.active.is_some_and(|id| ids.contains(&id));
                            this.archive_linked_threads(ids, cx);
                            if followed {
                                this.close_work_item_panel(window, cx);
                            }
                        }
                        inbox::Event::ItemsChanged(_) => {}
                    },
                ));
                self.observations
                    .push(cx.observe(&view, |_, _, cx| cx.notify()));
                self.work_item_panel = Some(view.clone());
                view
            }
        };
        view.update(cx, |inbox, cx| {
            inbox.set_context(
                self.workspace.projects.iter().map(Into::into).collect(),
                self.workspace.project.clone().into(),
                cx,
            );
            inbox.open_url(url, cx)
        });
        // A panel the link had to open closes again with the work item.
        if panel.read(cx).issue_tab().is_none() {
            self.work_item_opened_panel = !self.right_panel_shown(cx);
        }
        panel.update(cx, |panel, cx| {
            panel.set_issue_tab(Some(label), cx);
            panel.select(RightTab::Issue, cx)
        });
        self.workspace.right_panel_width = self.workspace.right_panel_width.max(420.);
        if self.workspace.right_panel_open {
            self.sync_right_panel(cx);
        } else {
            self.set_right_panel_open(true, window, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
        true
    }

    /// Closing the work item removes its tab. The panel closes with it when
    /// the link opened the panel, or when the project has nothing else to show.
    pub(super) fn close_work_item_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.right_panel.clone() else {
            return;
        };
        let opened = std::mem::take(&mut self.work_item_opened_panel);
        if opened || !self.git.read(cx).available() {
            self.set_right_panel_open(false, window, cx);
        }
        // Back to the tab the panel last showed, which was never the work item.
        let tab = self.workspace.right_panel_tab;
        panel.update(cx, |panel, cx| {
            if panel.tab() == RightTab::Issue {
                panel.select(tab, cx);
            }
            panel.set_issue_tab(None, cx);
        });
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// The panel's content for the Issue tab: a header over the work item.
    pub(super) fn work_item_panel_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.work_item_panel.clone() else {
            return div().into_any_element();
        };
        let item = view.read(cx).shown();
        let title = item.as_ref().map_or_else(
            || "Opening…".to_owned(),
            |item| {
                let kind = if item.kind == "pr" { "PR" } else { "Issue" };
                format!("{kind} #{} · {}", item.number, item.title)
            },
        );
        let url = item.map(|item| item.url);
        div()
            .id("work-item-panel")
            .debug_selector(|| "work-item-panel".into())
            .size_full()
            .flex()
            .flex_col()
            .pt(px(if self.workspace.boxy_layout() {
                TITLE_BAR_HEIGHT
            } else {
                0.
            }))
            .child(
                pane_header("work-item-panel-header", "Work item")
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .when_some(url, |d, url| {
                        d.child(
                            icon_action(
                                "work-item-panel-browser",
                                "Open in browser",
                                "external-link",
                                true,
                            )
                            .cursor_pointer()
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .child(
                        icon_action("work-item-panel-close", "Close", "close", true)
                            .debug_selector(|| "work-item-panel-close".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_work_item_panel(window, cx)
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(view))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::TestAppContext;

    const PULL: &str = "https://github.com/acme/app/pull/7";

    #[gpui::test]
    fn a_link_followed_from_a_thread_opens_beside_it(cx: &mut TestAppContext) {
        // The bounds below are the panel's at rest, not partway through its slide.
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, window, cx| {
                assert!(shell.workspace.active.is_some());
                shell.open_work_link(PULL, window, cx);
                assert!(!shell.app_view_open(), "the thread keeps the main space");
                assert!(shell.workspace.active.is_some());
                assert!(shell.workspace.right_panel_open);
                let panel = shell.right_panel.clone().expect("the panel opened");
                assert_eq!(panel.read(cx).tab(), RightTab::Issue);
                assert_ne!(
                    shell.workspace.right_panel_tab,
                    RightTab::Issue,
                    "a work item is not restored on the next launch"
                );
                let view = shell.work_item_panel.clone().expect("the item's view");
                // Another link replaces the content in the same view.
                shell.open_work_link("https://github.com/acme/app/issues/8", window, cx);
                assert_eq!(shell.work_item_panel.as_ref(), Some(&view));
                assert!(!shell.app_view_open());
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let panel = visual
            .debug_bounds("work-item-panel")
            .expect("panel painted");
        let card = visual
            .debug_bounds("main-pane-frame")
            .expect("card painted");
        assert!(
            panel.left() - card.left() > gpui::px(100.) && panel.right() <= card.right(),
            "the panel takes the card's right edge and leaves room for the thread"
        );
        assert!(visual.debug_bounds("work-item-panel-close").is_some());
        window
            .update(cx, |shell, window, cx| {
                shell.close_work_item_panel(window, cx);
                assert!(!shell.workspace.right_panel_open);
            })
            .unwrap();
    }

    /// Links the active thread to `PULL` and opens that pull request, merged,
    /// in the panel, as following the thread's badge would.
    fn open_merged_pull(window: gpui::WindowHandle<Shell>, cx: &mut TestAppContext) -> i64 {
        let id = window
            .update(cx, |shell, window, cx| {
                let id = shell.workspace.active.unwrap();
                let item = monocode_core::session_groups::parse_github_work_item(PULL).unwrap();
                locked(&shell.store)
                    .unwrap()
                    .set_work_item(id, Some(&item))
                    .unwrap();
                shell.open_work_link(PULL, window, cx);
                let view = shell.work_item_panel.clone().unwrap();
                view.update(cx, |inbox, _| {
                    inbox.pin_for_test(monocode_core::inbox::Item {
                        provider: "github".into(),
                        kind: "pr".into(),
                        state: "merged".into(),
                        repo: "acme/app".into(),
                        number: 7,
                        url: PULL.into(),
                        ..Default::default()
                    })
                });
                id
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
        id
    }

    #[gpui::test]
    fn archiving_a_merged_pull_requests_thread_closes_its_panel(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = open_merged_pull(window, cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let button = visual
            .debug_bounds("inbox-archive-threads")
            .expect("opening the panel finds the thread with no list refresh");
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.session_summaries[&id].archived);
                let saved = locked(&shell.store).unwrap().sessions().unwrap();
                assert!(saved.iter().any(|s| s.id == id && s.archived));
                let panel = shell.right_panel.clone().unwrap();
                assert!(panel.read(cx).issue_tab().is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn archiving_skips_threads_it_would_restore_or_does_not_know(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let id = window
            .update(cx, |shell, _, cx| {
                let id = shell.workspace.active.unwrap();
                shell.archive_linked_threads(&[id, i64::MAX], cx);
                id
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.session_summaries[&id].archived);
                shell.archive_linked_threads(&[id], cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.session_summaries[&id].archived, "stays archived");
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_work_item_is_a_tab_beside_the_others(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.classic_layout = false;
                shell.set_right_panel_open(true, window, cx);
                shell.ensure_right_panel(window, cx);
                let panel = shell.right_panel.clone().unwrap();
                panel.update(cx, |panel, cx| panel.select(RightTab::Graph, cx));
                shell.open_work_link(PULL, window, cx);
                let read = |cx: &App| {
                    let panel = panel.read(cx);
                    (panel.tab(), panel.tabs(), panel.issue_tab().cloned())
                };
                let (tab, tabs, label) = read(cx);
                assert_eq!(tab, RightTab::Issue);
                assert_eq!(tabs.last(), Some(&RightTab::Issue));
                assert_eq!(label.as_deref(), Some("PR #7"));
                // Another tab keeps the work item; opening one replaces it.
                panel.update(cx, |panel, cx| panel.select(RightTab::Files, cx));
                assert_eq!(read(cx).2.as_deref(), Some("PR #7"));
                shell.open_work_link("https://github.com/acme/app/issues/8", window, cx);
                let (tab, _, label) = read(cx);
                assert_eq!(tab, RightTab::Issue);
                assert_eq!(label.as_deref(), Some("Issue #8"));
                // Closing removes the tab and returns to the saved one.
                shell.close_work_item_panel(window, cx);
                let (tab, tabs, label) = read(cx);
                assert_eq!(tab, RightTab::Files);
                assert!(!tabs.contains(&RightTab::Issue) && label.is_none());
                assert!(
                    shell.workspace.right_panel_open,
                    "the panel was already open"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_tab_strip_shows_a_close_button_for_the_open_item(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.classic_layout = false;
                shell.open_work_link(PULL, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(visual.debug_bounds("right-panel-tab-Issue").is_some());
        assert!(visual.debug_bounds("right-panel-issue-close").is_some());
        // The panel is still sliding in, so a click could land off the button.
        // The button's own handler emits this event.
        window
            .update(cx, |shell, _, cx| {
                let panel = shell.right_panel.clone().unwrap();
                panel.update(cx, |_, cx| {
                    cx.emit(crate::right_panel::PanelEvent::CloseIssue)
                });
            })
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                let panel = shell.right_panel.clone().unwrap();
                assert!(panel.read(cx).issue_tab().is_none());
                assert!(!shell.workspace.right_panel_open, "the link opened it");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_project_without_git_still_opens_the_work_item_beside_the_thread(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, window, cx| {
                let id = shell.workspace.active.unwrap();
                let id = shell.workspace.session_for_pane(id);
                shell.sessions[&id].update(cx, |view, _| view.summary.cwd = "~".into());
                shell.sync_git(cx);
                assert!(!shell.git.read(cx).available());
                shell.open_work_link(PULL, window, cx);
                assert!(!shell.app_view_open(), "the thread keeps the main space");
                assert!(shell.workspace.right_panel_open);
                assert!(shell.right_panel_shown(cx));
                shell.close_work_item_panel(window, cx);
                assert!(!shell.workspace.right_panel_open);
                assert!(!shell.right_panel_shown(cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_link_followed_in_the_inbox_stays_in_the_inbox(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.open_inbox(window, cx);
                shell.open_work_link(PULL, window, cx);
                assert_eq!(
                    shell.active_app_view,
                    Some(crate::workspace::AppView::Inbox)
                );
                assert!(shell.work_item_panel.is_none());
            })
            .unwrap();
    }
}
