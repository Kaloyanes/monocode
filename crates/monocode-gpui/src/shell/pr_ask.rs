//! Chats about a pull request, started from the Inbox's "Ask about this PR".
use super::*;

impl Shell {
    /// The chat an earlier "Ask about this PR" made for `item`, newest first.
    fn pr_ask_session(&self, item: &monocode_core::inbox::Item) -> Option<i64> {
        let work_item = item.work_item();
        self.session_summaries
            .values()
            .filter(|summary| {
                !summary.archived
                    && summary.work_item.as_ref() == Some(&work_item)
                    && monocode_core::inbox::ask::is_ask_title(&summary.title)
            })
            .map(|summary| summary.id)
            .max()
    }

    /// Hands the Inbox the chat it made earlier for `item`, if there is one.
    pub(super) fn find_pr_ask_session(
        &mut self,
        item: monocode_core::inbox::Item,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self
            .pr_ask_session(&item)
            .and_then(|id| self.ensure_session(id, cx).map(|chat| (id, chat)))
        else {
            return;
        };
        if let Some(inbox) = self.inbox.clone() {
            inbox.update(cx, |inbox, cx| {
                inbox.attach_ask(item.key(), chat.0, chat.1, cx)
            });
        }
    }

    /// Moves the "Ask about this PR" chat `id` from its tab back beside the
    /// pull request in the Inbox.
    pub(super) fn move_pr_ask_to_sidebar(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(url) = self
            .session_summaries
            .get(&id)
            .and_then(|summary| summary.work_item.as_ref())
            .map(|item| item.url.clone())
        else {
            return;
        };
        self.open_work_link(&url, window, cx);
        let (Some(inbox), Some(chat)) = (self.inbox.clone(), self.ensure_session(id, cx)) else {
            return;
        };
        if inbox.read(cx).can_open(&url) {
            inbox.update(cx, |inbox, cx| inbox.attach_selected_ask(id, chat, cx));
        }
    }

    /// Starts a read-only chat linked to the pull request, sends `question`
    /// with `prompt` as its context, and hands the chat to the Inbox, which
    /// shows it beside the pull request.
    pub(super) fn start_pr_ask_session(
        &mut self,
        item: monocode_core::inbox::Item,
        question: String,
        prompt: String,
        title: String,
        cx: &mut Context<Self>,
    ) {
        let cwd = if item.project_path.is_empty() {
            self.workspace.project.clone()
        } else {
            item.project_path.clone()
        };
        let kind = self.default_kind();
        let model = self
            .workspace
            .preferences
            .model_for_new_chat(kind.id(), &cwd);
        let settings = monocode_core::agent::Settings {
            effort: crate::conversation::preferred_effort(kind.id(), model.as_deref(), cx),
            model,
            access: monocode_core::agent::AccessMode::ReadOnly,
            account: crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, &cwd))
                .unwrap_or_default(),
            ..Default::default()
        };
        let store = self.store.clone();
        let key = item.key();
        let work = cx.background_executor().spawn(async move {
            if !std::path::Path::new(&cwd).is_dir() {
                return Err("Choose a project folder before asking about this pull request".into());
            }
            let store = locked(&store)?;
            let mut summary = store.create(kind.id(), &cwd)?;
            summary.title = title;
            summary.work_item = Some(item.work_item());
            store.title(summary.id, &summary.title)?;
            store.set_work_item(summary.id, summary.work_item.as_ref())?;
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((summary, settings)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    let Some(chat) = this.sessions.get(&id).cloned() else {
                        return;
                    };
                    chat.update(cx, |chat, cx| {
                        chat.seed_new_settings(settings.clone(), cx);
                        chat.submit_with_context(question, prompt, settings, cx);
                    });
                    if let Some(inbox) = this.inbox.clone() {
                        inbox.update(cx, |inbox, cx| inbox.attach_ask(key, id, chat, cx));
                    }
                    this.persist(cx);
                }
                Err(error) => {
                    if let Some(inbox) = this.inbox.clone() {
                        inbox.update(cx, |inbox, cx| inbox.ask_failed(error.clone(), cx));
                    }
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
