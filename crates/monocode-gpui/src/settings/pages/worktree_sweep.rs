//! Worktrees → Cleanup: every worktree with what decides whether it can go.
//! Only worktrees that are clean and merged (or fully pushed) get a Remove
//! button; the verdict is `TreeStatus::safety` in monocode-core, and removal
//! checks it again right before deleting.
use crate::{
    settings::{
        SettingsView,
        management::Target,
        widgets::{note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use monocode_core::worktree_management::{Safety, TreeStatus};

const GREEN: u32 = 0x4ade80;
const AMBER: u32 = 0xfbbf24;

fn chip(text: impl Into<SharedString>, color: Option<u32>) -> AnyElement {
    div()
        .rounded_md()
        .border_1()
        .border_color(color.map_or(content(0.1), |c| gpui::rgb(c).opacity(0.35).into()))
        .px(px(6.))
        .py(px(1.))
        .text_size(px(11.))
        .text_color(color.map_or(content(0.55), |c| gpui::rgb(c).into()))
        .child(text.into())
        .into_any_element()
}

fn folder(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn removable(status: &TreeStatus) -> bool {
    matches!(status.safety(), Safety::Merged | Safety::Pushed)
}

/// Why a removable worktree may go, e.g. "Merged into main".
fn reason(status: &TreeStatus, base: Option<&str>) -> String {
    match (status.safety(), base) {
        (Safety::Merged, Some(base)) => format!("Merged into {base}"),
        (Safety::Merged, None) => "Merged".into(),
        _ => "Pushed, no unique local commits".into(),
    }
}

impl SettingsView {
    pub(in crate::settings) fn worktree_sweep(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let base = self.discovery.worktrees.base.clone();
        let list = self.discovery.worktrees.sweep.clone();
        let eligible: Vec<TreeStatus> = list.iter().filter(|s| removable(s)).cloned().collect();
        let now = monocode_core::time::now_ms();
        let mut rows = Vec::new();
        for (ix, status) in list.iter().enumerate() {
            let t = &status.tree;
            let sessions = self
                .ctx
                .sessions
                .iter()
                .filter(|(_, p, _)| monocode_core::files::is_equal_or_inside(p, &t.path))
                .count();
            let mut chips = Vec::new();
            if t.main {
                chips.push(chip("Main checkout", None));
            }
            match status.dirty {
                Some(true) => chips.push(chip("Uncommitted changes", Some(AMBER))),
                Some(false) => chips.push(chip("Clean", None)),
                None => chips.push(chip("Status unavailable", Some(AMBER))),
            }
            if !t.main {
                match (status.merged(), &base) {
                    (Some(true), Some(base)) => {
                        chips.push(chip(format!("Merged into {base}"), Some(GREEN)))
                    }
                    (Some(false), Some(base)) => {
                        chips.push(chip(format!("Not merged into {base}"), None))
                    }
                    _ => chips.push(chip("Merge status unknown", Some(AMBER))),
                }
                match status.unpushed {
                    Some(0) => {}
                    Some(n) => chips.push(chip(format!("{n} unpushed"), Some(AMBER))),
                    None => chips.push(chip("Unpushed unknown", Some(AMBER))),
                }
                chips.push(chip(
                    if status.has_upstream {
                        "Has upstream"
                    } else {
                        "No upstream"
                    },
                    None,
                ));
                if status.locked {
                    chips.push(chip("Locked", Some(AMBER)));
                }
            }
            let last = status
                .last_commit
                .map(|at| {
                    format!(
                        "Last commit {}",
                        monocode_core::worktree_management::age_label(at, now)
                    )
                })
                .unwrap_or_default();
            let action = if removable(status) {
                let one = vec![status.clone()];
                secondary_button(
                    SharedString::from(format!("sweep-remove-{ix}")),
                    format!("Remove {}", t.branch.as_deref().unwrap_or("worktree")),
                    true,
                    false,
                )
                .debug_selector(|| format!("sweep-remove-{ix}"))
                .child(icon("trash", 14.))
                .child("Remove")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_management(Target::Sweep(one.clone()), window, cx);
                }))
                .into_any_element()
            } else if let Safety::Blocked(why) = status.safety() {
                div()
                    .max_w(px(220.))
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(why.message())
                    .into_any_element()
            } else {
                div().into_any_element()
            };
            rows.push(
                div()
                    .p_4()
                    .flex()
                    .items_start()
                    .gap_3()
                    .border_t_1()
                    .border_color(content(0.1))
                    .when(ix == 0, |d| d.border_t_0())
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(icon("git-branch", 12.))
                                    .child(t.branch.clone().unwrap_or_else(|| {
                                        format!(
                                            "Detached at {}",
                                            t.head.chars().take(7).collect::<String>()
                                        )
                                    })),
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
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1()
                                    .children(chips),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .child(format!(
                                        "{last}{}{}",
                                        if last.is_empty() { "" } else { " · " },
                                        if sessions == 1 {
                                            "1 session".to_owned()
                                        } else {
                                            format!("{sessions} sessions")
                                        }
                                    )),
                            ),
                    )
                    .child(action)
                    .into_any_element(),
            );
        }
        let count = eligible.len();
        let bulk = secondary_button(
            "sweep-remove-merged",
            "Remove all merged worktrees",
            true,
            count == 0,
        )
        .debug_selector(|| "sweep-remove-merged".into())
        .child(icon("trash", 14.))
        .child(if count == 0 {
            "Remove all merged".to_owned()
        } else {
            format!("Remove all merged ({count})")
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if count > 0 {
                this.open_management(Target::Sweep(eligible.clone()), window, cx);
            }
        }))
        .into_any_element();
        if rows.is_empty() {
            rows.push(note("No worktrees found."));
        }
        self.group(
            Some("worktree-sweep"),
            false,
            "Cleanup",
            Some(
                format!(
                    "Only worktrees with no uncommitted changes whose commits are all merged into {} or pushed can be removed. Branches and commits are always kept.",
                    base.as_deref().unwrap_or("the base branch")
                )
                .into(),
            ),
            Some(bulk),
            rows,
        )
    }

    /// What the confirmation lists: exactly the folders that will be deleted.
    pub(in crate::settings) fn sweep_confirmation(&self, list: &[TreeStatus]) -> Vec<AnyElement> {
        let base = self.discovery.worktrees.base.as_deref();
        let sessions = list
            .iter()
            .map(|s| {
                self.ctx
                    .sessions
                    .iter()
                    .filter(|(_, p, _)| monocode_core::files::is_equal_or_inside(p, &s.tree.path))
                    .count()
            })
            .sum::<usize>();
        let mut rows = vec![note(
            "These folders are deleted from disk, including ignored files such as build output. Each branch and its commits are kept.",
        )];
        rows.push(
            div()
                .rounded_lg()
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.05))
                .p_3()
                .text_size(px(12.))
                .flex()
                .flex_col()
                .gap_2()
                .children(list.iter().map(|s| {
                    div()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(gpui::FontWeight::MEDIUM).child(format!(
                            "{} · {}",
                            folder(&s.tree.path),
                            s.tree.branch.as_deref().unwrap_or("detached")
                        )))
                        .child(div().text_color(content(0.45)).child(s.tree.path.clone()))
                        .child(div().text_color(content(0.55)).child(reason(s, base)))
                }))
                .into_any_element(),
        );
        if sessions > 0 {
            rows.push(note(format!(
                "{sessions} {} using these worktrees {} kept. Select a branch or worktree to continue them.",
                if sessions == 1 { "session" } else { "sessions" },
                if sessions == 1 { "is" } else { "are" },
            )));
        }
        rows.push(note(
            "Each worktree is checked again just before it is removed; one that changed since this list was drawn is skipped.",
        ));
        rows
    }
}
