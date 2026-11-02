//! A new worktree is born with a branch named from the request. A text model
//! then makes that branch conventional (`mc/fix/sidebar-scroll`) in the
//! background; the folder keeps its name so the session's paths stay valid.
use super::*;
use monocode_core::git::{self, naming};

impl Conversation {
    pub(super) fn refine_worktree_branch(
        &mut self,
        path: String,
        message: String,
        cx: &mut Context<Self>,
    ) {
        if self.is_remote() || self.settings.mono.is_some() {
            return;
        }
        let text = self.task_text_model(crate::settings::TextTask::Branch, cx);
        let project = self.summary.cwd.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let work = cx.background_executor().spawn(async move {
            // The name the worktree has now; a later manual change wins.
            let expected = git::worktree_branch(Path::new(&path))?;
            if !expected.starts_with(naming::PREFIX) || cfg!(test) {
                return None;
            }
            let wanted = naming::generate_for(text, Path::new(&path), &message, cancel)?.branch();
            match git::rename_worktree_branch(
                Path::new(&project),
                Path::new(&path),
                &expected,
                &wanted,
            ) {
                Ok(name) => Some((path, name)),
                Err(error) => {
                    eprintln!("Kept worktree branch {expected}: {error}");
                    None
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let Some((path, _)) = work.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.settings.worktree_cwd.as_deref() == Some(path.as_str()) {
                    this.refresh_git(cx);
                    cx.emit(SessionChanged);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
