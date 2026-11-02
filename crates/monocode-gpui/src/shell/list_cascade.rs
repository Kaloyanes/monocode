//! A session joining or leaving the sidebar: the rows below travel from where
//! they were painted to their new places one after another, top first, and a
//! new row fades in behind them as they make room.
//! A session that only changes place dims and slips behind the rows it
//! passes, then comes forward in its new slot.
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
/// A reordered row's trip from its old slot to its new one.
const TRAVEL: Duration = Duration::from_millis(560);
/// How far the reordered row dims while it is behind the others.
const DIM: f32 = 0.4;
/// Fractions of the trip at which it has finished dimming and starts to
/// come forward again.
const DIMMED_BY: f32 = 0.22;
const SURFACES_AT: f32 = 0.72;
const LENGTH: Duration = Duration::from_millis(
    // The longer of the stagger and the reordered row's trip.
    max_u64(
        PUSH.as_millis() as u64 + STAGGER.as_millis() as u64 * MAX_RANK as u64,
        TRAVEL.as_millis() as u64,
    ),
);

const fn max_u64(a: u64, b: u64) -> u64 {
    if a > b { a } else { b }
}

struct Run {
    start: Instant,
    entering: Option<usize>,
    /// A session that moved without joining or leaving, by new list index.
    reordered: Option<usize>,
    /// Opacity a row was painted at when an earlier run was cut short, by new
    /// list index; it fades up from there.
    fades: HashMap<usize, f32>,
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

fn travel(elapsed: Duration) -> f32 {
    bezier(
        (elapsed.as_secs_f32() / TRAVEL.as_secs_f32()).min(1.),
        0.4,
        0.,
        0.2,
        1.,
    )
}

/// 1 at either end of the trip, `DIM` while the row is behind the others.
fn reordered_opacity(elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / TRAVEL.as_secs_f32()).min(1.);
    let behind = if t < DIMMED_BY {
        t / DIMMED_BY
    } else if t < SURFACES_AT {
        1.
    } else {
        (1. - t) / (1. - SURFACES_AT)
    };
    1. - (1. - DIM) * bezier(behind, 0.4, 0., 0.2, 1.)
}

#[derive(Clone, Default)]
pub(super) struct ListCascade(Rc<RefCell<Option<Run>>>);

impl ListCascade {
    pub(super) fn start(
        &self,
        entering: Option<usize>,
        reordered: Option<usize>,
        moved: HashMap<usize, (Pixels, u32)>,
        fades: HashMap<usize, f32>,
    ) {
        *self.0.borrow_mut() = (entering.is_some() || !moved.is_empty()).then(|| Run {
            start: Instant::now(),
            entering,
            reordered,
            fades,
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

    /// Opacity of the row at `ix`; only the entering and reordered rows are
    /// below 1.
    pub(super) fn opacity(&self, ix: usize) -> f32 {
        match &*self.0.borrow() {
            Some(run) if run.entering == Some(ix) => progress(run.elapsed(), FADE_DELAY, FADE),
            Some(run) if run.reordered == Some(ix) => reordered_opacity(run.elapsed()),
            Some(run) => run.fades.get(&ix).map_or(1., |from| {
                from + (1. - from) * progress(run.elapsed(), Duration::ZERO, FADE)
            }),
            None => 1.,
        }
    }

    /// How far the row at `ix` is painted from its layout `top` right now and
    /// at what opacity, so a run that replaces this one can start from there.
    pub(super) fn carried(&self, ix: usize, top: Pixels) -> (f32, f32) {
        if !self.active() {
            return (0., 1.);
        }
        (self.offset(ix, top), self.opacity(ix))
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
        if run.reordered == Some(ix) {
            return from * (1. - travel(elapsed));
        }
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
        cascade.start(Some(1), None, moved, HashMap::new());
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
        cascade.start(
            None,
            None,
            HashMap::from([(1, (px(152.), 0))]),
            HashMap::new(),
        );
        assert_eq!(cascade.offset(1, px(100.)).round(), 52.);
        // Scrolling after the first paint does not change the distance.
        assert_eq!(cascade.offset(1, px(60.)).round(), 52.);
        assert_eq!(cascade.offset(0, px(48.)), 0.);
    }

    #[test]
    fn a_reordered_row_dims_behind_the_rows_it_passes() {
        let cascade = ListCascade::default();
        // The row at slot 3 moved to slot 0; the three above it stepped down.
        let moved = HashMap::from([
            (0, (px(180.), 0)),
            (1, (px(0.), 0)),
            (2, (px(60.), 1)),
            (3, (px(120.), 2)),
        ]);
        cascade.start(None, Some(0), moved, HashMap::new());
        assert_eq!(cascade.offset(0, px(0.)).round(), 180.);
        assert!(cascade.opacity(0) > 0.99);
        assert_eq!(cascade.opacity(1), 1.);
        aged(&cascade, Duration::from_millis(280));
        let mid = cascade.opacity(0);
        assert!((mid - DIM).abs() < 0.01, "dimmed mid-trip: {mid}");
        let offset = cascade.offset(0, px(0.));
        assert!(offset > 0. && offset < 180.);
        aged(&cascade, Duration::from_millis(540));
        assert!(cascade.opacity(0) > mid);
        aged(&cascade, LENGTH);
        assert_eq!(cascade.offset(0, px(0.)), 0.);
        assert!(!cascade.active());
        assert_eq!(cascade.opacity(0), 1.);
    }

    #[test]
    fn a_run_cut_short_hands_its_painted_state_to_the_next() {
        let cascade = ListCascade::default();
        // A row was pushed down by 52px and is on its way back.
        cascade.start(
            None,
            None,
            HashMap::from([(1, (px(152.), 0))]),
            HashMap::new(),
        );
        assert_eq!(cascade.offset(1, px(100.)).round(), 52.);
        aged(&cascade, Duration::from_millis(120));
        let (offset, opacity) = cascade.carried(1, px(100.));
        assert!(offset > 0. && offset < 52., "mid-flight: {offset}");
        assert_eq!(opacity, 1.);
        // The next run starts where the row is painted, not where it was laid out.
        cascade.start(
            None,
            None,
            HashMap::from([(1, (px(100.) + px(offset), 0))]),
            HashMap::new(),
        );
        assert!((cascade.offset(1, px(100.)) - offset).abs() < 0.5);
        // A row that was fading in keeps fading from where it was.
        cascade.start(
            None,
            None,
            HashMap::from([(2, (px(0.), 0))]),
            HashMap::from([(2, 0.3)]),
        );
        assert!((cascade.opacity(2) - 0.3).abs() < 0.01);
        aged(&cascade, FADE);
        assert!((cascade.opacity(2) - 1.).abs() < 0.01);
        assert_eq!(cascade.carried(9, px(0.)).1, 1.);
    }
}
