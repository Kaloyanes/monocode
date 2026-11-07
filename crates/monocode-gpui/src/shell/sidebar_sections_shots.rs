//! Renders the sidebar's Working and Archived sections in a real shell with
//! the Metal renderer, and writes PNGs. No window is shown.
//! `SIDEBAR_SHOTS=/tmp/shots cargo test … sidebar_sections_shots -- --ignored`
use super::*;
use gpui::{HeadlessAppContext, size};
use monocode_core::store::Store;

const PROJECT: &str = "/Users/kaloyanes/Dev/monocode";

#[test]
#[ignore = "Requires a Metal device; writes screenshots"]
fn sidebar_sections_shots() {
    let dir = std::env::var("SIDEBAR_SHOTS").unwrap_or_else(|_| "/tmp/sidebar-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut sessions = Vec::new();
    let now = monocode_core::time::now_ms();
    for (ix, (title, archived)) in [
        ("Audit Laggy Thread Scrolling", false),
        ("Remove Items and Add Animations", false),
        ("Use Monocode Visualization Tool", false),
        ("Create a Visualization", false),
        ("Empty State Display Issue", false),
        ("Fix Worktree Cleanup", true),
        ("Tune Composer Motion", true),
        ("Update Mobile Pairing", true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut s = store.create("claude", PROJECT).unwrap();
        store.title(s.id, title).unwrap();
        s.title = title.into();
        s.has_messages = true;
        s.archived = archived;
        s.updated_at = now - ix as u64 * 3_600_000;
        sessions.push(s);
    }
    let ids: Vec<_> = sessions.iter().map(|s| s.id).collect();
    let mut state = Workspace::default();
    state.add_project(PROJECT);
    state.open(ids[2]);
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
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::popover::bind(cx);
    });
    let shell_ids = ids.clone();
    let window = cx
        .open_window(size(px(1100.), px(760.)), |_, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(store, sessions, PROJECT.into(), Some(saved), cx);
                for id in &shell_ids {
                    shell.ensure_session(*id, cx);
                }
                for id in &shell_ids[..4] {
                    shell.sessions[id].update(cx, |s, _| s.busy = true);
                }
                // [0] leads [1] and [3]; [2] is a thread it opened; [4] is its own.
                shell.thread_parents.insert(shell_ids[1], shell_ids[0]);
                shell.thread_parents.insert(shell_ids[3], shell_ids[0]);
                shell.started_by.insert(shell_ids[2], shell_ids[0]);
                shell
            })
        })
        .unwrap();
    let shot = |cx: &mut HeadlessAppContext, name: &str, change: &dyn Fn(&mut Shell)| {
        window
            .update(cx, |shell, _, cx| {
                change(shell);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        for _ in 0..6 {
            cx.run_until_parked();
            cx.advance_clock(std::time::Duration::from_millis(400));
            cx.update_window(window.into(), |_, w, _| w.refresh())
                .unwrap();
        }
        cx.run_until_parked();
        let image = cx.capture_screenshot(window.into()).unwrap();
        let mut flat = image::RgbaImage::from_pixel(
            image.width(),
            image.height(),
            image::Rgba([20, 20, 20, 255]),
        );
        image::imageops::overlay(&mut flat, &image, 0, 0);
        let width = flat.width().min(860);
        image::imageops::crop_imm(&flat, 0, 0, width, flat.height())
            .to_image()
            .save(format!("{dir}/{name}.png"))
            .unwrap();
    };
    for (name, classic) in [("focused", false), ("classic", true)] {
        shot(&mut cx, &format!("{name}-list"), &|s| {
            s.workspace.classic_layout = classic;
            s.workspace.preferences.docked_sections = false;
            s.archive_view = None;
        });
        shot(&mut cx, &format!("{name}-archive"), &|s| {
            s.archive_view = Some(PROJECT.into());
        });
        shot(&mut cx, &format!("{name}-docked"), &|s| {
            s.archive_view = None;
            s.workspace.preferences.docked_sections = true;
        });
        shot(&mut cx, &format!("{name}-docked-open"), &|s| {
            s.archive_dock_open = true;
        });
        shot(&mut cx, &format!("{name}-reset"), &|s| {
            s.archive_dock_open = false;
        });
    }
}
