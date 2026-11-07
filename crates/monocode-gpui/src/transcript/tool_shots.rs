//! Renders a transcript holding chart, draft and ordinary tool rows with the
//! Metal renderer and writes a PNG, to see the MonoCode logo on its own rows.
//! `TOOL_SHOTS=/tmp/tool-shots cargo test … tool_shots -- --ignored`
use super::*;
use gpui::{HeadlessAppContext, size};
use monocode_core::transcript::Row;

#[test]
#[ignore = "Requires a Metal device; writes screenshots"]
fn tool_shots() {
    let dir = std::env::var("TOOL_SHOTS").unwrap_or_else(|_| "/tmp/tool-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| cx.set_reduce_motion(true));
    let (width, height) = (760u32, 520u32);
    let window = cx
        .open_window(size(px(width as f32), px(height as f32)), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap();
    window
        .update(&mut cx, |view, _, cx| {
            let row = |role: &str, text: &str| Row {
                role: role.into(),
                text: text.into(),
                first: true,
                sent_at: None,
            };
            let tool = |id: &str, name: &str, input: &str| {
                let mut call = ToolCall::new(id, name);
                call.input = input.into();
                call.state = ToolState::Completed;
                row("tool", &serde_json::to_string(&call).unwrap())
            };
            view.patch(
                0,
                vec![
                    row("user", "Chart my commits, then mock a landing page"),
                    tool("g1", "Bash", r#"{"command":"git log --oneline | wc -l"}"#),
                    tool("c1", "mcp__monocode__visual_docs", r#"{}"#),
                    tool("c2", "mcp__monocode__visual_check", r#"{"source":"x"}"#),
                    tool(
                        "c3",
                        "mcp__monocode__visual_show",
                        r#"{"title":"Commits per week","source":"x"}"#,
                    ),
                    row("assistant", "Here is the chart."),
                    row("user", "Now the landing page"),
                    tool(
                        "d1",
                        "mcp__monocode__draft_preview",
                        r#"{"html":"<p>x</p>","title":"Coffee hero"}"#,
                    ),
                    tool(
                        "d2",
                        "mcp__monocode__draft_preview",
                        r#"{"draft":"d","action":{"kind":"click","reference":"1"}}"#,
                    ),
                    tool(
                        "d3",
                        "mcp__monocode__draft_preview",
                        r#"{"draft":"d","publish":true}"#,
                    ),
                    row("assistant", "The page is above."),
                ],
                cx,
            );
            for group in 0..12 {
                view.work_folds.insert(group, true);
                view.phases.insert(group, true);
            }
            view.rebuild(0, cx);
        })
        .unwrap();
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, _| w.refresh())
            .unwrap();
        cx.run_until_parked();
    }
    let image = cx.capture_screenshot(window.into()).unwrap();
    let mut flat = image::RgbaImage::from_pixel(
        image.width(),
        image.height(),
        image::Rgba([20, 20, 20, 255]),
    );
    image::imageops::overlay(&mut flat, &image, 0, 0);
    flat.save(format!("{dir}/tools.png")).unwrap();
}
