//! Settings → Worktrees → Cleanup: removes several worktrees one after another.
//! Each is checked again by `remove_if_safe` right before it is removed, so a
//! worktree that gained changes since the list was drawn is skipped, not deleted.
//! Sessions in a removed worktree are kept and detached, as for a single delete.
use super::*;
use monocode_core::files::is_equal_or_inside;
use std::path::Path;

#[derive(Default)]
struct Outcome {
    removed: Vec<String>,
    detached: Vec<(Session, monocode_core::agent::Settings)>,
    skipped: Vec<String>,
}

fn folder(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

fn report(removed: usize, skipped: &[String]) -> Result<(), String> {
    if skipped.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Removed {removed}. Skipped {}: {}",
        skipped.len(),
        skipped.join("; ")
    ))
}

impl Shell {
    pub(super) fn remove_swept_worktrees(
        &mut self,
        cwd: String,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let mut skipped = Vec::new();
        let paths: Vec<String> = paths
            .into_iter()
            .filter(|path| {
                let in_use = self
                    .terminals
                    .values()
                    .flatten()
                    .any(|t| is_equal_or_inside(&t.read(cx).cwd, path))
                    || self.session_summaries.keys().any(|id| {
                        is_equal_or_inside(self.session_working_copy(*id, cx), path)
                            && self.sessions.get(id).is_some_and(|v| v.read(cx).busy)
                    });
                if in_use {
                    skipped.push(format!(
                        "{} is used by a terminal or running session",
                        folder(path)
                    ));
                }
                !in_use
            })
            .collect();
        let pending = self.group_writes.take();
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            if let Some(task) = pending {
                task.await;
            }
            let work = cx.background_executor().spawn(async move {
                let mut out = Outcome {
                    skipped,
                    ..Default::default()
                };
                let snapshot = monocode_core::git::snapshot(Path::new(&cwd))?;
                let main = snapshot
                    .worktrees
                    .first()
                    .ok_or("No main working copy found")?;
                for path in paths {
                    let main_path = Path::new(&main.path);
                    if let Err(e) = monocode_core::worktree_management::remove_if_safe(
                        main_path,
                        Path::new(&path),
                    ) {
                        out.skipped.push(format!("{}: {e}", folder(&path)));
                        continue;
                    }
                    match locked(&store).and_then(|s| s.detach_worktree(&path, &main.path)) {
                        Ok(detached) => out.detached.extend(detached),
                        Err(e) => out.skipped.push(format!(
                            "{} was removed, but its sessions could not be updated: {e}",
                            folder(&path)
                        )),
                    }
                    out.removed.push(path);
                }
                Ok::<_, String>(out)
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let result = result.and_then(|out| {
                    for (summary, settings) in out.detached {
                        this.session_summaries.insert(summary.id, summary.clone());
                        this.session_worktrees
                            .insert(summary.id, settings.worktree_cwd.clone());
                        if let Some(view) = this.sessions.get(&summary.id) {
                            view.update(cx, |v, cx| v.worktree_deleted(summary, settings, cx));
                        }
                    }
                    this.worktree_focus.retain(|_, focus| {
                        !out.removed
                            .iter()
                            .any(|p| is_equal_or_inside(&focus.path, p))
                    });
                    report(out.removed.len(), &out.skipped)
                });
                this.finish_worktree_action(result, cx);
            });
        })
        .detach();
    }
}
