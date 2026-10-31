//! `MONOCODE_DEVICE_SELFTEST=1` opens one window with the Device panel beside
//! an empty canvas, streams the first booted simulator or emulator, and
//! prints one JSON line per phase of what GPUI did meanwhile:
//!
//! - `still`: the device streams but its screen does not change.
//! - `swiping`: the device's home screen is swiped, so frames flow.
//! - `animating` and `animating+swiping`: the same with a GPUI animation
//!   asking for every display frame, as a busy app would.
//!
//! Each phase reports GPUI's draws per second and their main-thread time
//! (render to paint), the Device panel's renders and its screen's paints,
//! and the frames the video layer took. Frames should flow while the panel
//! renders and paints not at all, and an animating window should keep the
//! display's full rate either way. `_SECONDS` sets each phase's length.
use super::{DevicePanel, counters};
use crate::device::stream::Input;
use gpui::{
    AnimationExt, App, Context, Entity, Window, WindowOptions, canvas, div, prelude::*, px, size,
};
use monocode_core::device::{self, hub, protocol::Phase};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Stats {
    draws: Cell<usize>,
    frame_times: RefCell<Vec<Duration>>,
}

struct Root {
    panel: Entity<DevicePanel>,
    animate: Rc<Cell<bool>>,
    stats: Rc<Stats>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let render_started = Instant::now();
        let animate = self.animate.get();
        let stats = self.stats.clone();
        div()
            .size_full()
            .flex()
            .bg(gpui::rgb(0x1b1b1d))
            .child(div().w(px(420.)).h_full().child(self.panel.clone()))
            .when(animate, |d| {
                // GPUI's own animation, which asks for every display frame.
                d.child(
                    div()
                        .relative()
                        .w(px(240.))
                        .h(px(24.))
                        .child(
                            div()
                                .absolute()
                                .size(px(24.))
                                .rounded_full()
                                .bg(gpui::rgb(0x4da3f5))
                                .with_animation(
                                    "selftest-dot",
                                    gpui::Animation::new(Duration::from_secs(1)).repeat(),
                                    |dot, delta| dot.left(px(216. * delta)),
                                ),
                        ),
                )
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, _, _| {
                        stats.draws.set(stats.draws.get() + 1);
                        stats.frame_times.borrow_mut().push(render_started.elapsed());
                    },
                )
                .flex_1()
                .h_full(),
            )
            .child(crate::draft::sentinel())
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

pub(crate) fn run() {
    let seconds = std::env::var("MONOCODE_DEVICE_SELFTEST_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(4);
    let launched = Instant::now();
    let platform = gpui_platform::current_platform(false);
    gpui::Application::with_platform(platform)
        .with_assets(crate::assets::Assets)
        .run(move |cx: &mut App| {
            // `baseline` measures the same window with devices off.
            let baseline = std::env::var("MONOCODE_DEVICE_SELFTEST").as_deref() == Ok("baseline");
            device::set_enabled(!baseline, false);
            // With `perf-trace` and MONOCODE_PERF_LOG, GPUI's own frame
            // timings go to that log; phases print their span in its clock.
            #[cfg(feature = "perf-trace")]
            crate::performance_probe::start(launched, cx);
            let panel = cx.new(DevicePanel::new);
            let animate = Rc::new(Cell::new(false));
            let stats = Rc::new(Stats::default());
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                            None,
                            size(px(820.), px(900.)),
                            cx,
                        ))),
                        // Floats above other windows: a covered window stops
                        // drawing, which would hide what is being measured.
                        kind: gpui::WindowKind::Floating,
                        ..Default::default()
                    },
                    {
                        let (panel, animate, stats) = (panel.clone(), animate.clone(), stats.clone());
                        move |_, cx| {
                            cx.new(|_| Root {
                                panel,
                                animate,
                                stats,
                            })
                        }
                    },
                )
                .expect("open the self-test window");
            // An occluded window stops drawing; measure one on screen.
            cx.activate(true);
            // Show it on whichever Space is in front, a full-screen one too:
            // a window on another Space never draws either.
            let _ = handle.update(cx, |_, window, _| {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                window.activate_window();
                if let Ok(handle) = HasWindowHandle::window_handle(window)
                    && let RawWindowHandle::AppKit(handle) = handle.as_raw()
                {
                    let view = handle.ns_view.as_ptr().cast::<objc2::runtime::AnyObject>();
                    unsafe {
                        let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![view, window];
                        // canJoinAllSpaces | fullScreenAuxiliary
                        let _: () = objc2::msg_send![ns_window, setCollectionBehavior: (1usize << 0) | (1usize << 8)];
                        let _: () = objc2::msg_send![ns_window, orderFrontRegardless];
                    }
                }
            });
            panel.update(cx, |panel, cx| panel.set_shown(true, cx));
            cx.spawn(async move |cx| {
                let executor = cx.background_executor().clone();
                let timer = |ms: u64| executor.timer(Duration::from_millis(ms));
                let waited = Instant::now();
                loop {
                    if baseline {
                        break;
                    }
                    timer(100).await;
                    let state = panel
                        .read_with(cx, |p, _| {
                            (
                                p.live.as_ref().is_some_and(|l| l.streaming),
                                p.error.clone().or_else(|| match &p.hub {
                                    super::HubState::Failed(e) => Some(e.clone()),
                                    _ => None,
                                }),
                            )
                        });
                    if state.0 {
                        break;
                    }
                    if let Some(error) = state.1.filter(|_| waited.elapsed() > Duration::from_secs(5)) {
                        println!("{}", json!({"ok": false, "error": error}));
                        cx.update(|cx| cx.quit());
                        return;
                    }
                    if waited.elapsed() > Duration::from_secs(90) {
                        println!("{}", json!({"ok": false, "error": "no picture in 90 s"}));
                        cx.update(|cx| cx.quit());
                        return;
                    }
                }
                let device = panel
                    .read_with(cx, |p, _| p.selected.clone());
                println!(
                    "{}",
                    json!({"ok": true, "device": device.map(|d| d.name), "streamingAfterMs": waited.elapsed().as_millis()})
                );
                // Let the first frames and the panel's own updates settle.
                timer(1500).await;
                for (name, animated, swiping) in [
                    ("still", false, false),
                    ("swiping", false, true),
                    ("animating", true, false),
                    ("animating+swiping", true, true),
                ] {
                    animate.set(animated);
                    let _ = handle.update(cx, |_, window, cx| {
                        window.refresh();
                        cx.notify();
                    });
                    timer(300).await;
                    stats.draws.set(0);
                    stats.frame_times.borrow_mut().clear();
                    let renders = counters::PANEL_RENDERS.load(Ordering::Relaxed);
                    let paints = counters::SCREEN_PAINTS.load(Ordering::Relaxed);
                    let frames = counters::FRAMES_QUEUED.load(Ordering::Relaxed);
                    let phase = Instant::now();
                    let length = Duration::from_secs(seconds);
                    let mut round = 0;
                    while phase.elapsed() < length {
                        if !swiping || baseline {
                            timer(100).await;
                            continue;
                        }
                        // Swipe between home screen pages, as a finger would.
                        let (from, to) = if round % 2 == 0 { (0.85, 0.15) } else { (0.15, 0.85) };
                        round += 1;
                        let send = |input: Input, cx: &mut gpui::AsyncApp| {
                            panel.read_with(cx, |p, _| p.send(input));
                        };
                        send(Input::Touch(Phase::Begin, from, 0.5), cx);
                        for step in 1..=12 {
                            timer(16).await;
                            let x = from + (to - from) * step as f64 / 12.;
                            send(Input::Touch(Phase::Move, x, 0.5), cx);
                        }
                        send(Input::Touch(Phase::End, to, 0.5), cx);
                        timer(700).await;
                    }
                    let elapsed = phase.elapsed().as_secs_f64();
                    // What macOS says about the window: drawing stops while it
                    // is occluded, and slows when it is not key.
                    let (visible, key, max_fps) = handle
                        .update(cx, |_, window, _| {
                            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                            let Ok(handle) = HasWindowHandle::window_handle(window) else {
                                return (false, false, 0);
                            };
                            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                                return (false, false, 0);
                            };
                            let view = handle.ns_view.as_ptr().cast::<objc2::runtime::AnyObject>();
                            unsafe {
                                let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![view, window];
                                let occlusion: usize = objc2::msg_send![ns_window, occlusionState];
                                let key: bool = objc2::msg_send![ns_window, isKeyWindow];
                                let screen: *mut objc2::runtime::AnyObject = objc2::msg_send![ns_window, screen];
                                let max_fps: isize = if screen.is_null() { 0 } else { objc2::msg_send![screen, maximumFramesPerSecond] };
                                (occlusion & (1 << 1) != 0, key, max_fps)
                            }
                        })
                        .unwrap_or((false, false, 0));
                    let mut times: Vec<f64> = stats
                        .frame_times
                        .borrow()
                        .iter()
                        .map(|d| d.as_secs_f64() * 1000.)
                        .collect();
                    times.sort_by(f64::total_cmp);
                    let draws = stats.draws.get();
                    let frames = counters::FRAMES_QUEUED.load(Ordering::Relaxed) - frames;
                    println!(
                        "{}",
                        json!({
                            "phase": name,
                            "windowVisible": visible,
                            "windowKey": key,
                            "screenMaxFps": max_fps,
                            "fromMs": (phase - launched).as_millis(),
                            "toMs": launched.elapsed().as_millis(),
                            "seconds": (elapsed * 10.).round() / 10.,
                            "gpuiDrawsPerSecond": (draws as f64 / elapsed).round(),
                            "frameMsP50": (percentile(&times, 0.5) * 100.).round() / 100.,
                            "frameMsP99": (percentile(&times, 0.99) * 100.).round() / 100.,
                            "frameMsMax": (times.last().copied().unwrap_or(0.) * 100.).round() / 100.,
                            "panelRenders": counters::PANEL_RENDERS.load(Ordering::Relaxed) - renders,
                            "screenPaints": counters::SCREEN_PAINTS.load(Ordering::Relaxed) - paints,
                            "videoFrames": frames,
                            "videoFps": (frames as f64 / elapsed).round(),
                        })
                    );
                }
                panel.update(cx, |panel, cx| panel.set_shown(false, cx));
                cx.background_executor()
                    .spawn(async { hub::stop() })
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
