//! Archives conversations left idle for the number of days the Archive
//! settings page picks. It runs from the shell's 30-second clock, at most once
//! an hour, and never touches a conversation someone could be using.
use super::*;
use std::time::{Duration, Instant};

const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// Whether a conversation last touched at `updated_at` has been idle for
/// `days`. An unrecorded time (zero) is never old enough to archive.
pub(super) fn idle_for(now_ms: u64, updated_at: u64, days: u32) -> bool {
    days > 0 && updated_at > 0 && now_ms.saturating_sub(updated_at) >= u64::from(days) * DAY_MS
}

impl Shell {
    pub(super) fn poll_auto_archive(&mut self, cx: &mut Context<Self>) {
        if self.workspace.preferences.auto_archive_days == 0 {
            self.auto_archive_swept = None;
            return;
        }
        if self
            .auto_archive_swept
            .is_some_and(|at| at.elapsed() < SWEEP_EVERY)
        {
            return;
        }
        self.auto_archive_swept = Some(Instant::now());
        self.auto_archive_idle(cx);
    }

    /// Whether `id` is in use or waiting on something, so it must stay.
    fn auto_archive_keeps(&self, id: i64, cx: &App) -> bool {
        let meta = self.session_metadata(id, cx);
        meta.summary.pinned
            || meta.busy
            || meta.responding()
            || meta.needs_approval()
            || meta.has_saved_draft()
            || meta.has_composer_draft()
            || meta.usage_limit().is_some_and(|l| l.resume_at_reset)
            || self.archiving.contains(&id)
            || self.workspace.active == Some(id)
            || self.workspace.tab_of(id).is_some()
            || self.session_preview.as_ref().is_some_and(|p| p.id == id)
            || self.orchestrations.active(id)
            || self.thread_status(id, cx)["status"] != "idle"
    }

    /// The conversations to archive: idle past the cutoff and not kept. A
    /// subagent goes with its lead, and a lead waits for any child that is
    /// still recent or in use.
    pub(super) fn auto_archive_candidates(&self, now_ms: u64, cx: &App) -> Vec<i64> {
        let days = self.workspace.preferences.auto_archive_days;
        let stale = |id: i64| {
            self.session_summaries.contains_key(&id)
                && idle_for(
                    now_ms,
                    self.session_metadata(id, cx).summary.updated_at,
                    days,
                )
        };
        let mut ids: Vec<i64> = self
            .session_summaries
            .iter()
            .filter(|(id, s)| {
                !s.archived
                    && !self.thread_parents.contains_key(id)
                    && stale(**id)
                    && !self.auto_archive_keeps(**id, cx)
            })
            .filter(|(id, _)| {
                self.subagents(**id).into_iter().all(|child| {
                    self.session_summaries.get(&child).is_none_or(|c| {
                        c.archived || (stale(child) && !self.auto_archive_keeps(child, cx))
                    })
                })
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    fn auto_archive_idle(&mut self, cx: &mut Context<Self>) {
        let leads = self.auto_archive_candidates(monocode_core::time::now_ms(), cx);
        if leads.is_empty() {
            return;
        }
        let mut ids = Vec::new();
        for lead in leads {
            for child in self.subagents(lead) {
                if self
                    .session_summaries
                    .get(&child)
                    .is_some_and(|c| !c.archived)
                {
                    ids.push(child);
                }
            }
            ids.push(lead);
        }
        for &id in &ids {
            self.archiving.insert(id);
        }
        let store = self.store.clone();
        let work_ids = ids.clone();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            Ok::<_, String>(
                work_ids
                    .into_iter()
                    .map(|id| (id, store.set_archived(id, true)))
                    .collect::<Vec<_>>(),
            )
        });
        cx.spawn(async move |this, cx| {
            let results = work.await;
            let _ = this.update(cx, |this, cx| {
                for id in &ids {
                    this.archiving.remove(id);
                }
                match results {
                    Ok(results) => {
                        for (id, result) in results {
                            match result {
                                Ok(()) => this.set_thread_archived(id, true, cx),
                                Err(error) => eprintln!("auto-archive of {id} failed: {error}"),
                            }
                        }
                    }
                    Err(error) => eprintln!("auto-archive failed: {error}"),
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    const NOW: u64 = 100 * DAY_MS;

    #[test]
    fn idle_for_counts_whole_days_since_the_last_activity() {
        assert!(!idle_for(NOW, NOW - 3 * DAY_MS + 1, 3));
        assert!(idle_for(NOW, NOW - 3 * DAY_MS, 3));
        assert!(idle_for(NOW, 1, 30));
    }

    #[test]
    fn idle_for_never_fires_when_off_or_without_a_recorded_time() {
        assert!(!idle_for(NOW, 1, 0));
        assert!(!idle_for(NOW, 0, 7));
        assert!(!idle_for(NOW, NOW + DAY_MS, 1));
    }

    #[gpui::test]
    fn the_sweep_keeps_what_is_pinned_recent_open_or_has_active_children(cx: &mut TestAppContext) {
        let (window, _, open) = crate::shell::tests::window(cx);
        let now = monocode_core::time::now_ms();
        let (old, pinned, recent, lead, fresh_child, stale_lead, stale_child) = window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.auto_archive_days = 7;
                let mut ids = Vec::new();
                for _ in 0..7 {
                    let summary = locked(&shell.store)
                        .unwrap()
                        .create("codex", &shell.workspace.project)
                        .unwrap();
                    ids.push(summary.id);
                    shell.add(summary, cx);
                }
                let age = |shell: &mut Shell, id: i64, days: u64, cx: &mut Context<Shell>| {
                    let at = now - days * DAY_MS;
                    shell.session_summaries.get_mut(&id).unwrap().updated_at = at;
                    shell.sessions[&id].update(cx, |s, _| s.summary.updated_at = at);
                };
                for (id, days) in [
                    (ids[0], 30),
                    (ids[1], 30),
                    (ids[2], 1),
                    (ids[3], 30),
                    (ids[4], 1),
                    (ids[5], 30),
                    (ids[6], 30),
                    (open, 30),
                ] {
                    age(shell, id, days, cx);
                }
                shell.sessions[&ids[1]].update(cx, |s, _| s.summary.pinned = true);
                shell.thread_parents.insert(ids[4], ids[3]);
                shell.thread_parents.insert(ids[6], ids[5]);
                shell.workspace.active = Some(open);
                (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6])
            })
            .unwrap();
        window
            .read_with(cx, |shell, cx| {
                let found = shell.auto_archive_candidates(now, cx);
                assert!(found.contains(&old), "idle for 30 days");
                assert!(found.contains(&stale_lead), "its only child is idle too");
                for (id, why) in [
                    (pinned, "pinned"),
                    (recent, "touched yesterday"),
                    (lead, "a child was touched yesterday"),
                    (open, "the open conversation"),
                    (fresh_child, "a subagent goes with its lead"),
                    (stale_child, "a subagent goes with its lead"),
                ] {
                    assert!(!found.contains(&id), "{id} is kept: {why}");
                }
            })
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.auto_archive_days = 0;
                assert_eq!(shell.auto_archive_candidates(now, cx), Vec::<i64>::new());
            })
            .unwrap();
    }
}
