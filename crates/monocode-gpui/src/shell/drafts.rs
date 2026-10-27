//! Drafts the agent opens in the open conversation. Each is a card in the
//! thread; this holds the live pages behind those cards. Requests from the
//! agent's MCP tools arrive here, run on the main thread, and answer with what
//! the page looks like.
use super::*;
use crate::draft::{self, Draft, DraftEvent};
use monocode_core::visual_bridge::{
    DraftAction, DraftContent, DraftFeedback, DraftReply, DraftRequest,
};

/// Live pages kept per conversation; opening another retires the oldest, and
/// its card says so.
const LIVE_PER_CONVERSATION: usize = 3;

impl Shell {
    pub(super) fn init_draft_inbox(&mut self, cx: &mut Context<Self>) {
        if cx.is_test() {
            return;
        }
        let requests = crate::draft::inbox();
        cx.spawn(async move |this, cx| {
            while let Ok(request) = requests.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.handle_draft_request(request, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// The session whose conversation is on screen.
    pub(super) fn visible_session(&self) -> Option<i64> {
        self.workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))
    }

    /// A native view stays where it was last placed, so the drafts of every
    /// conversation that is not on screen step aside. Within a conversation,
    /// the thread hides the ones scrolled out of view.
    pub(super) fn hide_drafts_off_screen(&self, cx: &App) {
        let showing = self.visible_session();
        for (session, drafts) in &self.drafts {
            if Some(*session) != showing {
                for draft in drafts {
                    draft.read(cx).hide();
                }
            }
        }
    }

    fn reply_draft(request: &DraftRequest, result: Result<DraftReply, String>) {
        let _ = request.reply.send(result);
    }

    fn draft_named(&self, session: i64, id: &str, cx: &App) -> Option<Entity<Draft>> {
        self.drafts
            .get(&session)?
            .iter()
            .find(|draft| draft.read(cx).id() == id)
            .cloned()
    }

    fn handle_draft_request(
        &mut self,
        request: DraftRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.visible_session() != Some(request.session) || self.mono_active(cx) {
            return Self::reply_draft(
                &request,
                Err("Drafts are drawn in the open conversation, and this one is in the background. Try again when the user is back in it.".into()),
            );
        }
        let session = request.session;
        let draft = match (&request.draft, &request.content) {
            (Some(id), _) => {
                match self.draft_named(session, id, cx) {
                    Some(draft) => draft,
                    None => {
                        return Self::reply_draft(
                        &request,
                        Err("That draft was closed or replaced. Start a new one with html or url.".into()),
                    );
                    }
                }
            }
            (None, Some(content)) => {
                self.create_draft(session, content.clone(), &request, window, cx)
            }
            (None, None) => {
                return Self::reply_draft(
                    &request,
                    Err("Pass html or url to start a draft".into()),
                );
            }
        };
        if matches!(request.action, DraftAction::Close) {
            let id = draft.read(cx).id().to_owned();
            self.drop_draft(session, &id, cx);
            cx.notify();
            return Self::reply_draft(&request, Ok(DraftReply::Closed { draft: id }));
        }
        cx.notify();
        let weak = draft.downgrade();
        cx.spawn(async move |_, cx| {
            let result = Draft::run(&weak, &request, cx).await;
            Self::reply_draft(&request, result);
        })
        .detach();
    }

    fn create_draft(
        &mut self,
        session: i64,
        content: DraftContent,
        request: &DraftRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Draft> {
        let draft = cx.new(|cx| {
            Draft::new(
                request.id.clone(),
                session,
                content,
                (request.width, request.height),
                request.light,
                request.title.clone(),
                cx,
            )
        });
        self.add_draft(session, &draft, window, cx);
        draft
    }

    fn add_draft(
        &mut self,
        session: i64,
        draft: &Entity<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = draft.read(cx).id().to_owned();
        self.observations.push(cx.subscribe_in(
            draft,
            window,
            move |this, draft, event: &DraftEvent, _, cx| match event {
                DraftEvent::Closed => {
                    this.drop_draft(session, &id, cx);
                    cx.notify();
                }
                DraftEvent::Send => this.deliver_draft_notes(session, draft, cx),
            },
        ));
        let live = self.drafts.entry(session).or_default();
        live.push(draft.clone());
        let retired = (live.len() > LIVE_PER_CONVERSATION).then(|| live.remove(0));
        if let Some(old) = retired {
            old.read(cx).hide();
            let old_id = old.read(cx).id().to_owned();
            draft::unregister(&old_id, cx);
        }
        let id = draft.read(cx).id().to_owned();
        draft::register(&id, draft, cx);
    }

    /// The person sent their notes. An agent that is waiting gets them as the
    /// result of its wait; otherwise they go to the conversation as a message,
    /// steering a running turn where the provider can take it.
    fn deliver_draft_notes(&mut self, session: i64, draft: &Entity<Draft>, cx: &mut Context<Self>) {
        if draft.read(cx).waiting() {
            return;
        }
        let (notes, actions) = draft.update(cx, |draft, _| draft.take_outbox());
        if notes.is_empty() {
            return;
        }
        let title = draft.read(cx).title();
        let text = DraftFeedback::message(&title, &notes, &actions);
        if let Some(view) = self.sessions.get(&session).cloned() {
            view.update(cx, |view, cx| view.send_draft_feedback(text, cx));
        }
    }

    fn drop_draft(&mut self, session: i64, id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.drafts.get_mut(&session) else {
            return;
        };
        let Some(at) = live.iter().position(|draft| draft.read(cx).id() == id) else {
            return;
        };
        let draft = live.remove(at);
        if live.is_empty() {
            self.drafts.remove(&session);
        }
        draft.read(cx).hide();
        draft::unregister(id, cx);
    }

    /// A kept draft's card turns into its live page, in place.
    pub(super) fn open_kept_draft(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.visible_session() else {
            return;
        };
        let id = draft::kept_id(path);
        if self.draft_named(session, &id, cx).is_some() {
            return;
        }
        let Ok(html) = std::fs::read_to_string(path) else {
            self.error = Some("That draft's page is no longer available.".into());
            cx.notify();
            return;
        };
        let draft = cx.new(|cx| {
            Draft::new(
                Some(id),
                session,
                DraftContent::Html(html),
                (None, None),
                None,
                None,
                cx,
            )
        });
        self.add_draft(session, &draft, window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn request(
        session: i64,
        draft: Option<String>,
        content: Option<DraftContent>,
        action: DraftAction,
    ) -> (DraftRequest, mpsc::Receiver<Result<DraftReply, String>>) {
        let (reply, receive) = mpsc::channel();
        (
            DraftRequest {
                session,
                draft,
                content,
                action,
                width: None,
                height: None,
                light: None,
                title: None,
                id: None,
                reply,
            },
            receive,
        )
    }

    #[gpui::test]
    fn a_draft_lives_in_the_open_conversation_and_background_ones_are_refused(
        cx: &mut gpui::TestAppContext,
    ) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                cx.set_reduce_motion(true);
                shell.workspace.classic_layout = false;
                shell.layout_changed(cx);
                let open = shell.visible_session().expect("a conversation is open");
                let html = || Some(DraftContent::Html("<p>Hi</p>".into()));

                // Another conversation is in the background: a clear error.
                let (background, answer) =
                    request(open + 1000, None, html(), DraftAction::Snapshot);
                shell.handle_draft_request(background, window, cx);
                let error = answer.try_recv().unwrap().err().unwrap();
                assert!(error.contains("background"), "{error}");
                assert!(shell.drafts.is_empty());

                // The open one gets a draft under the id the bridge chose, which
                // its card in the thread finds.
                let (mut shown, _answer) = request(open, None, html(), DraftAction::Snapshot);
                shown.id = Some("draft-1-abc".into());
                shell.handle_draft_request(shown, window, cx);
                assert_eq!(shell.drafts[&open].len(), 1);
                assert!(draft::live("draft-1-abc", cx).is_some());
                // No side panel opens for it.
                assert!(!shell.workspace.right_panel_open);

                // A stale id is refused rather than acting on another page.
                let (stale, answer) =
                    request(open, Some("draft-0-0".into()), None, DraftAction::Snapshot);
                shell.handle_draft_request(stale, window, cx);
                assert!(answer.try_recv().unwrap().is_err());
                assert_eq!(shell.drafts[&open].len(), 1);

                // Closing frees it and its card stops finding a page.
                let (close, answer) =
                    request(open, Some("draft-1-abc".into()), None, DraftAction::Close);
                shell.handle_draft_request(close, window, cx);
                assert!(matches!(
                    answer.try_recv().unwrap(),
                    Ok(DraftReply::Closed { draft }) if draft == "draft-1-abc"
                ));
                assert!(shell.drafts.is_empty());
                assert!(draft::live("draft-1-abc", cx).is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn drafts_stack_in_the_thread_and_the_oldest_is_retired(cx: &mut gpui::TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                cx.set_reduce_motion(true);
                shell.workspace.classic_layout = false;
                shell.layout_changed(cx);
                let open = shell.visible_session().unwrap();
                let mut answers = Vec::new();
                for n in 0..=LIVE_PER_CONVERSATION {
                    let page = Some(DraftContent::Html(format!("<p>{n}</p>")));
                    let (mut next, answer) = request(open, None, page, DraftAction::Snapshot);
                    next.id = Some(format!("draft-1-{n}"));
                    shell.handle_draft_request(next, window, cx);
                    answers.push(answer);
                }
                assert_eq!(shell.drafts[&open].len(), LIVE_PER_CONVERSATION);
                assert!(draft::live("draft-1-0", cx).is_none());
                assert!(draft::live(&format!("draft-1-{LIVE_PER_CONVERSATION}"), cx).is_some());
            })
            .unwrap();
    }
}
