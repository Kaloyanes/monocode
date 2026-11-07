use crate::{
    settings::{
        SettingsEvent, SettingsView,
        data::Tree,
        management::Target,
        widgets::{note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, Div, SharedString, Stateful, div, prelude::*, px};

/// A 16px box that shows a tick when `on`; `disabled` dims it and ignores clicks.
fn checkbox(
    id: SharedString,
    label: impl Into<SharedString>,
    on: bool,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::CheckBox)
        .aria_label(label.into())
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .mt(px(1.))
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(if on { accent_color() } else { content(0.2) })
        .when(on, |d| {
            d.bg(accent_color())
                .child(icon("check", 12.).text_color(gpui::rgb(0x06121f)))
        })
        .when(disabled, |d| d.opacity(0.3))
        .when(!disabled, |d| d.cursor_pointer())
}
impl SettingsView {
    pub(super) fn worktrees_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let project = self.project_picker(cx);
        // The list stays up while Git re-reads it, so only Create waits.
        let loading = self.worktrees_loading;
        let ready =
            self.discovery.worktrees.error.is_none() && !self.discovery.worktrees.main.is_empty();
        let mut page=vec![div().relative().child(self.reveal_probe("project-worktrees")).flex().items_center().gap_1().child(project).child(secondary_button("create-settings-worktree","Create worktree",false,!ready||loading).child(icon("plus",12.)).child("Create worktree").on_click(cx.listener(move|this,_,window,cx|{if ready&&!this.worktrees_loading{this.open_management(Target::Worktree,window,cx);}}))).into_any_element(),div().mt_4().flex().items_center().justify_between().gap_3().child(div().min_w_0().flex_1().text_size(px(12.)).line_height(px(18.)).text_color(content(0.5)).child("Sessions can share a worktree. Deleting one keeps its sessions by default and discards uncommitted changes. Its branch and commits are kept.")).child(self.refresh_button("refresh-settings-worktrees","Refresh",cx)).into_any_element()];
        if self.selected_project.is_empty() {
            page.push(note("Add a project to manage its worktrees."));
        } else if loading
            && self.discovery.worktrees.main.is_empty()
            && self.discovery.worktrees.error.is_none()
        {
            page.push(note("Loading worktrees…"));
        } else if let Some(error) = &self.discovery.worktrees.error {
            page.push(
                div()
                    .id("settings-page-error")
                    .mt_4()
                    .role(gpui::Role::Alert)
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error.clone())
                    .into_any_element(),
            );
        } else if self.discovery.worktrees.trees.is_empty() {
            page.push(
                div()
                    .mt_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .px_4()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(icon("folder-tree", 20.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("No additional worktrees"),
                    )
                    .child(
                        div().text_size(px(12.)).text_color(content(0.5)).child(
                            "Create a worktree to work on another branch in a separate folder.",
                        ),
                    )
                    .into_any_element(),
            );
        } else {
            let trees = self.discovery.worktrees.trees.clone();
            let pickable = |tree: &Tree| {
                tree.dirty == Some(false)
                    && !tree.locked
                    && !tree.tree.missing
                    && tree.tree.branch.is_some()
            };
            let clean: Vec<String> = trees
                .iter()
                .filter(|t| pickable(t))
                .map(|t| t.tree.path.clone())
                .collect();
            let chosen: Vec<Tree> = trees
                .iter()
                .filter(|t| self.selected_trees.contains(&t.tree.path))
                .cloned()
                .collect();
            if !clean.is_empty() {
                let count = chosen.len();
                let all = count == clean.len();
                page.push(
                    div()
                        .mt_4()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(div().text_size(px(12.)).text_color(content(0.5)).child(
                            if count == 0 {
                                format!(
                                    "{} clean {} can be deleted together.",
                                    clean.len(),
                                    if clean.len() == 1 {
                                        "worktree"
                                    } else {
                                        "worktrees"
                                    }
                                )
                            } else {
                                format!("{count} selected")
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    secondary_button(
                                        "select-clean-worktrees",
                                        if all {
                                            "Clear selection"
                                        } else {
                                            "Select all clean"
                                        },
                                        false,
                                        false,
                                    )
                                    .child(if all {
                                        "Clear selection"
                                    } else {
                                        "Select all clean"
                                    })
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if all {
                                                this.selected_trees.clear();
                                            } else {
                                                this.selected_trees =
                                                    clean.iter().cloned().collect();
                                            }
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    secondary_button(
                                        "delete-selected-worktrees",
                                        "Delete selected worktrees",
                                        true,
                                        count == 0,
                                    )
                                    .child(icon("trash", 14.))
                                    .child(if count == 0 {
                                        "Delete selected".to_owned()
                                    } else {
                                        format!("Delete selected ({count})")
                                    })
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            if !chosen.is_empty() {
                                                this.open_management(
                                                    Target::DeleteTrees(chosen.clone()),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        },
                                    )),
                                ),
                        )
                        .into_any_element(),
                );
            }
            page.push(
                div()
                    .mt_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .overflow_hidden()
                    .children(trees.into_iter().enumerate().map(|(ix, tree)| {
                        let t = &tree.tree;
                        let path = std::path::PathBuf::from(&t.path);
                        let count = self
                            .ctx
                            .sessions
                            .iter()
                            .filter(|(_, p, _)| {
                                monocode_core::files::is_equal_or_inside(p, &t.path)
                            })
                            .count();
                        let status = if t.missing {
                            "Missing folder"
                        } else if tree.dirty == Some(true) {
                            "Uncommitted changes"
                        } else if tree.dirty == Some(false) {
                            "Clean"
                        } else {
                            "Status unavailable"
                        };
                        let blocked = tree.locked || t.branch.is_none();
                        let pick = pickable(&tree);
                        let picked = self.selected_trees.contains(&t.path);
                        let pick_path = t.path.clone();
                        div()
                            .p_4()
                            .flex()
                            .items_start()
                            .gap_3()
                            .border_b_1()
                            .border_color(content(0.1))
                            .child(
                                checkbox(
                                    SharedString::from(format!("pick-tree-{ix}")),
                                    format!("Select {}", t.branch.as_deref().unwrap_or("worktree")),
                                    picked,
                                    !pick,
                                )
                                .when(pick, |d| {
                                    d.on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.selected_trees.remove(&pick_path) {
                                            this.selected_trees.insert(pick_path.clone());
                                        }
                                        cx.notify();
                                    }))
                                }),
                            )
                            .child(icon("folder-tree", 16.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(
                                                path.file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy()
                                                    .into_owned(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.4))
                                            .child(t.path.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.55))
                                            .child(icon("git-branch", 12.))
                                            .child(
                                                t.branch
                                                    .as_ref()
                                                    .map(|b| format!("Current branch: {b}"))
                                                    .unwrap_or_else(|| {
                                                        format!(
                                                            "Detached at {}",
                                                            t.head
                                                                .chars()
                                                                .take(7)
                                                                .collect::<String>()
                                                        )
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .text_size(px(11.))
                                            .text_color(content(0.55))
                                            .child(format!(
                                                "{count} {} in this worktree · {status}{}{}",
                                                if count == 1 { "session" } else { "sessions" },
                                                if tree.unpushed > 0 {
                                                    format!(
                                                        " · {} unpublished commits",
                                                        tree.unpushed
                                                    )
                                                } else {
                                                    String::new()
                                                },
                                                if tree.locked { " · Locked" } else { "" }
                                            )),
                                    ),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("new-session-tree-{ix}")),
                                    format!(
                                        "New session in {}",
                                        t.branch.as_deref().unwrap_or("worktree")
                                    ),
                                    false,
                                    t.missing,
                                )
                                .child(icon("pencil-edit-02", 16.))
                                .on_click(cx.listener({
                                    let (path, branch, missing) =
                                        (t.path.clone(), t.branch.clone(), t.missing);
                                    move |this, _, _, cx| {
                                        if !missing {
                                            cx.emit(SettingsEvent::NewSessionInWorktree {
                                                cwd: this.selected_project.clone(),
                                                path: path.clone(),
                                                branch: branch.clone(),
                                            });
                                        }
                                    }
                                })),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("reveal-tree-{ix}")),
                                    format!("Reveal {}", t.branch.as_deref().unwrap_or("worktree")),
                                    false,
                                    t.missing,
                                )
                                .child(icon("folder-open", 16.))
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("delete-tree-{ix}")),
                                    format!("Delete {}", t.branch.as_deref().unwrap_or("worktree")),
                                    false,
                                    blocked,
                                )
                                .child(icon("trash", 16.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if !blocked {
                                            this.open_management(
                                                Target::DeleteTree(tree.clone()),
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                    }))
                    .into_any_element(),
            );
        }
        if ready && !self.discovery.worktrees.sweep.is_empty() {
            page.push(self.worktree_sweep(cx));
        }
        if ready {
            page.push(note(format!(
                "New worktrees are created in {}.",
                self.discovery.worktrees.default_root.display()
            )));
        }
        page
    }
}
