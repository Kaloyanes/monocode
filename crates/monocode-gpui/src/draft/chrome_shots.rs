//! Draws a Chromium draft card with the Metal renderer, clicks and types into
//! it as the person would, lays a GPUI popover over it, and writes PNGs.
//! `CHROMIUM_SHOTS=/tmp/shots MONOCODE_CEF_APP=… cargo test --profile
//! native-dev chromium_card_shots -- --ignored`
use super::Draft;
use crate::theme::*;
use gpui::{
    Entity, HeadlessAppContext, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, PlatformInput, Point, Window, div, point, prelude::*, px, size,
};
use monocode_core::visual_bridge::DraftContent;
use std::{sync::Arc, time::Duration};

struct Root {
    draft: Entity<Draft>,
    popover: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(pane_surface())
            .p_6()
            .child(div().w(px(640.)).child(self.draft.clone()))
            .when(self.popover, |root| {
                root.child(
                    div()
                        .absolute()
                        .top(px(110.))
                        .left(px(300.))
                        .w(px(280.))
                        .p_4()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(content(0.12))
                        .bg(floating_fill())
                        .shadow_lg()
                        .text_size(px(13.))
                        .text_color(content(1.))
                        .child("A GPUI popover, drawn over the Chromium page."),
                )
            })
            .child(super::sentinel())
    }
}

#[test]
#[ignore = "Requires a Metal device and a headless Chromium; writes screenshots"]
fn chromium_card_shots() {
    let dir = std::env::var("CHROMIUM_SHOTS").unwrap_or_else(|_| "/tmp/chromium-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: an ignored test run on its own; nothing else reads the
    // environment meanwhile.
    unsafe { std::env::set_var("MONOCODE_DRAFT_ENGINE", "chromium") };
    assert!(
        crate::chromium::installed(),
        "set MONOCODE_CEF_APP to a CEF host bundle"
    );
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| crate::popover::bind(cx));
    let html = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/native/examples/draft-spike.html"),
    )
    .unwrap();
    let (width, height) = (720u32, 560u32);
    let window = cx
        .open_window(size(px(width as f32), px(height as f32)), |_, cx| {
            let draft = cx.new(|cx| {
                Draft::new(
                    None,
                    1,
                    DraftContent::Html(html),
                    (Some(600), None),
                    Some(false),
                    Some("Chromium draft".into()),
                    cx,
                )
            });
            cx.new(|_| Root {
                draft,
                popover: false,
            })
        })
        .unwrap();
    // Chromium answers on its own thread and the app's timers run on the
    // test clock, so both move forward together.
    let pump = |cx: &mut HeadlessAppContext, millis: u64| {
        for _ in 0..millis / 40 {
            std::thread::sleep(Duration::from_millis(40));
            cx.advance_clock(Duration::from_millis(40));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, _| window.refresh())
                .unwrap();
            cx.run_until_parked();
        }
    };
    let shot = |cx: &mut HeadlessAppContext, name: &str| {
        let image = cx.capture_screenshot(window.into()).unwrap();
        let mut flat = image::RgbaImage::from_pixel(
            image.width(),
            image.height(),
            image::Rgba([20, 20, 20, 255]),
        );
        image::imageops::overlay(&mut flat, &image, 0, 0);
        flat.save(format!("{dir}/{name}.png")).unwrap();
    };
    pump(&mut cx, 4000);
    shot(&mut cx, "1-card");

    // Where the page is drawn, to click the way the person would.
    let placed = window
        .update(&mut cx, |root, _, cx| root.draft.read(cx).placement())
        .unwrap()
        .expect("the card painted the page");
    let scale = placed[2] as f32 / 600.;
    let at = |x: f32, y: f32| {
        point(
            px(placed[0] as f32 + x * scale),
            px(placed[1] as f32 + y * scale),
        )
    };
    let click = |cx: &mut HeadlessAppContext, position: Point<gpui::Pixels>| {
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        })
        .unwrap();
    };
    // "Add one", then the name field.
    click(&mut cx, at(60., 85.));
    pump(&mut cx, 400);
    click(&mut cx, at(300., 130.));
    pump(&mut cx, 400);
    for letter in ["A", "d", "a"] {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                shift: letter == "A",
                ..Modifiers::default()
            },
            key: letter.to_lowercase(),
            key_char: Some(letter.into()),
        };
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
        })
        .unwrap();
        pump(&mut cx, 120);
    }
    pump(&mut cx, 800);
    shot(&mut cx, "2-clicked-and-typed");

    window
        .update(&mut cx, |root, _, cx| {
            root.popover = true;
            cx.notify();
        })
        .unwrap();
    pump(&mut cx, 400);
    shot(&mut cx, "3-popover");
}

/// A conversation in the background: the window draws something else.
struct Elsewhere;
impl Render for Elsewhere {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child(super::sentinel())
    }
}

/// The agent opens a page while its conversation is not on screen: the
/// card is never painted, and the page still has to load and answer.
#[test]
#[ignore = "Requires a Metal device and a headless Chromium"]
fn chromium_draft_answers_before_its_card_is_painted() {
    // SAFETY: an ignored test run on its own.
    unsafe { std::env::set_var("MONOCODE_DRAFT_ENGINE", "chromium") };
    assert!(crate::chromium::installed(), "set MONOCODE_CEF_APP");
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    let window = cx
        .open_window(size(px(800.), px(600.)), |_, cx| cx.new(|_| Elsewhere))
        .unwrap();
    let content = DraftContent::Html("<button>Unseen</button>".into());
    let (reply, replies) = std::sync::mpsc::channel();
    let request = monocode_core::visual_bridge::DraftRequest {
        session: 1,
        draft: None,
        content: Some(content.clone()),
        action: Default::default(),
        width: None,
        height: None,
        light: None,
        title: None,
        id: None,
        reply,
    };
    let (send, result) = async_channel::bounded(1);
    window
        .update(&mut cx, |_, window, cx| {
            let draft = cx.new(|cx| Draft::new(None, 1, content, (None, None), None, None, cx));
            let lease = draft.update(cx, |draft, cx| draft.lease(window, cx));
            let weak = draft.downgrade();
            cx.spawn(async move |_, cx| {
                let outcome = Draft::run(&weak, &request, cx).await;
                drop(lease);
                drop(draft);
                let _ = send.send(outcome.map(|_| ())).await;
            })
            .detach();
        })
        .unwrap();
    let started = std::time::Instant::now();
    let outcome = loop {
        std::thread::sleep(Duration::from_millis(40));
        cx.advance_clock(Duration::from_millis(40));
        cx.run_until_parked();
        if let Ok(outcome) = result.try_recv() {
            break outcome;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "no answer");
    };
    drop(replies);
    assert_eq!(outcome, Ok(()));
}
