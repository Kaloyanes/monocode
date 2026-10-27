//! `MONOCODE_DRAFT_SELFTEST=page.html` opens one window that hosts only the
//! Draft view, runs the page through the same request path the agent's tools
//! use, and writes `page.html.N.png` and `page.html.N.json` for every step.
//!
//! `MONOCODE_DRAFT_ACTIONS` is a JSON array of further `draft_preview` inputs;
//! `"$draft"` stands for the draft's id. `{"debug":"takeover"}` plays the part
//! of the person clicking in the page; `{"debug":"note"}` pins a note as they
//! would, `"send"` sends it and `"send_later"` sends it a second from now. A click's `reference` written as
//! `@Name` means the control with that name in the latest snapshot. `_WIDTH`, `_HEIGHT` and `_APPEARANCE`
//! shape the first step, `_URL` opens a dev server instead of the file, and
//! `_HOLD` keeps the window open that many seconds for a screen capture.
use super::Draft;
use gpui::{
    App, Context, Entity, Window, WindowBackgroundAppearance, WindowOptions, div, prelude::*, px,
    size,
};
use monocode_core::visual_bridge::{DraftReply, DraftRequest};
use serde_json::{Value, json};
use std::{ffi::OsString, time::Duration};

/// The draft on its own, or inside a thread when `_THREAD` is set.
struct Root(gpui::AnyView, #[allow(dead_code)] Entity<Draft>);
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

fn env_number(name: &str) -> Option<u32> {
    std::env::var(name).ok()?.parse().ok()
}

pub(crate) fn run(path: OsString) {
    let platform = gpui_platform::current_platform(false);
    gpui::Application::with_platform(platform)
        .with_assets(crate::assets::Assets)
        .run(move |cx: &mut App| {
            crate::popover::bind(cx);
            let mut first = json!({});
            if let Ok(url) = std::env::var("MONOCODE_DRAFT_SELFTEST_URL") {
                first["url"] = url.into();
            } else {
                let html = std::fs::read_to_string(&path).unwrap_or_default();
                first["html"] = monocode_core::visual_bridge::embed_draft_images(&html)
                    .unwrap_or_else(|error| {
                        eprintln!("{error}");
                        html
                    })
                    .into();
            }
            for (key, name) in [
                ("width", "MONOCODE_DRAFT_SELFTEST_WIDTH"),
                ("height", "MONOCODE_DRAFT_SELFTEST_HEIGHT"),
            ] {
                if let Some(value) = env_number(name) {
                    first[key] = value.into();
                }
            }
            if let Ok(appearance) = std::env::var("MONOCODE_DRAFT_SELFTEST_APPEARANCE") {
                first["appearance"] = appearance.into();
            }
            let steps: Vec<Value> = std::env::var("MONOCODE_DRAFT_ACTIONS")
                .ok()
                .and_then(|actions| serde_json::from_str(&actions).ok())
                .unwrap_or_default();
            let (reply, _) = std::sync::mpsc::channel();
            let Ok(opening) = DraftRequest::parse(1, &first, reply) else {
                eprintln!("The first request is invalid");
                cx.quit();
                return;
            };
            let content = opening.content.clone().expect("the first step has content");
            let draft = cx.new(|cx| {
                Draft::new(
                    None,
                    1,
                    content,
                    (opening.width, opening.height),
                    opening.light,
                    None,
                    cx,
                )
            });
            let window_width = env_number("MONOCODE_DRAFT_SELFTEST_PANEL").unwrap_or(460) as f32;
            let root: gpui::AnyView = if std::env::var("MONOCODE_DRAFT_SELFTEST_THREAD").is_ok() {
                let id = draft.read(cx).id().to_owned();
                crate::draft::register(&id, &draft, cx);
                let card = monocode_core::visual_bridge::DraftCard {
                    draft: id,
                    title: "Pricing page".into(),
                };
                crate::transcript::pages::selftest_thread(&card, cx).into()
            } else {
                draft.clone().into()
            };
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                            None,
                            size(px(window_width), px(780.)),
                            cx,
                        ))),
                        window_background: WindowBackgroundAppearance::Blurred,
                        ..Default::default()
                    },
                    {
                        let keep = draft.clone();
                        move |_, cx| cx.new(|_| Root(root, keep))
                    },
                )
                .expect("open the self-test window");
            let weak = draft.downgrade();
            // A window that nobody is looking at stops drawing; keep frames
            // coming so the page is placed the way a visible panel would be.
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(120))
                        .await;
                    if handle.update(cx, |_, window, _| window.refresh()).is_err() {
                        return;
                    }
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                // The window has to paint once before the page has a view.
                cx.background_executor().timer(Duration::from_secs(1)).await;
                // The draft already holds the first page; the first step looks
                // at it rather than loading it again.
                let mut id = weak
                    .read_with(cx, |draft, _| draft.id().to_owned())
                    .unwrap_or_default();
                let mut opening = json!({"draft": id});
                for key in ["width", "height", "appearance"] {
                    if let Some(value) = first.get(key) {
                        opening[key] = value.clone();
                    }
                }
                let mut named: Vec<(String, String)> = Vec::new();
                let mut all = vec![opening];
                all.extend(steps);
                // A new page follows its content's height, as it does when the
                // agent opens one.
                if let Ok(web) = Draft::ready(&weak, cx).await {
                    Draft::fit_height(&weak, &web, cx).await;
                }
                for (n, mut input) in all.into_iter().enumerate() {
                    let outcome = if let Some(what) = input.get("debug").and_then(Value::as_str) {
                        Draft::debug(&weak, what, cx).await
                    } else {
                        if let Some(draft) = input.get_mut("draft")
                            && draft == "$draft"
                        {
                            *draft = id.clone().into();
                        }
                        if let Some(reference) = input.pointer_mut("/action/reference")
                            && let Some(name) = reference.as_str().and_then(|r| r.strip_prefix('@'))
                            && let Some((_, found)) = named.iter().find(|(n, _)| n == name)
                        {
                            *reference = found.clone().into();
                        }
                        let (reply, _) = std::sync::mpsc::channel();
                        match DraftRequest::parse(1, &input, reply) {
                            Ok(request) => Draft::run(&weak, &request, cx).await.map(|reply| {
                                if let DraftReply::Snapshot(snapshot) = &reply {
                                    id = snapshot.draft.clone();
                                    named = snapshot
                                        .elements
                                        .iter()
                                        .map(|e| (e.name.clone(), e.reference.clone()))
                                        .collect();
                                    let _ = std::fs::write(
                                        format!("{}.{n}.png", path.to_string_lossy()),
                                        &snapshot.png,
                                    );
                                }
                                match reply {
                                    DraftReply::Snapshot(snapshot) => {
                                        serde_json::to_value(&*snapshot).unwrap_or_default()
                                    }
                                    DraftReply::Closed { draft } => json!({"closed": draft}),
                                    DraftReply::Kept { png, html, .. } => {
                                        json!({"kept": {"png": png, "html": html}})
                                    }
                                }
                            }),
                            Err(error) => Err(error),
                        }
                    };
                    let mut json = match outcome {
                        Ok(value) => value,
                        Err(error) => json!({"error": error}),
                    };
                    json["placed"] = weak
                        .read_with(cx, |draft, _| draft.placement())
                        .ok()
                        .flatten()
                        .map_or(Value::Null, |frame| json!(frame));
                    let _ = std::fs::write(
                        format!("{}.{n}.json", path.to_string_lossy()),
                        json.to_string(),
                    );
                }
                if let Some(hold) = env_number("MONOCODE_DRAFT_SELFTEST_HOLD") {
                    cx.background_executor()
                        .timer(Duration::from_secs(hold.into()))
                        .await;
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
