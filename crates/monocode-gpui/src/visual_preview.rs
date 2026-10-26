//! Renders an agent's HTML page offscreen so it can look at the result before
//! publishing: a screenshot, the page's console output, and the height it wants.
use gpui::App;
use monocode_core::visual_bridge::{self, PreviewRequest};

pub(crate) fn start(cx: &mut App) {
    let (send, receive) = async_channel::unbounded::<PreviewRequest>();
    visual_bridge::set_preview_handler(move |request| {
        let _ = send.try_send(request);
    });
    // Requests run one at a time on the main thread, where the web view lives.
    cx.spawn(async move |cx| {
        while let Ok(request) = receive.recv().await {
            let result = render(&request, cx).await;
            let _ = request.reply.send(result);
        }
    })
    .detach();
    #[cfg(debug_assertions)]
    selftest(cx);
}

/// `MONOCODE_PREVIEW_SELFTEST=page.html` renders that page once the app is up,
/// writes `page.html.png` and `page.html.json`, and quits. Debug builds only.
#[cfg(debug_assertions)]
fn selftest(cx: &mut App) {
    let Some(path) = std::env::var_os("MONOCODE_PREVIEW_SELFTEST") else {
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(3))
            .await;
        let html = std::fs::read_to_string(&path).unwrap_or_default();
        // As the MCP tools do before a page reaches the app.
        let data = std::env::var("MONOCODE_PREVIEW_SELFTEST_DATA")
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default();
        let html = monocode_core::visual::images::embed_images(&html)
            .and_then(|html| monocode_core::visual::data::embed_data(&html, &data))
            .unwrap_or_else(|error| {
                eprintln!("{error}");
                html
            });
        let (reply, _) = std::sync::mpsc::channel();
        let request = PreviewRequest {
            html,
            width: 640,
            height: 480,
            light: std::env::var("MONOCODE_PREVIEW_SELFTEST_APPEARANCE")
                .ok()
                .map(|appearance| appearance == "light"),
            reply,
        };
        let outcome = render(&request, cx).await;
        let mut json = path.clone();
        json.push(".json");
        match outcome {
            Ok(preview) => {
                let mut png = path.clone();
                png.push(".png");
                let _ = std::fs::write(png, &preview.png);
                let _ = std::fs::write(
                    json,
                    serde_json::json!({
                        "width": preview.width, "height": preview.height,
                        "contentHeight": preview.content_height,
                        "loaded": preview.loaded, "console": preview.console,
                    })
                    .to_string(),
                );
            }
            Err(error) => {
                let _ = std::fs::write(json, serde_json::json!({"error": error}).to_string());
            }
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

#[cfg(not(target_os = "macos"))]
async fn render(
    _: &PreviewRequest,
    _: &mut gpui::AsyncApp,
) -> Result<visual_bridge::Preview, String> {
    Err("Previews are only available on macOS".into())
}

#[cfg(target_os = "macos")]
use mac::render;

#[cfg(target_os = "macos")]
mod mac {
    use super::PreviewRequest;
    use crate::monos::html::Palette;
    use block2::RcBlock;
    use monocode_core::visual_bridge::Preview;
    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::{
        NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSDictionary, NSError, NSNumber, NSPoint, NSRect, NSSize, NSString};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView, WKWebViewConfiguration};
    use std::time::{Duration, Instant};

    const LOAD_TIMEOUT: Duration = Duration::from_secs(12);
    /// Entrance animations and late layout settle before the picture is taken.
    const SETTLE: Duration = Duration::from_millis(900);

    async fn wait<T>(
        receive: &async_channel::Receiver<T>,
        limit: Duration,
        cx: &gpui::AsyncApp,
    ) -> Option<T> {
        let started = Instant::now();
        loop {
            if let Ok(value) = receive.try_recv() {
                return Some(value);
            }
            if started.elapsed() > limit {
                return None;
            }
            cx.background_executor()
                .timer(Duration::from_millis(30))
                .await;
        }
    }

    /// The wrapper's state as JSON: `{log, loaded, h}`.
    async fn state(web: &WKWebView, cx: &gpui::AsyncApp) -> Option<serde_json::Value> {
        let text =
            crate::monos::html::evaluate(web, "window.__mc ? JSON.stringify(window.__mc) : ''", cx)
                .await?;
        serde_json::from_str(&text).ok()
    }

    pub(super) async fn render(
        request: &PreviewRequest,
        cx: &mut gpui::AsyncApp,
    ) -> Result<Preview, String> {
        let mtm = MainThreadMarker::new().ok_or("Previews must render on the main thread")?;
        let (width, height) = (f64::from(request.width), f64::from(request.height));
        let palette = request.light.map_or_else(Palette::current, Palette::forced);
        let markup = crate::monos::html::preview_page(&request.html, palette);
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(width, height));
        // A web view only paints inside a window that counts as visible, so
        // the page lives in a borderless window that is on screen but
        // transparent and deaf to the mouse.
        let scale;
        let (window, web) = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            scale = window.backingScaleFactor();
            window.setAlphaValue(0.01);
            window.setIgnoresMouseEvents(true);
            window.setHasShadow(false);
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&objc2_web_kit::WKWebsiteDataStore::nonPersistentDataStore(
                mtm,
            ));
            let web = WKWebView::initWithFrame_configuration(mtm.alloc(), rect, &config);
            let _: () = objc2::msg_send![&*web, _setDrawsBackground: false];
            window
                .contentView()
                .ok_or("The preview window has no content view")?
                .addSubview(&web);
            window.orderFrontRegardless();
            web.loadHTMLString_baseURL(&NSString::from_str(&markup), None);
            (window, web)
        };
        let outcome = capture(&web, mtm, width / scale, cx).await;
        window.orderOut(None);
        window.close();
        let (png, size, json) = outcome?;
        let console = json["log"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some((
                    entry["level"].as_str()?.to_owned(),
                    entry["text"].as_str()?.to_owned(),
                ))
            })
            .collect();
        let content = json["h"].as_u64().unwrap_or(0) as u32;
        Ok(Preview {
            png,
            width: size.0,
            height: size.1,
            content_height: if content == 0 {
                request.height
            } else {
                content
            },
            loaded: json["loaded"].as_bool().unwrap_or(false),
            console,
        })
    }

    async fn capture(
        web: &Retained<WKWebView>,
        mtm: MainThreadMarker,
        width: f64,
        cx: &gpui::AsyncApp,
    ) -> Result<(Vec<u8>, (u32, u32), serde_json::Value), String> {
        let started = Instant::now();
        loop {
            if state(web, cx)
                .await
                .is_some_and(|state| state["loaded"] == true)
            {
                break;
            }
            if started.elapsed() > LOAD_TIMEOUT {
                // Still report what there is; a page that never finishes loading
                // usually has a blocked script or a stuck request.
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
        }
        cx.background_executor().timer(SETTLE).await;
        let json = state(web, cx).await.unwrap_or_default();
        let (send, receive) = async_channel::bounded(1);
        let block = RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
            let encoded = unsafe { image.as_ref() }
                .and_then(|image| image.TIFFRepresentation())
                .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                .and_then(|rep| {
                    let size = (rep.pixelsWide() as u32, rep.pixelsHigh() as u32);
                    let data = unsafe {
                        rep.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        )
                    }?;
                    Some((data.to_vec(), size))
                });
            let _ = send.try_send(encoded);
        });
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            config.setSnapshotWidth(Some(&NSNumber::new_f64(width)));
            web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
        }
        let (png, size) = wait(&receive, Duration::from_secs(8), cx)
            .await
            .flatten()
            .ok_or("The page could not be captured")?;
        Ok((png, size, json))
    }
}
