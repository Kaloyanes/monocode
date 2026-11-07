//! A chat about a pull request, started from the Inbox. The transcript shows
//! the reviewer's question; the model also gets the pull request and its diff.
use super::*;

impl Conversation {
    /// Lets the owner of this chat claim the file links and chips in its
    /// answers.
    pub(crate) fn set_link_intercept(
        &mut self,
        intercept: Option<crate::markdown::LinkIntercept>,
        cx: &mut Context<Self>,
    ) {
        self.transcript.update(cx, |transcript, cx| {
            transcript.link_intercept = intercept;
            cx.notify();
        });
    }

    /// Sends `question` with `context` ahead of it for the model only.
    pub(crate) fn submit_with_context(
        &mut self,
        question: String,
        context: String,
        mut settings: Settings,
        cx: &mut Context<Self>,
    ) {
        settings.prepared_prompt = Some(context);
        self.quick_submit(question, settings, cx);
        // The turn took its own copy. Left here, the next question would
        // carry the whole diff again.
        self.settings.prepared_prompt = None;
    }

    /// A follow-up in the same chat. The model already has the context.
    pub(crate) fn submit_follow_up(&mut self, question: String, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.prepared_prompt = None;
        self.quick_submit(question, settings, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::store::Store;
    use std::sync::Mutex;

    fn chat(cx: &mut TestAppContext) -> gpui::WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        // The project folder is missing, so the turn fails after it is saved
        // and no provider is ever launched.
        let mut summary = store.create("codex", "/missing/pr-ask-project").unwrap();
        summary.provider = "fixture".into();
        cx.update(|cx| {
            input::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view
                })
            })
            .unwrap()
        })
    }

    #[gpui::test]
    fn the_transcript_shows_the_question_and_the_diff_stays_out_of_later_turns(
        cx: &mut TestAppContext,
    ) {
        let window = chat(cx);
        window
            .update(cx, |view, _, cx| {
                view.submit_with_context(
                    "Why jitter?".into(),
                    "The whole diff, then: Why jitter?".into(),
                    Settings {
                        access: AccessMode::ReadOnly,
                        ..Default::default()
                    },
                    cx,
                );
                assert!(view.summary.has_messages);
                let rows = view.transcript.read(cx).source_rows();
                let prompt: String = rows
                    .iter()
                    .filter(|row| row.is_prompt())
                    .map(|row| row.text.as_str())
                    .collect();
                assert_eq!(prompt, "Why jitter?");
                assert_eq!(view.settings.access, AccessMode::ReadOnly);
                assert!(
                    view.settings.prepared_prompt.is_none(),
                    "the next question must not carry the diff again"
                );
            })
            .unwrap();
    }
}
