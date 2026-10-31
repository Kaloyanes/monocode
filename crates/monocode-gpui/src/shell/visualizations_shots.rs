//! Renders Settings › Visualizations in a real shell with the Metal renderer.
//! `VISUALIZATIONS_SHOTS=/tmp/shots cargo test … visualizations_shots -- --ignored`
use super::*;
use crate::settings::Section;
use gpui::{HeadlessAppContext, size};
use monocode_core::store::Store;

const PROJECT: &str = "/Users/kaloyanes/Dev/monocode";

#[test]
#[ignore = "Requires a Metal device; writes screenshots"]
fn visualizations_shots() {
    let dir = std::env::var("VISUALIZATIONS_SHOTS")
        .unwrap_or_else(|_| "/tmp/visualizations-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("claude", PROJECT).unwrap();
    let id = session.id;
    let mut state = Workspace::default();
    state.add_project(PROJECT);
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
    let (width, height) = (1100u32, 720u32);
    let window = cx
        .open_window(size(px(width as f32), px(height as f32)), |_, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(store, vec![session], PROJECT.into(), Some(saved), cx);
                shell.ensure_session(id, cx);
                shell.workspace.active = Some(id);
                shell
            })
        })
        .unwrap();
    window
        .update(&mut cx, |shell, window, cx| {
            shell.open_settings(Section::Visualizations, window, cx)
        })
        .unwrap();
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(120));
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
    flat.save(format!("{dir}/visualizations.png")).unwrap();
}
