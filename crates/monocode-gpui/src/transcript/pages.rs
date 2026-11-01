//! Pages the agent opens in the thread: the live draft behind a card, or a
//! plain note once it is gone.
use super::*;
use gpui::AnyElement;
use monocode_core::{transcript::images::ImageDocument, visual_bridge::DraftCard};

impl TranscriptView {
    /// Cards change height when their page opens, resizes or closes.
    pub(super) fn watch_drafts(&mut self, cx: &mut Context<Self>) {
        if self.draft_watch.is_some() {
            return;
        }
        self.draft_watch = Some(crate::draft::watch(cx, |this, cx| {
            for (ix, row) in this.rows.iter().enumerate() {
                if matches!(row.body, Body::Draft(..) | Body::Image(_)) {
                    this.list.remeasure_items(ix..ix + 1);
                }
            }
            cx.notify();
        }));
    }

    /// A live page places its native view on every paint, so a thread holding
    /// one can't be replayed from cache: the sentinel would take the replayed
    /// frame for one where the card was not drawn and hide the page.
    pub(crate) fn holds_live_draft(&self, cx: &gpui::App) -> bool {
        self.rows.iter().any(|row| {
            matches!(&row.body, Body::Draft(card, _) if crate::draft::live(&card.draft, cx).is_some())
        })
    }

    /// Whether a card for draft `id` is already in the thread. The page is one
    /// native view, so a second card for it would leave it in only one place.
    pub(super) fn shows_draft(&self, id: &str) -> bool {
        self.rows
            .iter()
            .any(|row| matches!(&row.body, Body::Draft(card, _) if card.draft == id))
    }

    /// The picture the agent kept of the draft whose card is the message at
    /// `at`, in the turn `turn..turn_end`.
    pub(super) fn kept_picture(
        &self,
        at: usize,
        turn: usize,
        turn_end: usize,
    ) -> Option<Arc<ImageDocument>> {
        let (_, picture) = monocode_core::visual_bridge::kept_upgrades(&self.source[turn..turn_end])
            .into_iter()
            .find(|&(card, _)| turn + card == at)?;
        let start = turn + picture;
        let end = (start + 1..turn_end)
            .find(|&i| self.source[i].first)
            .unwrap_or(turn_end);
        let text: String = self.source[start..end]
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        serde_json::from_str::<ImageDocument>(&text)
            .ok()
            .filter(|document| document.validate().is_ok())
            .map(Arc::new)
    }

    /// Whether a card above already carries this kept picture.
    pub(super) fn card_holds(&self, picture: &ImageDocument) -> bool {
        self.rows.iter().any(|row| {
            matches!(&row.body, Body::Draft(_, Some(kept)) if kept.path == picture.path)
        })
    }

    /// A draft's card: its live page, else the picture kept of it, else a
    /// note that it is closed.
    pub(super) fn draft_row(
        &mut self,
        card: &DraftCard,
        kept: Option<Arc<ImageDocument>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.watch_drafts(cx);
        if let Some(draft) = crate::draft::live(&card.draft, cx) {
            return draft.into_any_element();
        }
        if let Some(kept) = kept {
            return self.image_row(kept, cx);
        }
        // After a restart the saved page comes back on its own.
        if let Some(session) = self.draft_session {
            crate::draft::ask_to_reopen(session, &card.draft, cx);
        }
        let name = if card.title.is_empty() {
            "Draft"
        } else {
            &card.title
        };
        let reopen = self
            .draft_session
            .filter(|_| crate::draft::reopenable(&card.draft, cx))
            .map(|session| crate::draft::Reopen {
                session,
                draft: card.draft.clone(),
            });
        div()
            .w_full()
            .min_w_0()
            .py_3()
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(0.45))
            .child(icon("app-window", 14.))
            .child(div().min_w_0().truncate().child(format!("{name} · closed")))
            .when_some(reopen, |row, reopen| {
                row.child(
                    div()
                        .id(SharedString::from(format!("reopen-{}", reopen.draft)))
                        .debug_selector(|| "transcript-reopen-draft".into())
                        .role(Role::Button)
                        .aria_label("Open again")
                        .flex_shrink_0()
                        .h(px(24.))
                        .px_2p5()
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(content(0.12))
                        .text_color(content(0.8))
                        .hover(|s| s.bg(content(0.06)))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(Box::new(reopen.clone()), cx);
                        })
                        .child("Open again"),
                )
            })
            .into_any_element()
    }
}

/// A thread holding one draft card between two messages, for the draft
/// self-test.
#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn selftest_thread(
    card: &DraftCard,
    cx: &mut gpui::App,
) -> gpui::Entity<TranscriptView> {
    let row = |role: &str, text: String| Row {
        role: role.into(),
        text,
        first: true,
        sent_at: None,
    };
    cx.new(|cx| {
        let mut view = TranscriptView::new();
        let mut rows = vec![row("user", "Mock a pricing page for me".into())];
        rows.push(row(
            "assistant",
            "Here is a first pass. It is a live page, so click around in it.".into(),
        ));
        rows.push(row(
            monocode_core::visual_bridge::DRAFT_ROLE,
            serde_json::to_string(card).unwrap_or_default(),
        ));
        rows.push(row(
            "assistant",
            "Tell me what to change and I will update it in place.".into(),
        ));
        view.patch(0, rows, cx);
        view
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::Draft;
    use gpui::TestAppContext;
    use monocode_core::visual_bridge::DraftContent;

    fn row(role: &str, text: String) -> Row {
        Row {
            role: role.into(),
            text,
            first: true,
            sent_at: None,
        }
    }

    fn card_row(draft: &str) -> Row {
        row(
            monocode_core::visual_bridge::DRAFT_ROLE,
            serde_json::to_string(&DraftCard {
                draft: draft.into(),
                title: "Pricing".into(),
            })
            .unwrap(),
        )
    }

    /// A picture kept from a draft, with the page beside it as the app saves it.
    fn kept_row(dir: &std::path::Path, name: &str) -> Row {
        let path = dir.join(format!("{name}.png"));
        std::fs::write(path.with_extension("html"), "<p>kept</p>").unwrap();
        row(
            monocode_core::transcript::images::ROLE,
            serde_json::to_string(&monocode_core::transcript::images::ImageDocument {
                title: name.into(),
                path,
                mime_type: "image/png".into(),
                width: 800,
                height: 400,
            })
            .unwrap(),
        )
    }

    fn keep_call(draft: &str) -> Row {
        let mut call = monocode_core::tool::ToolCall::new("keep", "mcp__monocode__draft_keep");
        call.input = serde_json::json!({"draft": draft, "title": "Pricing"}).to_string();
        row("tool", serde_json::to_string(&call).unwrap())
    }

    fn drafts(view: &TranscriptView) -> Vec<(String, bool)> {
        view.rows
            .iter()
            .filter_map(|row| match &row.body {
                Body::Draft(card, kept) => Some((card.draft.clone(), kept.is_some())),
                _ => None,
            })
            .collect()
    }

    fn images(view: &TranscriptView) -> usize {
        view.rows
            .iter()
            .filter(|row| matches!(row.body, Body::Image(_)))
            .count()
    }

    #[gpui::test]
    fn keeping_a_draft_upgrades_its_card_instead_of_adding_a_second_one(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("monocode-keep-card-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            let mut rows = vec![
                row("user", "Mock a pricing page".into()),
                card_row("draft-1-a"),
                row("assistant", "Here is a first pass.".into()),
            ];
            view.patch(0, rows.clone(), cx);
            assert_eq!(drafts(view), [("draft-1-a".to_string(), false)]);
            // The keep arrives later, as the agent's next step.
            rows.extend([
                keep_call("draft-1-a"),
                kept_row(&dir, "a"),
                row("assistant", "Saved it.".into()),
            ]);
            view.patch(3, rows[3..].to_vec(), cx);
            assert_eq!(
                drafts(view),
                [("draft-1-a".to_string(), true)],
                "one card, now carrying the picture"
            );
            assert_eq!(images(view), 0, "no second card beside it");
            // A rebuild from the top gives the same thread.
            view.rebuild(0, cx);
            assert_eq!(drafts(view), [("draft-1-a".to_string(), true)]);
            assert_eq!(images(view), 0);
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn a_kept_draft_whose_page_is_gone_shows_one_picture_card(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let dir = std::env::temp_dir().join(format!("monocode-keep-gone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| TranscriptView::new()))
                .unwrap()
        });
        window
            .update(cx, |view, _, cx| {
                view.patch(
                    0,
                    vec![
                        row("user", "Mock it".into()),
                        card_row("draft-1-gone"),
                        keep_call("draft-1-gone"),
                        kept_row(&dir, "gone"),
                        row("assistant", "Saved.".into()),
                    ],
                    cx,
                )
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("transcript-draft-card").is_some());
        assert!(visual.debug_bounds("transcript-open-draft").is_some());
        assert!(visual.debug_bounds("draft-card").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn a_picture_kept_from_another_draft_leaves_the_card_it_did_not_come_from(
        cx: &mut TestAppContext,
    ) {
        let dir = std::env::temp_dir().join(format!("monocode-keep-two-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.patch(
                0,
                vec![
                    row("user", "Mock two".into()),
                    card_row("draft-1-a"),
                    card_row("draft-1-b"),
                    keep_call("draft-1-a"),
                    kept_row(&dir, "a"),
                ],
                cx,
            );
            assert_eq!(
                drafts(view),
                [("draft-1-a".to_string(), true), ("draft-1-b".to_string(), false)]
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn two_rows_naming_one_draft_show_it_once(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.patch(
                0,
                vec![
                    row("user", "Mock it".into()),
                    card_row("draft-1-a"),
                    row("assistant", "Here.".into()),
                    row("user", "Again".into()),
                    card_row("draft-1-a"),
                    card_row("draft-1-b"),
                ],
                cx,
            );
            assert_eq!(
                drafts(view),
                [("draft-1-a".to_string(), false), ("draft-1-b".to_string(), false)]
            );
        });
    }

    #[gpui::test]
    fn a_live_card_sits_at_its_place_in_the_turn_and_scrolls_with_it(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                        gpui::point(px(0.), px(0.)),
                        gpui::size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| TranscriptView::new()),
            )
            .unwrap()
        });
        let mut rows = vec![row("user", "Mock a pricing page".into())];
        rows.push(row("assistant", "Before the page.".into()));
        rows.push(card_row("draft-1-scroll"));
        for n in 0..40 {
            rows.push(row("assistant", format!("Paragraph {n} after the page.")));
            rows.last_mut().unwrap().first = n == 0;
        }
        window
            .update(cx, |view, _, cx| view.patch(0, rows, cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let draft = visual.update(|_, cx| {
            let draft = cx.new(|cx| {
                Draft::new(
                    Some("draft-1-scroll".into()),
                    1,
                    DraftContent::Html("<p>Hi</p>".into()),
                    (Some(600), Some(200)),
                    None,
                    Some("Pricing".into()),
                    cx,
                )
            });
            crate::draft::register("draft-1-scroll", &draft, cx);
            draft
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.run_until_parked();
        // The first paints after a scroll still report the old place: settle.
        let top = |visual: &mut gpui::VisualTestContext| {
            let mut last = None;
            for _ in 0..6 {
                visual.refresh().unwrap();
                visual.run_until_parked();
                let now = visual.debug_bounds("draft-card").map(|bounds| bounds.top());
                if now == last {
                    break;
                }
                last = now;
            }
            last
        };
        let before = top(visual).expect("the card is in view");
        window
            .update(visual, |view, _, _| view.list.scroll_by(px(-30.)))
            .unwrap();
        visual.run_until_parked();
        let after = top(visual).expect("still in view after a small scroll");
        assert!(
            (before - after).abs() > px(10.),
            "the card moved with the thread: {before:?} then {after:?}"
        );
        drop(draft);
    }

    #[gpui::test]
    fn a_draft_card_shows_its_live_page_then_a_note_and_follows_its_size(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TranscriptView::new())
            })
            .unwrap()
        });
        let card = DraftCard {
            draft: "draft-1-a".into(),
            title: "Pricing".into(),
        };
        window
            .update(cx, |view, _, cx| {
                view.patch(
                    0,
                    vec![
                        row("user", "Mock a pricing page".into()),
                        row(
                            monocode_core::visual_bridge::DRAFT_ROLE,
                            serde_json::to_string(&card).unwrap(),
                        ),
                        row("assistant", "Here it is.".into()),
                    ],
                    cx,
                );
                view.rebuild(0, cx);
                assert!(
                    view.rows
                        .iter()
                        .any(|row| matches!(&row.body, Body::Draft(c, _) if c.title == "Pricing"))
                );
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // Nothing is open behind the card yet: it says so.
        assert!(visual.debug_bounds("draft-card").is_none());

        // The page opens: the card becomes the page, without a rebuild.
        let draft = visual.update(|_, cx| {
            let draft = cx.new(|cx| {
                Draft::new(
                    Some("draft-1-a".into()),
                    1,
                    DraftContent::Html("<p>Hi</p>".into()),
                    (Some(600), None),
                    None,
                    Some("Pricing".into()),
                    cx,
                )
            });
            crate::draft::register("draft-1-a", &draft, cx);
            draft
        });
        visual.run_until_parked();
        window
            .update(visual, |_, window, _| window.refresh())
            .unwrap();
        visual.run_until_parked();
        let open = visual
            .debug_bounds("draft-card")
            .expect("the card shows the live page");
        assert!(open.size.width <= px(602.));

        // Closing it takes the page away again.
        visual.update(|_, cx| crate::draft::unregister("draft-1-a", cx));
        visual.run_until_parked();
        window
            .update(visual, |_, window, _| window.refresh())
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("draft-card").is_none());
        drop(draft);
    }
}
