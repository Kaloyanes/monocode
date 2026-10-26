//! A session joining or leaving the sidebar: the rows below travel from where
//! they were painted to their new places one after another, top first, and a
//! new row fades in behind them as they make room.
use crate::reveal::bezier;
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, point, px,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

const PUSH: Duration = Duration::from_millis(320);
const STAGGER: Duration = Duration::from_millis(22);
/// Rows further down share the last delay, so a long list never trails.
const MAX_RANK: u32 = 10;
const FADE: Duration = Duration::from_millis(240);
const FADE_DELAY: Duration = Duration::from_millis(40);
/// The new row drops this far into its slot while it fades in.
const RISE: f32 = 8.;
const LENGTH: Duration =
    Duration::from_millis(PUSH.as_millis() as u64 + STAGGER.as_millis() as u64 * MAX_RANK as u64);

struct Run {
    start: Instant,
    entering: Option<usize>,
    /// Painted top before the change and stagger rank, by new list index.
    moved: HashMap<usize, (Pixels, u32)>,
    /// Distance each row starts away from its new place, taken on its first
    /// paint after the change.
    from: HashMap<usize, f32>,
}

#[cfg(test)]
thread_local! {
    static FROZEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Holds every cascade at its first frame, so tests can inspect it.
#[cfg(test)]
pub(super) fn freeze(frozen: bool) {
    FROZEN.set(frozen);
}

impl Run {
    fn elapsed(&self) -> Duration {
        #[cfg(test)]
        if FROZEN.get() {
            return Duration::ZERO;
        }
        self.start.elapsed()
    }
}

fn progress(elapsed: Duration, delay: Duration, length: Duration) -> f32 {
    let t = elapsed.saturating_sub(delay).as_secs_f32() / length.as_secs_f32();
    bezier(t.min(1.), 0.2, 0., 0., 1.)
}

#[derive(Clone, Default)]
pub(super) struct ListCascade(Rc<RefCell<Option<Run>>>);

impl ListCascade {
    pub(super) fn start(&self, entering: Option<usize>, moved: HashMap<usize, (Pixels, u32)>) {
        *self.0.borrow_mut() = (entering.is_some() || !moved.is_empty()).then(|| Run {
            start: Instant::now(),
            entering,
            moved,
            from: HashMap::new(),
        });
    }

    pub(super) fn cancel(&self) {
        self.0.borrow_mut().take();
    }

    pub(super) fn active(&self) -> bool {
        let mut run = self.0.borrow_mut();
        if run.as_ref().is_some_and(|r| r.elapsed() >= LENGTH) {
            *run = None;
        }
        run.is_some()
    }

    /// Opacity of the row at `ix`; only the entering row is below 1.
    pub(super) fn opacity(&self, ix: usize) -> f32 {
        match &*self.0.borrow() {
            Some(run) if run.entering == Some(ix) => progress(run.elapsed(), FADE_DELAY, FADE),
            _ => 1.,
        }
    }

    fn offset(&self, ix: usize, top: Pixels) -> f32 {
        let mut run = self.0.borrow_mut();
        let Some(run) = run.as_mut() else {
            return 0.;
        };
        let elapsed = run.elapsed();
        if run.entering == Some(ix) {
            return -RISE * (1. - progress(elapsed, FADE_DELAY, FADE));
        }
        let Some(&(before, rank)) = run.moved.get(&ix) else {
            return 0.;
        };
        let from = *run.from.entry(ix).or_insert(f32::from(before - top));
        from * (1. - progress(elapsed, STAGGER * rank.min(MAX_RANK), PUSH))
    }

    pub(super) fn wrap(&self, ix: usize, child: AnyElement) -> AnyElement {
        Shift {
            cascade: self.clone(),
            ix,
            child,
        }
        .into_any_element()
    }
}

struct Shift {
    cascade: ListCascade,
    ix: usize,
    child: AnyElement,
}

impl IntoElement for Shift {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Shift {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if cx.reduce_motion() {
            self.cascade.cancel();
        }
        // Whole device pixels keep glyphs from shimmering while they move.
        let scale = window.scale_factor();
        let offset = (self.cascade.offset(self.ix, bounds.top()) * scale).round() / scale;
        window.with_element_offset(point(px(0.), px(offset)), |window| {
            self.child.prepaint(window, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(cascade: &ListCascade, age: Duration) {
        if let Some(run) = cascade.0.borrow_mut().as_mut() {
            run.start = Instant::now() - age;
        }
    }

    #[test]
    fn rows_below_start_where_they_were_and_settle_top_first() {
        let cascade = ListCascade::default();
        let moved = HashMap::from([(2, (px(100.), 0)), (3, (px(140.), 1))]);
        cascade.start(Some(1), moved);
        // The new row pushed both rows down by 52px.
        assert_eq!(cascade.offset(2, px(152.)).round(), -52.);
        assert_eq!(cascade.offset(3, px(192.)).round(), -52.);
        assert_eq!(cascade.offset(1, px(100.)).round(), -RISE);
        assert!(cascade.opacity(1) < 0.01);
        assert_eq!(cascade.opacity(2), 1.);
        aged(&cascade, Duration::from_millis(120));
        let (upper, lower) = (cascade.offset(2, px(152.)), cascade.offset(3, px(192.)));
        assert!(upper > -52. && lower > -52. && lower < upper);
        assert!(cascade.opacity(1) > 0.);
        aged(&cascade, LENGTH);
        assert_eq!(cascade.offset(3, px(192.)), 0.);
        assert!(!cascade.active());
        assert_eq!(cascade.offset(3, px(192.)), 0.);
    }

    #[test]
    fn a_removal_slides_the_rows_below_up() {
        let cascade = ListCascade::default();
        cascade.start(None, HashMap::from([(1, (px(152.), 0))]));
        assert_eq!(cascade.offset(1, px(100.)).round(), 52.);
        // Scrolling after the first paint does not change the distance.
        assert_eq!(cascade.offset(1, px(60.)).round(), 52.);
        assert_eq!(cascade.offset(0, px(48.)), 0.);
    }
}
