//! Remembers where the main window was when MonoCode last closed.

use gpui::{Bounds, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "window-bounds";
pub const MIN_WIDTH: f32 = 820.;
pub const MIN_HEIGHT: f32 = 520.;
/// Less than this much of the window on a display and the title bar is out of
/// reach, so the saved position is dropped.
const MIN_VISIBLE: f32 = 160.;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
struct Saved {
    mode: Mode,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Windowed,
    Maximized,
    Fullscreen,
}

pub fn encode(bounds: WindowBounds) -> String {
    let (mode, b) = match bounds {
        WindowBounds::Windowed(b) => (Mode::Windowed, b),
        WindowBounds::Maximized(b) => (Mode::Maximized, b),
        WindowBounds::Fullscreen(b) => (Mode::Fullscreen, b),
    };
    serde_json::to_string(&Saved {
        mode,
        x: b.origin.x.as_f32(),
        y: b.origin.y.as_f32(),
        width: b.size.width.as_f32(),
        height: b.size.height.as_f32(),
    })
    .unwrap_or_default()
}

/// The saved bounds, if they still land on one of `displays`. A monitor that
/// was unplugged or rearranged since the last run falls back to the default.
pub fn decode(raw: &str, displays: &[Bounds<gpui::Pixels>]) -> Option<WindowBounds> {
    let s: Saved = serde_json::from_str(raw).ok()?;
    let finite = [s.x, s.y, s.width, s.height].iter().all(|v| v.is_finite());
    if !finite || s.width < MIN_WIDTH || s.height < MIN_HEIGHT {
        return None;
    }
    let visible = displays.iter().any(|d| {
        let (left, top) = (d.origin.x.as_f32(), d.origin.y.as_f32());
        let (right, bottom) = (left + d.size.width.as_f32(), top + d.size.height.as_f32());
        let w = (s.x + s.width).min(right) - s.x.max(left);
        let h = (s.y + s.height).min(bottom) - s.y.max(top);
        w >= MIN_VISIBLE && h >= MIN_VISIBLE
    });
    if !visible {
        return None;
    }
    let bounds = Bounds::new(point(px(s.x), px(s.y)), size(px(s.width), px(s.height)));
    Some(match s.mode {
        Mode::Windowed => WindowBounds::Windowed(bounds),
        Mode::Maximized => WindowBounds::Maximized(bounds),
        Mode::Fullscreen => WindowBounds::Fullscreen(bounds),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn round_trips_every_mode() {
        let screen = [rect(0., 0., 1920., 1080.)];
        for bounds in [
            WindowBounds::Windowed(rect(100., 80., 1180., 800.)),
            WindowBounds::Maximized(rect(100., 80., 1180., 800.)),
            WindowBounds::Fullscreen(rect(100., 80., 1180., 800.)),
        ] {
            assert_eq!(decode(&encode(bounds), &screen), Some(bounds));
        }
    }

    #[test]
    fn drops_bounds_on_a_missing_display() {
        let saved = encode(WindowBounds::Windowed(rect(2000., 100., 1180., 800.)));
        assert_eq!(decode(&saved, &[rect(0., 0., 1920., 1080.)]), None);
        assert!(decode(&saved, &[rect(1920., 0., 1920., 1080.)]).is_some());
    }

    #[test]
    fn drops_bounds_that_hang_mostly_off_screen() {
        let saved = encode(WindowBounds::Windowed(rect(1800., 100., 1180., 800.)));
        assert_eq!(decode(&saved, &[rect(0., 0., 1920., 1080.)]), None);
    }

    #[test]
    fn rejects_garbage_and_undersized_windows() {
        let screen = [rect(0., 0., 1920., 1080.)];
        assert_eq!(decode("", &screen), None);
        assert_eq!(decode("{\"mode\":\"windowed\"}", &screen), None);
        let tiny = encode(WindowBounds::Windowed(rect(0., 0., 300., 200.)));
        assert_eq!(decode(&tiny, &screen), None);
    }
}
