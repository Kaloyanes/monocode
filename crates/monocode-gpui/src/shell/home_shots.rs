//! Renders the built-in Home project in a real shell with the Metal renderer
//! and writes PNGs: Home selected with a draft chat, then a code project with
//! Home first in the switcher rail.
//! `HOME_SHOTS=/tmp/shots cargo test … home_shots -- --ignored`
use super::*;
use gpui::{HeadlessAppContext, size};
use monocode_core::store::Store;

const PROJECT: &str = "/Users/kaloyanes/Dev/monocode";

#[test]
#[ignore = "Requires a Metal device; writes screenshots"]
fn home_shots() {
    let dir = std::env::var("HOME_SHOTS").unwrap_or_else(|_| "/tmp/home-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let home = monocode_core::files::home_project().unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut sessions = Vec::new();
    for (cwd, title, started) in [
        (home.as_str(), "", false),
        (home.as_str(), "Free up disk space", true),
        (home.as_str(), "Rename screenshots by date", true),
        (PROJECT, "Run Claude Agents Without Projects", true),
        (PROJECT, "Headless Chrome Visualization Support", true),
    ] {
        let mut s = store.create("claude", cwd).unwrap();
        if started {
            store.title(s.id, title).unwrap();
            s.title = title.into();
            s.has_messages = true;
        }
        sessions.push(s);
    }
    let ids: Vec<i64> = sessions.iter().map(|s| s.id).collect();
    let mut state = Workspace::default();
    state.add_project(PROJECT);
    state.project = home.clone();
    state.open(ids[0]);
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
        .open_window(size(px(1280.), px(760.)), |_, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(store, sessions, home.clone(), Some(saved), cx);
                shell.workspace.ensure_home_project();
                for id in &ids {
                    shell.ensure_session(*id, cx);
                }
                shell
            })
        })
        .unwrap();
    let frame = |cx: &mut HeadlessAppContext| {
        for _ in 0..4 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, _| w.refresh())
                .unwrap();
            cx.run_until_parked();
        }
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
        flat.save(format!("{dir}/{name}.png")).unwrap();
    };
    window
        .update(&mut cx, |shell, _, _| {
            assert_eq!(shell.workspace.projects[0], home);
            assert_eq!(shell.project_label(&home), "Home");
        })
        .unwrap();
    shot(&mut cx, "0-home");
    window
        .update(&mut cx, |shell, _, cx| {
            shell.workspace.project = PROJECT.into();
            shell.workspace.active = None;
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    shot(&mut cx, "1-project");
}
