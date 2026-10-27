//! Images an agent produced or published, kept in the conversation's folder.
//! They keep their own aspect ratio and open full screen when clicked.
use super::*;
use gpui::{AnyElement, ImageSource};
use monocode_core::transcript::images::ImageDocument;
use std::path::PathBuf;

/// Wide images shrink to the column; this caps how wide they are drawn.
const MAX_WIDTH: f32 = 560.;
/// The decoded preview's shorter side, enough for MAX_WIDTH at 2x.
const PREVIEW_SIDE: u32 = 1120;
/// Each preview holds a decoded sprite; long histories keep only recent ones.
const RETAINED_PICTURES: usize = 12;

pub(super) struct CachedPicture {
    view: gpui::Entity<Picture>,
    touched: Instant,
}

pub(super) struct Picture {
    source: Option<ImageSource>,
    missing: bool,
}

impl Picture {
    fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let work = cx.background_executor().spawn(async move {
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let pixels = monocode_core::composer::thumbnail_bgra(&bytes, PREVIEW_SIDE)?;
            Ok::<_, String>(Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                pixels,
            )])))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(sprite) => this.source = Some(ImageSource::Render(sprite)),
                    Err(_) => this.missing = true,
                }
                cx.notify();
            });
        })
        .detach();
        cx.on_release(|this, cx| {
            if let Some(ImageSource::Render(image)) = this.source.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        Self {
            source: None,
            missing: false,
        }
    }
}

impl Render for Picture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let frame = div()
            .size_full()
            .rounded_lg()
            .overflow_hidden()
            .bg(content(0.04));
        match &self.source {
            Some(source) => frame.child(
                gpui::img(source.clone())
                    .size_full()
                    .object_fit(gpui::ObjectFit::Contain),
            ),
            None if self.missing => frame
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(content(0.55))
                .child("This image is no longer available."),
            None => frame,
        }
    }
}

impl TranscriptView {
    pub(super) fn image_row(
        &mut self,
        document: Arc<ImageDocument>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = document.path.clone();
        if let Some(page) = document.draft() {
            self.watch_drafts(cx);
            // "Open draft" turns the card into the page itself.
            if let Some(draft) = crate::draft::live(&crate::draft::kept_id(&page), cx) {
                return draft.into_any_element();
            }
        }
        if !self.pictures.contains_key(&path) {
            if self.pictures.len() >= RETAINED_PICTURES
                && let Some(oldest) = self
                    .pictures
                    .iter()
                    .min_by_key(|(_, cached)| cached.touched)
                    .map(|(key, _)| key.clone())
            {
                self.pictures.remove(&oldest);
            }
            let read = path.clone();
            let view = cx.new(|cx| Picture::new(read, cx));
            self.pictures.insert(
                path.clone(),
                CachedPicture {
                    view,
                    touched: Instant::now(),
                },
            );
        }
        let cached = self.pictures.get_mut(&path).unwrap();
        cached.touched = Instant::now();
        // Image pixels are drawn at 2x, so a 1200px screenshot spans 600 points.
        let width = (document.width as f32 / 2.).clamp(48., MAX_WIDTH);
        let title: SharedString = document.title.clone().into();
        let alt = title.clone();
        let page = document.draft();
        let card_id = SharedString::from(format!("open-draft-{}", path.display()));
        let open_picture = div()
            .id(SharedString::from(format!("image-{}", path.display())))
            .debug_selector(|| "transcript-agent-image".into())
            .w_full()
            .aspect_ratio(document.width as f32 / document.height as f32)
            .role(Role::Button)
            .focusable()
            .aria_label(format!("Open {title} full screen"))
            .cursor(gpui::CursorStyle::PointingHand)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(
                    Box::new(crate::image_lightbox::Open {
                        source: crate::image_lightbox::Source::Path(path.clone()),
                        alt: alt.clone(),
                    }),
                    cx,
                );
            })
            .child(cached.view.clone());
        let Some(page) = page else {
            return div()
                .w_full()
                .min_w_0()
                .py_3()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(div().w_full().max_w(px(width)).child(open_picture))
                .child(
                    div()
                        .max_w(px(width))
                        .text_size(px(11.))
                        .text_color(content(0.55))
                        .truncate()
                        .child(title),
                )
                .into_any_element();
        };
        // A kept design draft is a card like the app's others: the picture,
        // then its name and a way back into the page in the Draft panel.
        div()
            .w_full()
            .min_w_0()
            .py_3()
            .child(
                div()
                    .debug_selector(|| "transcript-draft-card".into())
                    .w_full()
                    .max_w(px(width))
                    .flex()
                    .flex_col()
                    .rounded_xl()
                    .overflow_hidden()
                    .border_1()
                    .border_color(content(0.08))
                    .bg(content(0.03))
                    .child(
                        div()
                            .w_full()
                            .border_b_1()
                            .border_color(content(0.08))
                            .child(open_picture),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(40.))
                            .px_3()
                            .child(icon("app-window", 14.).text_color(content(0.55)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(content(0.8))
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .id(card_id)
                                    .debug_selector(|| "transcript-open-draft".into())
                                    .role(Role::Button)
                                    .aria_label("Open draft")
                                    .flex_shrink_0()
                                    .h(px(24.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(content(0.12))
                                    .text_size(px(12.))
                                    .text_color(content(0.8))
                                    .hover(|s| s.bg(content(0.06)))
                                    .cursor(gpui::CursorStyle::PointingHand)
                                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        window.dispatch_action(
                                            Box::new(crate::draft::OpenKept { page: page.clone() }),
                                            cx,
                                        );
                                    })
                                    .child("Open draft"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn image_row(title: &str, path: &str) -> Row {
        let document = ImageDocument {
            title: title.into(),
            path: path.into(),
            mime_type: "image/png".into(),
            width: 800,
            height: 400,
        };
        Row {
            role: monocode_core::transcript::images::ROLE.into(),
            text: serde_json::to_string(&document).unwrap(),
            first: true,
            sent_at: None,
        }
    }

    #[gpui::test]
    fn agent_images_stay_visible_when_work_folds_and_bad_rows_explain_themselves(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        let tool = ToolCall::new("shot", "mcp__browser__screenshot");
        let rows = vec![
            Row {
                role: "user".into(),
                text: "Take a screenshot".into(),
                first: true,
                sent_at: None,
            },
            Row {
                role: "tool".into(),
                text: serde_json::to_string(&tool).unwrap(),
                first: true,
                sent_at: None,
            },
            image_row("screenshot", "/data/threads/1/images/a.png"),
            Row {
                role: "assistant".into(),
                text: "Here it is.".into(),
                first: true,
                sent_at: None,
            },
            Row {
                role: monocode_core::transcript::images::ROLE.into(),
                text: "{broken".into(),
                first: true,
                sent_at: None,
            },
        ];
        view.update(cx, |view, cx| {
            view.patch(0, rows, cx);
            for reveal in view.reveals.values_mut() {
                reveal.settle(Instant::now());
            }
            view.rebuild(0, cx);
            assert!(view.rows.iter().any(|row| {
                matches!(&row.body, Body::Image(image) if image.title == "screenshot") && !row.work
            }));
            assert!(view.rows.iter().any(|row| {
                matches!(&row.body, Body::Plain(text) if text.as_ref() == "This image could not be loaded.")
            }));
        });
    }

    #[gpui::test]
    fn a_kept_draft_is_a_card_and_a_plain_image_stays_a_picture(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("monocode-draft-card-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kept = dir.join("a.png");
        std::fs::write(dir.join("a.html"), "<p>Hi</p>").unwrap();
        let document = |path: PathBuf| {
            Arc::new(ImageDocument {
                title: "Sign-up".into(),
                path,
                mime_type: "image/png".into(),
                width: 800,
                height: 400,
            })
        };
        assert!(document(kept.clone()).draft().is_some());
        assert!(document(dir.join("b.png")).draft().is_none());
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            let _ = view.image_row(document(kept), cx);
            let _ = view.image_row(document(dir.join("b.png")), cx);
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn retained_previews_are_bounded(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            for index in 0..RETAINED_PICTURES + 3 {
                let document = Arc::new(ImageDocument {
                    title: "Image".into(),
                    path: format!("/missing/{index}.png").into(),
                    mime_type: "image/png".into(),
                    width: 10,
                    height: 10,
                });
                let _ = view.image_row(document, cx);
            }
            assert_eq!(view.pictures.len(), RETAINED_PICTURES);
            assert!(!view.pictures.contains_key(&PathBuf::from("/missing/0.png")));
        });
    }
}
