//! Deletes Claude Code's leftover per-session temporary folders, which it
//! never cleans up itself (see `monocode_core::scratch`). Runs from the
//! shell's 30-second clock: on the first tick after launch, then every six
//! hours, off the UI thread.
use super::*;
use std::time::{Duration, Instant};

const SWEEP_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

impl Shell {
    pub(super) fn poll_scratch_sweep(&mut self, cx: &mut Context<Self>) {
        if self
            .scratch_swept
            .is_some_and(|at| at.elapsed() < SWEEP_EVERY)
        {
            return;
        }
        let Some(root) = monocode_core::scratch::root() else {
            return;
        };
        self.scratch_swept = Some(Instant::now());
        let working = self.working_sessions(cx);
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                // Read the threads, then let go of the store before touching disk.
                let threads = match locked(&store)
                    .and_then(|store| monocode_core::scratch::threads(&store, &working))
                {
                    Ok(threads) => threads,
                    Err(error) => {
                        eprintln!("Claude scratch sweep skipped: {error}");
                        return;
                    }
                };
                let swept =
                    monocode_core::scratch::sweep(&root, &threads, std::time::SystemTime::now());
                if swept.folders > 0 || swept.failed > 0 {
                    eprintln!(
                        "Freed {:.1} MB from {} Claude scratch folders ({} failed)",
                        swept.bytes as f64 / 1_000_000.0,
                        swept.folders,
                        swept.failed
                    );
                }
            })
            .detach();
    }

    /// Every thread with a turn running, chats and Monos alike.
    fn working_sessions(&self, cx: &App) -> HashSet<i64> {
        let mut ids: HashSet<i64> = self
            .sessions
            .iter()
            .filter(|(_, view)| view.read(cx).working())
            .map(|(id, _)| *id)
            .collect();
        ids.extend(self.busy_sessions.iter().copied());
        if let Some(monos) = &self.monos {
            ids.extend(
                monos
                    .read(cx)
                    .busy_sessions(cx)
                    .into_iter()
                    .map(|(id, _)| id),
            );
        }
        ids
    }
}
