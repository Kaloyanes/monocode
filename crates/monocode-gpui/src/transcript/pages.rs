//! Pages the agent opens in the thread: the live draft behind a card, or a
//! plain note once it is gone.
use super::*;
use gpui::AnyElement;
use monocode_core::visual_bridge::DraftCard;

impl TranscriptView {
    /// Cards change height when their page opens, resizes or closes.
    pub(super) fn watch_drafts(&mut self, cx: &mut Context<Self>) {
        if self.draft_watch.is_some() {
            return;
        }
        self.draft_watch = Some(crate::draft::watch(cx, |this, cx| {
            for (ix, row) in this.rows.iter().enumerate() {
                if matches!(row.body, Body::Draft(_) | Body::Image(_)) {
                    this.list.remeasure_items(ix..ix + 1);
                }
            }
            cx.notify();
        }));
    }

    pub(super) fn draft_row(&mut self, card: &DraftCard, cx: &mut Context<Self>) -> AnyElement {
        self.watch_drafts(cx);
        if let Some(draft) = crate::draft::live(&card.draft, cx) {
            return draft.into_any_element();
        }
        let name = if card.title.is_empty() {
            "Draft"
        } else {
            &card.title
        };
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
                        .any(|row| matches!(&row.body, Body::Draft(c) if c.title == "Pricing"))
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
