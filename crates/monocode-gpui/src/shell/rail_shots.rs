//! Renders an orchestrator's status rail in a real shell with the Metal
//! renderer, still and while it cascades open, and writes PNGs of the sidebar.
//! `RAIL_SHOTS=/tmp/shots cargo test … rail_shots -- --ignored`
use super::*;
use gpui::{HeadlessAppContext, size};
use monocode_core::{orchestration::SummaryTask, store::Store};

const PROJECT: &str = "/Users/kaloyanes/Dev/monocode";

#[test]
#[ignore = "Requires a Metal device; writes screenshots"]
fn rail_shots() {
    let dir = std::env::var("RAIL_SHOTS").unwrap_or_else(|_| "/tmp/rail-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut sessions = Vec::new();
    for title in [
        "Orchestrator Thread Creation",
        "Luna test run",
        "model id probe",
        "luna probe b",
        "Haiku test run",
        "Plain thread",
    ] {
        let mut s = store.create("claude", PROJECT).unwrap();
        store.title(s.id, title).unwrap();
        s.title = title.into();
        s.has_messages = true;
        sessions.push(s);
    }
    let ids: Vec<i64> = sessions.iter().map(|s| s.id).collect();
    let mut state = Workspace::default();
    state.add_project(PROJECT);
    state.sidebar_visible = true;
    state.right_panel_open = false;
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
    cx.update(|cx| crate::popover::bind(cx));
    let window = cx
        .open_window(size(px(1100.), px(560.)), |_, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(store, sessions, PROJECT.into(), Some(saved), cx);
                for id in &ids {
                    shell.ensure_session(*id, cx);
                }
                shell
            })
        })
        .unwrap();
    let lead = ids[0];
    let tasks = |mixed: bool| monocode_core::orchestration::Summary {
        live: mixed,
        tasks: ids[1..5]
            .iter()
            .zip([
                ("Luna test run", "codex", "completed"),
                (
                    "model id probe",
                    "claude",
                    if mixed { "running" } else { "completed" },
                ),
                (
                    "luna probe b",
                    "codex",
                    if mixed { "failed" } else { "completed" },
                ),
                ("Haiku test run", "claude", "completed"),
            ])
            .map(|(id, (title, provider, status))| SummaryTask {
                session: Some(*id),
                title: title.into(),
                provider: provider.into(),
                model: None,
                status: status.into(),
            })
            .collect(),
    };
    let set = |cx: &mut HeadlessAppContext, mixed: bool, active: i64| {
        window
            .update(cx, |shell, _, cx| {
                shell.orchestration_summaries.insert(lead, tasks(mixed));
                // Turns long enough to show a run time.
                for (n, id) in ids[1..5].iter().enumerate() {
                    shell.sessions[id].update(cx, |chat, _| {
                        chat.settings.turns.insert(
                            0,
                            monocode_core::agent::TurnInfo {
                                started_at: Some(1),
                                completed_at: Some(1 + 48_000 * (n as u64 + 1)),
                                duration_ms: 48_000 * (n as u64 + 1) + 14_000,
                                duration_known: true,
                                ..Default::default()
                            },
                        );
                    });
                }
                shell.workspace.active = Some(active);
                shell.invalidate_chrome(cx);
            })
            .unwrap();
    };
    let frame = |cx: &mut HeadlessAppContext| {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, _| w.refresh())
            .unwrap();
        cx.run_until_parked();
    };
    let shot = |cx: &mut HeadlessAppContext, name: &str| {
        frame(cx);
        let image = cx.capture_screenshot(window.into()).unwrap();
        let mut flat = image::RgbaImage::from_pixel(
            image.width(),
            image.height(),
            image::Rgba([20, 20, 20, 255]),
        );
        image::imageops::overlay(&mut flat, &image, 0, 0);
        let side = image::imageops::crop_imm(&flat, 0, 0, 640, flat.height()).to_image();
        side.save(format!("{dir}/{name}.png")).unwrap();
    };
    let settle = |cx: &mut HeadlessAppContext| {
        for _ in 0..4 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            frame(cx);
        }
    };
    set(&mut cx, false, ids[5]);
    settle(&mut cx);
    shot(&mut cx, "0-closed");
    set(&mut cx, false, lead);
    frame(&mut cx);
    // Rail motion runs on the wall clock; pin it to a moment of the opening
    // just before each frame is painted.
    for at in [60u64, 120, 180, 260] {
        let started = std::time::Instant::now() - std::time::Duration::from_millis(at);
        window
            .update(&mut cx, |shell, _, cx| {
                if let Some(motion) = shell.rails.motions.get_mut(&lead) {
                    motion.start = started;
                }
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        cx.update_window(window.into(), |_, w, _| w.refresh())
            .unwrap();
        let painted = std::time::Instant::now();
        let image = cx.capture_screenshot(window.into()).unwrap();
        eprintln!("{at}ms frame took {:?}", painted.elapsed());
        let mut flat = image::RgbaImage::from_pixel(
            image.width(),
            image.height(),
            image::Rgba([20, 20, 20, 255]),
        );
        image::imageops::overlay(&mut flat, &image, 0, 0);
        image::imageops::crop_imm(&flat, 0, 640, 640, 440)
            .to_image()
            .save(format!("{dir}/1-opening-{at}ms.png"))
            .unwrap();
    }
    settle(&mut cx);
    shot(&mut cx, "2-open");
    set(&mut cx, true, lead);
    settle(&mut cx);
    shot(&mut cx, "3-mixed");
    window
        .update(&mut cx, |shell, _, cx| {
            shell.change_sidebar_option("compact-session-cards", cx);
        })
        .unwrap();
    settle(&mut cx);
    shot(&mut cx, "4-detailed");
}
