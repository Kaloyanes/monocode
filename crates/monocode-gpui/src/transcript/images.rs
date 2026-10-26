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
        div()
            .w_full()
            .min_w_0()
            .py_3()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .id(SharedString::from(format!("image-{}", path.display())))
                    .debug_selector(|| "transcript-agent-image".into())
                    .w_full()
                    .max_w(px(width))
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
                    .child(cached.view.clone()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.55))
                    .truncate()
                    .child(title),
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
