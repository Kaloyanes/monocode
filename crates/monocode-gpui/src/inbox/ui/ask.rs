//! "Ask about this PR": a chat with a read-only agent beside the pull request.
//! The button lives in the PR header, so it is there from the Summary tab and
//! from the Code tab alike.
use super::*;

const WIDTH: f32 = 400.;

/// The label shown, and the question sent.
const SUGGESTIONS: [(&str, &str); 3] = [
    ("Summarize this PR", ""),
    (
        "What could break?",
        "What could break because of this change? Check the callers and the tests, and number the risks.",
    ),
    (
        "Is it tested?",
        "Do the tests cover this change? Name what is missing.",
    ),
];

impl Inbox {
    pub(super) fn ask_button(&self, cx: &mut Context<Self>) -> AnyElement {
        icon_action("inbox-ask", "Ask about this PR", "sparkles")
            .when(self.ask_open, |d| {
                d.bg(content(0.1)).text_color(content(1.))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.ask_open {
                    this.close_ask(cx);
                } else {
                    this.open_ask(cx);
                }
            }))
            .into_any_element()
    }

    /// The pane beside the detail, when it is open.
    pub(super) fn ask_pane_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.ask_open || self.detail_only {
            return None;
        }
        let item = self.item()?;
        let chat = self.ask_pane().map(|pane| pane.view.clone());
        let header = div()
            .h(px(36.))
            .flex_shrink_0()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(content(0.07))
            .child(icon("sparkles", 14.).text_color(content(0.7)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(1.))
                    .child(format!("Ask about {}", item.reference())),
            )
            .when(chat.is_some(), |d| {
                d.child(
                    icon_action("inbox-ask-open", "Open in conversation", "external-link")
                        .h(px(24.))
                        .px(px(8.))
                        .on_click(cx.listener(|this, _, _, cx| this.open_ask_in_conversation(cx))),
                )
            })
            .child(
                icon_button("inbox-ask-close", "Close", "close")
                    .size(px(20.))
                    .on_click(cx.listener(|this, _, _, cx| this.close_ask(cx))),
            );
        let body = match chat {
            Some(chat) => div().flex_1().min_h_0().child(chat).into_any_element(),
            None => self.ask_start_view(cx),
        };
        Some(
            div()
                .id("inbox-ask-pane")
                .role(Role::Group)
                .aria_label("Ask about this pull request")
                .w(px(WIDTH))
                .flex_shrink_0()
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(content(0.07))
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    /// Before the first question: what to ask, and where to type it.
    fn ask_start_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let starting = self.ask_starting;
        let can_send = !starting && !self.ask_input.read(cx).content.trim().is_empty();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .text_color(content(0.55))
                            .child(
                                "Ask anything about this pull request. The agent reads the diff and the code around it, and cites lines. It is read-only, and nothing reaches the author unless you post it.",
                            ),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.)).children(
                            SUGGESTIONS.into_iter().enumerate().map(
                                |(ix, (label, question))| {
                                    div()
                                        .id(("inbox-ask-suggestion", ix))
                                        .role(Role::Button)
                                        .aria_label(label)
                                        .h(px(26.))
                                        .px(px(10.))
                                        .flex()
                                        .items_center()
                                        .rounded_full()
                                        .border_1()
                                        .border_color(content(0.12))
                                        .text_size(px(12.))
                                        .text_color(content(0.8))
                                        .cursor_pointer()
                                        .when(starting, |d| d.opacity(0.4))
                                        .hover(|s| s.bg(content(0.07)).text_color(content(1.)))
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.ask_question(question.to_string(), cx)
                                        }))
                                },
                            ),
                        ),
                    )
                    .when(starting, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.55))
                                .child("Reading the diff…"),
                        )
                    })
                    .when_some(self.ask_error.clone(), |d, text| d.child(error(text))),
            )
            .child(
                div().p(px(12.)).flex_shrink_0().child(
                    div()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.05))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px(px(12.))
                                .py(px(8.))
                                .when(starting, |d| d.opacity(0.4))
                                .child(self.ask_input.clone()),
                        )
                        .child(
                            div().px(px(8.)).pb(px(8.)).flex().justify_end().child(
                                action("inbox-ask-send", "Ask")
                                    .px(px(12.))
                                    .bg(content(1.))
                                    .text_color(shell_fill())
                                    .when(!can_send, |d| d.opacity(0.4))
                                    .on_click(cx.listener(|this, _, _, cx| this.submit_ask(cx))),
                            ),
                        ),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::inbox::DiffFile;
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    #[derive(Debug, PartialEq)]
    enum Seen {
        Asked {
            question: String,
            prompt: String,
            title: String,
        },
        Opened(i64),
    }

    fn pr(number: i64) -> Item {
        Item {
            provider: "github".into(),
            kind: "pr".into(),
            repo: "acme/app".into(),
            number,
            title: "Add retry to uploader".into(),
            url: format!("https://github.com/acme/app/pull/{number}"),
            project_path: "/private/tmp/inbox-ask".into(),
            ..Default::default()
        }
    }

    fn inbox(cx: &mut TestAppContext) -> (Entity<Inbox>, Rc<RefCell<Vec<Seen>>>) {
        let inbox = cx.new(|cx| {
            let store = Arc::new(Mutex::new(
                monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
            ));
            let mut inbox = Inbox::new_inner(
                store,
                vec![],
                PathBuf::from("/private/tmp/inbox-ask"),
                true,
                cx,
            );
            inbox.snapshot.items = vec![pr(7), pr(8)];
            inbox.selected = Some(pr(7).key());
            inbox.details = Some(Details {
                body: "Retries uploads with backoff.".into(),
                author: "sam".into(),
                head_ref_name: "feat/retry".into(),
                base_ref_name: "main".into(),
                ..Default::default()
            });
            // What a fetch would have loaded, so no `gh` call is made.
            inbox.diff = Some(Diff {
                additions: 1,
                deletions: 1,
                files: vec![DiffFile {
                    path: "src/retry.rs".into(),
                    additions: 1,
                    deletions: 1,
                }],
                patch: "--- a/src/retry.rs\n+++ b/src/retry.rs\n@@ -1 +1 @@\n-sleep(2)\n+sleep(jitter())\n".into(),
                truncated: false,
            });
            inbox
        });
        let seen = Rc::new(RefCell::new(vec![]));
        let log = seen.clone();
        cx.update(|cx| {
            cx.subscribe(&inbox, move |_, event: &Event, _| match event {
                Event::AskPr {
                    question,
                    prompt,
                    title,
                    ..
                } => log.borrow_mut().push(Seen::Asked {
                    question: question.clone(),
                    prompt: prompt.clone(),
                    title: title.clone(),
                }),
                Event::OpenSession(id) => log.borrow_mut().push(Seen::Opened(*id)),
                _ => {}
            })
            .detach();
        });
        (inbox, seen)
    }

    fn chat(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
        let store = monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let mut summary = store.create("codex", "/missing/inbox-ask-project").unwrap();
        summary.provider = "fixture".into();
        cx.update(|cx| {
            crate::input::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Conversation::new(summary, Arc::new(Mutex::new(store)), cx))
            })
            .unwrap()
        })
    }

    #[gpui::test]
    fn the_first_question_carries_the_pull_request_and_its_diff(cx: &mut TestAppContext) {
        let (inbox, seen) = inbox(cx);
        inbox.update(cx, |inbox, cx| inbox.ask_question("Why jitter?".into(), cx));
        cx.run_until_parked();
        let seen = seen.borrow();
        let [
            Seen::Asked {
                question,
                prompt,
                title,
            },
        ] = seen.as_slice()
        else {
            panic!("expected one question, got {seen:?}");
        };
        assert_eq!(question, "Why jitter?");
        assert!(prompt.contains("+sleep(jitter())"));
        assert!(prompt.contains("Branch: feat/retry into main"));
        assert!(prompt.ends_with("Why jitter?"));
        assert_eq!(title, "#7 · Ask · Add retry to uploader");
        inbox.read_with(cx, |inbox, _| {
            assert!(inbox.ask_open && inbox.ask_starting);
        });
    }

    #[gpui::test]
    fn a_second_question_goes_to_the_same_chat(cx: &mut TestAppContext) {
        let (inbox, seen) = inbox(cx);
        let window = chat(cx);
        let view = window.entity(cx).unwrap();
        inbox.update(cx, |inbox, cx| {
            inbox.attach_ask(pr(7).key(), 11, view.clone(), cx)
        });
        inbox.update(cx, |inbox, cx| inbox.ask_question("And tests?".into(), cx));
        cx.run_until_parked();
        assert!(seen.borrow().is_empty(), "no second chat is started");
        view.read_with(cx, |chat, _| assert!(chat.summary.has_messages));
    }

    #[gpui::test]
    fn the_chat_shows_only_beside_its_own_pull_request(cx: &mut TestAppContext) {
        let (inbox, _) = inbox(cx);
        let window = chat(cx);
        let view = window.entity(cx).unwrap();
        inbox.update(cx, |inbox, cx| {
            assert_eq!(inbox.ask_session(), None);
            inbox.attach_ask(pr(7).key(), 11, view, cx);
            assert_eq!(inbox.ask_session(), Some(11));
            inbox.selected = Some(pr(8).key());
            assert_eq!(inbox.ask_session(), None, "another PR has no chat yet");
            inbox.selected = Some(pr(7).key());
            inbox.close_ask(cx);
            assert_eq!(inbox.ask_session(), None, "a closed pane is not drawn");
        });
    }

    #[gpui::test]
    fn opening_the_chat_in_the_conversation_hands_it_to_the_shell(cx: &mut TestAppContext) {
        let (inbox, seen) = inbox(cx);
        let window = chat(cx);
        let view = window.entity(cx).unwrap();
        inbox.update(cx, |inbox, cx| {
            inbox.attach_ask(pr(7).key(), 11, view, cx);
            inbox.open_ask_in_conversation(cx);
            assert_eq!(inbox.ask_session(), None);
            assert!(!inbox.ask_open);
        });
        assert_eq!(*seen.borrow(), vec![Seen::Opened(11)]);
    }

    #[gpui::test]
    fn citing_a_changed_file_opens_the_code_tab_and_others_fall_through(cx: &mut TestAppContext) {
        let (inbox, _) = inbox(cx);
        inbox.update(cx, |inbox, cx| {
            let diff = inbox.diff.clone().unwrap();
            inbox
                .diff_view
                .update(cx, |view, cx| view.set_files(diff::models(&diff), cx));
            assert_eq!(inbox.tab, Tab::Summary);
            assert!(inbox.cite("/private/tmp/inbox-ask/src/retry.rs:1:1", cx));
            assert_eq!(inbox.tab, Tab::Code);
            inbox.tab = Tab::Summary;
            assert!(!inbox.cite("/private/tmp/inbox-ask/src/other.rs:9:1", cx));
            assert_eq!(
                inbox.tab,
                Tab::Summary,
                "an unchanged file does not switch tabs"
            );
        });
    }

    #[gpui::test]
    fn pending_review_comments_are_kept_per_pull_request_and_survive_a_reload(
        cx: &mut TestAppContext,
    ) {
        use monocode_core::inbox::review::Side;
        let (inbox, _) = inbox(cx);
        inbox.update(cx, |inbox, cx| {
            inbox.add_review_comment("src/retry.rs".into(), 1, Side::Right, " Why? ".into(), cx);
            inbox.add_review_comment("src/retry.rs".into(), 1, Side::Left, "  ".into(), cx);
            inbox.add_review_comment("src/retry.rs".into(), 2, Side::Left, "Gone?".into(), cx);
            let comments = inbox.review_comments();
            assert_eq!(comments.len(), 2, "a blank comment is not kept");
            assert_eq!((comments[0].id, comments[1].id), (1, 2));
            assert_eq!(comments[0].text, "Why?");

            // Another pull request has its own, and a reload reads the saved ones.
            inbox.selected = Some(pr(8).key());
            assert!(inbox.review_comments().is_empty());
            inbox.selected = Some(pr(7).key());
            inbox.review.clear();
            inbox.load_review(cx);
            assert_eq!(inbox.review_comments().len(), 2);

            inbox.remove_review_comment(1, cx);
            assert_eq!(inbox.review_comments().len(), 1);
            inbox.review.clear();
            inbox.load_review(cx);
            assert_eq!(inbox.review_comments()[0].text, "Gone?");
        });
    }

    #[gpui::test]
    fn submitting_with_nothing_pending_does_nothing(cx: &mut TestAppContext) {
        let (inbox, _) = inbox(cx);
        inbox.update(cx, |inbox, cx| {
            inbox.submit_review(cx);
            assert!(!inbox.review_posting);
        });
    }

    #[gpui::test]
    fn a_failed_start_shows_the_error_and_lets_the_reviewer_retry(cx: &mut TestAppContext) {
        let (inbox, _) = inbox(cx);
        inbox.update(cx, |inbox, cx| {
            inbox.ask_question("Why?".into(), cx);
            assert!(inbox.ask_starting);
            inbox.ask_failed("No project folder".into(), cx);
            assert!(!inbox.ask_starting);
            assert_eq!(inbox.ask_error.as_deref(), Some("No project folder"));
            inbox.ask_question("Why?".into(), cx);
            assert!(inbox.ask_starting && inbox.ask_error.is_none());
        });
    }

    /// Renders the real inbox offscreen with Metal, for looking at the pane.
    /// Run explicitly: `cargo test inbox_ask_screenshots -- --ignored --nocapture`
    #[test]
    #[ignore = "Requires a Metal device; run explicitly on macOS"]
    fn inbox_ask_screenshots() {
        use gpui::{HeadlessAppContext, px, size};
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        let item = pr(482);
        let window = cx
            .open_window(size(px(1280.), px(760.)), |_, cx| {
                cx.new(|cx| {
                    crate::input::bind(cx);
                    let store = Arc::new(Mutex::new(
                        monocode_core::store::Store::open(std::path::Path::new(":memory:"))
                            .unwrap(),
                    ));
                    let mut view = Inbox::new_inner(
                        store,
                        vec![],
                        PathBuf::from("/private/tmp/inbox-ask"),
                        false,
                        cx,
                    );
                    view.snapshot.items = vec![item.clone()];
                    view.snapshot.connections.insert(
                        "github".into(),
                        model::Status {
                            connected: true,
                            ..Default::default()
                        },
                    );
                    view.state.source = "github".into();
                    view.selected = Some(item.key());
                    let body = "Retries failed uploads with exponential backoff and jitter.";
                    let weak = cx.weak_entity();
                    view.details = Some(Details {
                        body: body.into(),
                        author: "sam".into(),
                        base_ref_name: "main".into(),
                        head_ref_name: "feat/upload-retry".into(),
                        ..Default::default()
                    });
                    view.document = Some(cx.new(|cx| {
                        document::InboxDocument::linked(
                            body.into(),
                            PathBuf::from("/repo"),
                            weak,
                            cx,
                        )
                    }));
                    view
                })
            })
            .unwrap();
        let shoot = |cx: &mut HeadlessAppContext, name: &str| {
            let _ = cx.capture_screenshot(window.into());
            let image = cx.capture_screenshot(window.into()).unwrap();
            image.save(format!("/tmp/inbox-ask-{name}.png")).unwrap();
        };
        shoot(&mut cx, "1-overview");
        window
            .update(&mut cx, |view, _, cx| view.open_ask(cx))
            .unwrap();
        shoot(&mut cx, "2-open");
        window
            .update(&mut cx, |view, _, cx| {
                view.ask_starting = true;
                view.ask_input.update(cx, |input, cx| {
                    input.set_text("Why jitter here?".into(), cx)
                });
                cx.notify();
            })
            .unwrap();
        shoot(&mut cx, "3-starting");
        window
            .update(&mut cx, |view, _, cx| {
                let store = Arc::new(Mutex::new(
                    monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
                ));
                let mut summary = store
                    .lock()
                    .unwrap()
                    .create("codex", "/missing/inbox-ask-shot")
                    .unwrap();
                summary.provider = "fixture".into();
                summary.title = "#482 · Ask · Add retry to uploader".into();
                let chat = cx.new(|cx| Conversation::new(summary, store, cx));
                chat.update(cx, |chat, cx| {
                    chat.submit_follow_up("Why jitter here?".into(), cx);
                    // The shell does this for a chat it keeps on screen.
                    chat.load(cx);
                    chat.transcript.update(cx, |t, cx| t.visible(true, cx));
                });
                view.attach_ask(pr(482).key(), 1, chat, cx);
            })
            .unwrap();
        cx.run_until_parked();
        shoot(&mut cx, "4-chat");
        window
            .update(&mut cx, |view, _, cx| {
                use monocode_core::inbox::review::Side;
                let patch = "diff --git a/src/retry.rs b/src/retry.rs\n--- a/src/retry.rs\n+++ b/src/retry.rs\n@@ -38,7 +38,9 @@ fn upload()\n     let mut attempt = 0;\n     loop {\n         match send(&chunk) {\n-            Err(_) => sleep(2),\n+            Err(err) if err.retryable() => {\n+                sleep(backoff(attempt) + jitter());\n+            }\n             Ok(done) => return done,\n         }\n         attempt += 1;\n";
                let diff = Diff {
                    additions: 3,
                    deletions: 1,
                    files: vec![DiffFile {
                        path: "src/retry.rs".into(),
                        additions: 3,
                        deletions: 1,
                    }],
                    patch: patch.into(),
                    truncated: false,
                };
                let files = diff::models(&diff);
                view.diff = Some(diff);
                view.diff_view.update(cx, |dv, cx| dv.set_files(files, cx));
                view.tab = Tab::Code;
                view.add_review_comment(
                    "src/retry.rs".into(),
                    43,
                    Side::Right,
                    "Why add jitter on top of the backoff?".into(),
                    cx,
                );
                view.add_review_comment(
                    "src/retry.rs".into(),
                    41,
                    Side::Left,
                    "Was the fixed 2s sleep relied on anywhere?".into(),
                    cx,
                );
                view.review_open = true;
                view.cite("/private/tmp/inbox-ask/src/retry.rs:42", cx);
            })
            .unwrap();
        cx.run_until_parked();
        shoot(&mut cx, "5-review-and-citation");
    }
}
