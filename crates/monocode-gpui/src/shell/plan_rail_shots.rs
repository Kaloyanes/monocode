//! Renders the plan rail in a real shell with the Metal renderer and the
//! system text stack, and writes PNGs. No window is shown.
//! `PLAN_RAIL_SHOTS=/tmp/shots cargo test … plan_rail_shots -- --ignored`
use super::*;
use gpui::{HeadlessAppContext, size};
use monocode_core::{store::Store, transcript::Row};

const PROJECT: &str = "/Users/kaloyanes/Dev/monocode";
const W: f32 = 1440.;
const H: f32 = 860.;

fn tool_row(id: &str, name: &str, input: serde_json::Value) -> Row {
    let mut call = monocode_core::tool::ToolCall::new(id, name);
    call.input = input.to_string();
    call.state = monocode_core::tool::ToolState::Completed;
    Row {
        role: "tool".into(),
        text: serde_json::to_string(&call).unwrap(),
        first: true,
        sent_at: None,
    }
}

#[test]
#[ignore = "Requires a Metal device; writes screenshots"]
fn plan_rail_shots() {
    let dir = std::env::var("PLAN_RAIL_SHOTS").unwrap_or_else(|_| "/tmp/plan-rail-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut ids = Vec::new();
    let mut sessions = Vec::new();
    for (provider, title, transcript) in crate::fixtures::plans() {
        let mut s = store.create(provider, PROJECT).unwrap();
        store.title(s.id, title).unwrap();
        store.write_rows(s.id, 0, &transcript.rows).unwrap();
        s.title = title.into();
        s.has_messages = true;
        ids.push(s.id);
        sessions.push(s);
    }
    let mut state = Workspace::default();
    state.add_project(PROJECT);
    for id in &ids {
        state.open(*id);
    }
    state.sidebar_visible = true;
    state.right_panel_open = false;
    state.preferences.title_plan = true;
    let saved = serde_json::to_string(&state).unwrap();
    let store = Arc::new(Mutex::new(store));

    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::popover::bind(cx);
    });
    let window = cx
        .open_window(size(px(W), px(H)), |_, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(store, sessions, PROJECT.into(), Some(saved), cx);
                for id in &ids {
                    shell.ensure_session(*id, cx);
                }
                shell
            })
        })
        .unwrap();
    let settle = |cx: &mut HeadlessAppContext| {
        for _ in 0..6 {
            cx.run_until_parked();
            cx.advance_clock(std::time::Duration::from_millis(400));
            cx.update_window(window.into(), |_, w, _| w.refresh())
                .unwrap();
        }
        cx.run_until_parked();
    };
    let shot = |cx: &mut HeadlessAppContext, name: &str| {
        settle(cx);
        // A real harness version check may have found an update; it is not
        // what these pictures are about.
        window
            .update(cx, |shell, _, cx| {
                shell
                    .notification_state
                    .harness
                    .update(cx, |h, cx| h.clear(cx));
            })
            .unwrap();
        settle(cx);
        let image = cx.capture_screenshot(window.into()).unwrap();
        // The window is translucent glass; flatten it over the chat colour.
        let mut flat = image::RgbaImage::from_pixel(
            image.width(),
            image.height(),
            image::Rgba([20, 20, 20, 255]),
        );
        image::imageops::overlay(&mut flat, &image, 0, 0);
        flat.save(format!("{dir}/{name}.png")).unwrap();
        let bar = image::imageops::crop_imm(&flat, 0, 0, flat.width(), 640).to_image();
        bar.save(format!("{dir}/{name}-bar.png")).unwrap();
    };
    let open = |cx: &mut HeadlessAppContext, id: i64| {
        window
            .update(cx, |shell, w, cx| {
                shell.open(id, w, cx);
                shell
                    .plan_rail
                    .update(cx, |rail, cx| rail.set_open(false, cx));
                shell.invalidate_chrome(cx);
            })
            .unwrap();
    };
    let popover = |cx: &mut HeadlessAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .plan_rail
                    .update(cx, |rail, cx| rail.set_open(true, cx));
            })
            .unwrap();
    };
    let live = |cx: &mut HeadlessAppContext, id: i64, calls: &[(&str, &str)]| {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&id].update(cx, |chat, cx| {
                    let rows: Vec<Row> = calls
                        .iter()
                        .enumerate()
                        .map(|(i, (name, target))| {
                            let key = if *name == "Bash" {
                                "command"
                            } else {
                                "file_path"
                            };
                            tool_row(
                                &format!("live-{id}-{i}"),
                                name,
                                serde_json::json!({ key: target }),
                            )
                        })
                        .collect();
                    let start = chat.transcript.read(cx).source_rows().len();
                    chat.transcript.update(cx, |t, cx| t.patch(start, rows, cx));
                    chat.busy = true;
                    cx.notify();
                });
            })
            .unwrap();
    };
    let [mid, revised, done, stale, none] = ids[..] else {
        panic!("five plan fixtures")
    };

    // The same session with the setting off, for comparison.
    window
        .update(&mut cx, |shell, w, cx| {
            shell.workspace.preferences.title_plan = false;
            shell.open(mid, w, cx);
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    shot(&mut cx, "00-tabs-before");
    window
        .update(&mut cx, |shell, _, cx| {
            shell.workspace.preferences.title_plan = true;
            shell.invalidate_chrome(cx);
        })
        .unwrap();

    open(&mut cx, mid);
    shot(&mut cx, "01-in-progress");
    live(
        &mut cx,
        mid,
        &[
            ("Read", "crates/monocode-gpui/src/quick_composer.rs"),
            ("Edit", "crates/monocode-gpui/src/quick_composer.rs"),
            ("Bash", "cargo check -p monocode-gpui"),
        ],
    );
    shot(&mut cx, "02-live-ticks");
    popover(&mut cx);
    shot(&mut cx, "03-live-list");
    open(&mut cx, revised);
    shot(&mut cx, "04-revised");
    popover(&mut cx);
    shot(&mut cx, "05-revised-list");
    open(&mut cx, done);
    shot(&mut cx, "06-done");
    open(&mut cx, stale);
    shot(&mut cx, "07-stale");
    open(&mut cx, none);
    shot(&mut cx, "08-no-plan");
    live(
        &mut cx,
        none,
        &[
            ("Read", "src-tauri/src/fs.rs"),
            ("Bash", "cargo clippy"),
            ("Read", "src-tauri/src/inbox_media.rs"),
            ("Edit", "src-tauri/src/fs.rs"),
            ("Bash", "cargo clippy"),
        ],
    );
    shot(&mut cx, "09-no-plan-live");
}
