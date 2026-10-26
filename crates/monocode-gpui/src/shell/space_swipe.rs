//! The sidebar's spaces sit side by side like pages, the way Arc's do. A
//! two-finger sideways swipe drags the strip, showing the neighbouring space's
//! real header and sessions beside the current one. Letting go hands the
//! fingers' speed to a spring that settles on whichever page the swipe was
//! heading for. Switching spaces any other way slides the same strip.
use super::*;
use crate::pane_motion::PaneSlide;
use gpui::{NavigationDirection, ScrollDelta, ScrollWheelEvent, Task, TouchPhase};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The settle spring's stiffness: critically damped, it lands in about 0.3s.
const SETTLE_FREQUENCY: f32 = 22.;
/// Travel before a gesture commits to an axis, so a slightly diagonal
/// vertical scroll stays a scroll.
const AXIS_SLOP: f32 = 6.;
/// A swipe lands on the neighbour when, carried on at its release speed for
/// this long, it would pass half the sidebar.
const PROJECTION: f32 = 0.15;
const COMMIT_FRACTION: f32 = 0.5;
const MIN_COMMIT_TRAVEL: f32 = 16.;
/// Release speed is the travel across the last few samples, not one gap.
const VELOCITY_WINDOW: Duration = Duration::from_millis(80);
/// The spaces bar's pill trails the pages by this time constant, so it reads
/// as pulled along by the swipe rather than running ahead of it.
const PILL_LAG: f32 = 0.05;
/// AppKit reports a cancelled scroll as one more move, never an end, so a
/// gesture with no events for this long is treated as released.
const IDLE_RELEASE: Duration = Duration::from_millis(180);

/// The per-project state a sidebar page draws from. The live page's sits in
/// `Shell` itself; a neighbour's is swapped in only while it draws.
pub(super) struct SidebarPage {
    list: sidebar_list::SidebarList,
    rails: rails::Rails,
}

impl SidebarPage {
    fn new(cx: &mut Context<Shell>) -> Self {
        Self {
            list: sidebar_list::SidebarList::new(cx),
            rails: Default::default(),
        }
    }
}

/// The strip returning to rest, and the space it just left, which stays
/// beside it on that side until it is out of view.
struct Settle {
    spring: PaneSlide,
    companion: Option<(String, f32)>,
}

#[derive(Default)]
pub(super) struct SpaceSwipe {
    /// The locked axis of the current gesture: `Some(true)` sideways.
    axis: Option<bool>,
    pending: (f32, f32),
    /// Raw finger travel; positive pulls the strip to the right.
    travel: f32,
    samples: VecDeque<(Instant, f32)>,
    last: Option<Instant>,
    /// The fingers lifted. Momentum events that follow are swallowed until
    /// the next touch so a flick can't step twice.
    lifted: bool,
    idle: Option<Task<()>>,
    settle: Option<Settle>,
    pages: HashMap<String, SidebarPage>,
    /// A neighbour page is drawing with its state swapped in.
    drawing_neighbour: bool,
    scale: f32,
    /// The spaces bar's strip, so its selection pill can travel between slots.
    pub(super) spaces_strip: AnchorSlot,
    /// Where the pill was last drawn, along the bar, and when.
    pill: Option<(f32, Instant)>,
    pill_chasing: bool,
    /// The way a step that wrapped past an end is travelling. The pill is
    /// then tracked in a frame where the new space sits one slot beyond the
    /// end it came in from, and the old space leaves across the other.
    wrap: Option<f32>,
}

/// Where the spaces bar shows the sidebar.
pub(super) struct PagerPosition {
    /// The selection pill's slot, fractional while it travels.
    pub shown: f32,
    /// While looping past an end, the pill leaving across the opposite one:
    /// its slot, and how far the incoming pill has arrived (0 to 1).
    pub leaving: Option<(f32, f32)>,
}

impl SpaceSwipe {
    pub(super) fn animating(&self) -> bool {
        self.settle.is_some() || self.pill_chasing
    }

    pub(super) fn dragging(&self) -> bool {
        self.axis == Some(true) && !self.lifted
    }

    /// The pages are moving under the pointer, so rows it rests on are only
    /// passing by.
    pub(super) fn moving(&self) -> bool {
        self.dragging() || self.settle.is_some()
    }

    pub(super) fn drawing_neighbour(&self) -> bool {
        self.drawing_neighbour
    }

    pub(super) fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// Neighbour lists rebuild with the live one when session data changes.
    pub(super) fn invalidate_pages(&self) {
        for page in self.pages.values() {
            page.list.invalidate_structure();
        }
    }

    fn velocity(&self) -> f32 {
        let (Some(&(first, from)), Some(&(last, to))) = (self.samples.front(), self.samples.back())
        else {
            return 0.;
        };
        let dt = last.duration_since(first).as_secs_f32();
        if dt < 0.008 { 0. } else { (to - from) / dt }
    }
}

/// Follows the fingers 1:1 towards a neighbour and resists past either end.
fn pulled(travel: f32, width: f32, open: bool) -> f32 {
    if open {
        return travel.clamp(-width, width);
    }
    let limit = width * 0.55;
    travel.signum() * limit * travel.abs() / (limit + travel.abs())
}

impl Shell {
    fn swipe_neighbour(&self, step: isize) -> Option<(usize, String)> {
        let projects = &self.workspace.projects;
        let ix = projects.iter().position(|p| *p == self.workspace.project)? as isize + step;
        (ix >= 0)
            .then(|| projects.get(ix as usize).map(|p| (ix as usize, p.clone())))
            .flatten()
    }

    /// The step a swipe of this travel heads towards: dragging the strip left
    /// brings in the next space from the right.
    fn swipe_step(travel: f32) -> isize {
        if travel < 0. { 1 } else { -1 }
    }

    fn sidebar_page_width(&self) -> f32 {
        self.workspace.sidebar_width.max(1.)
    }

    /// Where the live page sits this frame. A finished settle lets go of the
    /// neighbour pages it no longer needs.
    fn space_pager_offset(&mut self, now: Instant) -> f32 {
        let width = self.sidebar_page_width();
        if self.space_swipe.dragging() {
            let open = self
                .swipe_neighbour(Self::swipe_step(self.space_swipe.travel))
                .is_some();
            return pulled(self.space_swipe.travel, width, open);
        }
        if let Some(settle) = &self.space_swipe.settle {
            let (offset, done) = settle.spring.at(now);
            if !done {
                return offset;
            }
            self.space_swipe.settle = None;
            self.prune_sidebar_pages();
        }
        0.
    }

    /// Keeps only the pages a swipe from here could show.
    fn prune_sidebar_pages(&mut self) {
        let keep: Vec<_> = [-1, 1]
            .into_iter()
            .filter_map(|step| self.swipe_neighbour(step).map(|(_, p)| p))
            .collect();
        self.space_swipe.pages.retain(|p, _| keep.contains(p));
    }

    /// The space showing beside the live page at this offset, and its side.
    fn space_pager_companion(&self, offset: f32) -> Option<(String, f32)> {
        if offset.abs() < 0.5 {
            return None;
        }
        let side = if offset < 0. { 1. } else { -1. };
        if let Some(settle) = &self.space_swipe.settle
            && let Some((project, at)) = &settle.companion
            && *at == side
        {
            return Some((project.clone(), side));
        }
        self.swipe_neighbour(side as isize).map(|(_, p)| (p, side))
    }

    /// Where the spaces bar shows the sidebar, fractional while the strip is
    /// between pages. It follows the pages' position a moment behind.
    pub(super) fn space_pager_position(&mut self, cx: &App) -> Option<PagerPosition> {
        let ix = self
            .workspace
            .projects
            .iter()
            .position(|p| *p == self.workspace.project)?;
        let now = cx.background_executor().now();
        let target = ix as f32 - self.space_pager_offset(now) / self.sidebar_page_width();
        let swipe = &mut self.space_swipe;
        let shown = match swipe.pill {
            Some((pill, at)) if !cx.reduce_motion() => {
                let dt = now.saturating_duration_since(at).as_secs_f32();
                let shown = pill + (target - pill) * (1. - (-dt / PILL_LAG).exp());
                if (shown - target).abs() < 0.002 {
                    target
                } else {
                    shown
                }
            }
            _ => target,
        };
        swipe.pill = Some((shown, now));
        swipe.pill_chasing = shown != target;
        let at_rest = shown == target && swipe.settle.is_none() && !swipe.dragging();
        if at_rest {
            swipe.wrap = None;
        }
        let count = self.workspace.projects.len() as f32;
        let leaving = self.space_swipe.wrap.map(|direction| {
            (
                shown + direction * count,
                (1. - (shown - ix as f32).abs()).clamp(0., 1.),
            )
        });
        Some(PagerPosition { shown, leaving })
    }

    /// Hands the sidebar from `previous` to the newly selected project. The
    /// previous page's state is kept, so it can draw beside the new one at
    /// the same scroll position, and a neighbour that was already drawing
    /// becomes the live page as it was.
    pub(super) fn hand_over_sidebar_page(
        &mut self,
        previous: String,
        direction: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        let mut page = self
            .space_swipe
            .pages
            .remove(&self.workspace.project)
            .unwrap_or_else(|| SidebarPage::new(cx));
        std::mem::swap(&mut self.sidebar_list, &mut page.list);
        std::mem::swap(&mut self.rails, &mut page.rails);
        self.space_swipe.wrap = None;
        self.space_swipe.pages.insert(previous.clone(), page);
        // Its rows carry over, but hit targets were only kept for the old
        // live page, so the adopted list registers its own again.
        self.sidebar_list.invalidate_structure();
        self.space_swipe.settle = match direction {
            Some(direction) if !cx.reduce_motion() => {
                let from = direction * self.sidebar_page_width();
                Some(Settle {
                    spring: PaneSlide::thrown(from, 0., -from * 10., SETTLE_FREQUENCY),
                    companion: Some((previous, -direction)),
                })
            }
            _ => None,
        };
        if self.space_swipe.settle.is_none() {
            self.prune_sidebar_pages();
        }
        self.sidebar_frame.update(cx, |_, cx| cx.notify());
    }

    /// A step that wrapped past either end still slides the way it moved, and
    /// the spaces bar's pill loops with it.
    pub(super) fn point_space_pager(&mut self, direction: f32, wrapped: bool) {
        let width = self.sidebar_page_width();
        let Some(settle) = &mut self.space_swipe.settle else {
            return;
        };
        let Some((_, side)) = &mut settle.companion else {
            return;
        };
        let from = direction * width;
        settle.spring = PaneSlide::thrown(from, 0., -from * 10., SETTLE_FREQUENCY);
        *side = -direction;
        if wrapped {
            self.wrap_pill(direction);
        }
    }

    /// Re-bases the pill into the looping frame, so it carries on the way it
    /// was heading instead of sweeping back across the bar.
    fn wrap_pill(&mut self, direction: f32) {
        let count = self.workspace.projects.len() as f32;
        let swipe = &mut self.space_swipe;
        if swipe.wrap.is_none()
            && let Some((pill, _)) = &mut swipe.pill
        {
            *pill -= direction * count;
        }
        swipe.wrap = Some(direction);
    }

    /// Whether stepping from the live space runs past an end of the list.
    pub(super) fn wraps(&self, step: isize) -> bool {
        self.workspace.projects.len() > 1 && self.swipe_neighbour(step).is_none()
    }

    /// A mouse's back and forward buttons over the sidebar step to the
    /// neighbouring space, looping past either end. A press while the pages
    /// are still settling carries on from where they are. Returns whether the
    /// press was taken; otherwise it falls through to session history.
    pub(super) fn step_space(
        &mut self,
        step: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace.classic_layout
            || self.settings.is_some()
            || self.workspace.projects.len() < 2
        {
            return false;
        }
        if self.menu.is_some() || self.space_swipe.dragging() {
            return true;
        }
        let wrapped = self.wraps(step);
        let Some(path) = adjacent(
            &self.workspace.projects,
            Some(&self.workspace.project),
            step,
        )
        .filter(|path| *path != self.workspace.project) else {
            return false;
        };
        let now = cx.background_executor().now();
        let current = self
            .space_swipe
            .settle
            .as_ref()
            .map_or(0., |settle| settle.spring.at(now).0);
        self.hovered_space = None;
        self.select_project(path, window, cx);
        let width = self.sidebar_page_width();
        if let Some(settle) = &mut self.space_swipe.settle {
            let from = current + step as f32 * width;
            settle.spring = PaneSlide::thrown(from, 0., -from * 10., SETTLE_FREQUENCY);
            if let Some((_, side)) = &mut settle.companion {
                *side = -step as f32;
            }
            if wrapped {
                self.wrap_pill(step as f32);
            }
        }
        true
    }

    /// Draws with `project`'s page state standing in for the live one.
    fn in_sidebar_page<R>(
        &mut self,
        project: &str,
        cx: &mut Context<Self>,
        draw: impl FnOnce(&mut Self, &mut Context<Self>) -> R,
    ) -> R {
        if project == self.workspace.project || self.space_swipe.drawing_neighbour {
            return draw(self, cx);
        }
        let mut page = self
            .space_swipe
            .pages
            .remove(project)
            .unwrap_or_else(|| SidebarPage::new(cx));
        std::mem::swap(&mut self.sidebar_list, &mut page.list);
        std::mem::swap(&mut self.rails, &mut page.rails);
        let live = std::mem::replace(&mut self.workspace.project, project.to_owned());
        self.space_swipe.drawing_neighbour = true;
        let drawn = draw(self, cx);
        self.space_swipe.drawing_neighbour = false;
        self.workspace.project = live;
        std::mem::swap(&mut self.sidebar_list, &mut page.list);
        std::mem::swap(&mut self.rails, &mut page.rails);
        self.space_swipe.pages.insert(project.to_owned(), page);
        drawn
    }

    /// Draws a row of a page's list as it lays out, which can happen after
    /// the page itself has finished drawing.
    pub(super) fn sidebar_page_row(
        &mut self,
        page: Option<&str>,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            Some(project) => {
                self.in_sidebar_page(project, cx, |this, cx| this.sidebar_list_row(ix, cx))
            }
            None => self.sidebar_list_row(ix, cx),
        }
    }

    /// The pages strip: the live space at its offset, the space beside it
    /// while the strip is between pages, and the layer that takes swipes.
    pub(super) fn space_pager(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = self.sidebar_page_width();
        let scale = if self.space_swipe.scale > 0. {
            self.space_swipe.scale
        } else {
            2.
        };
        // Whole device pixels keep glyphs from shimmering while they move.
        let now = cx.background_executor().now();
        let offset = (self.space_pager_offset(now) * scale).round() / scale;
        let companion = self.space_pager_companion(offset).map(|(project, side)| {
            let page = self.in_sidebar_page(&project, cx, |this, cx| {
                this.refresh_sidebar_structure(cx);
                this.sidebar_page(Some(project.clone()), cx)
            });
            div()
                .debug_selector(move || format!("sidebar-page-{project}"))
                .absolute()
                .top_0()
                .left(px(offset + side * width))
                .w(px(width))
                .h_full()
                .flex()
                .flex_col()
                .child(page)
                // The neighbour is a picture of the space until it is live.
                .child(div().absolute().top_0().left_0().size_full().occlude())
        });
        let live = self.sidebar_page(None, cx);
        div()
            .debug_selector(|| "sidebar-pages".into())
            .flex_1()
            .min_h_0()
            .relative()
            .when(offset != 0., |d| d.overflow_hidden())
            .child(
                div()
                    .debug_selector(|| "sidebar-page-live".into())
                    .relative()
                    .left(px(offset))
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(live),
            )
            .children(companion)
            .child(self.space_swipe_layer(cx))
            .into_any()
    }

    /// Runs before the session list sees the wheel and returns whether the
    /// swipe took it. Vertical gestures and wheel mice pass straight through.
    pub(super) fn space_swipe_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ScrollDelta::Pixels(delta) = event.delta else {
            return false;
        };
        if self.workspace.classic_layout
            || self.settings.is_some()
            || self.menu.is_some()
            || self.workspace.projects.len() < 2
        {
            return false;
        }
        let now = cx.background_executor().now();
        let swipe = &mut self.space_swipe;
        let stale = swipe
            .last
            .is_none_or(|last| now.duration_since(last) > IDLE_RELEASE);
        if event.touch_phase == TouchPhase::Started || (stale && !swipe.lifted) {
            if swipe.dragging() {
                // A new touch while the old one never ended: let it go first.
                self.release_space_swipe(window, cx);
            }
            let swipe = &mut self.space_swipe;
            swipe.axis = None;
            swipe.pending = (0., 0.);
            swipe.travel = 0.;
            swipe.samples.clear();
            swipe.lifted = false;
            swipe.idle = None;
        }
        let (dx, dy) = (delta.x.as_f32(), delta.y.as_f32());
        let swipe = &mut self.space_swipe;
        swipe.last = Some(now);
        if swipe.lifted {
            return swipe.axis == Some(true);
        }
        match swipe.axis {
            None => {
                swipe.pending.0 += dx;
                swipe.pending.1 += dy;
                let (x, y) = swipe.pending;
                if x.abs().max(y.abs()) < AXIS_SLOP {
                    return false;
                }
                let sideways = x.abs() > y.abs() * 1.2;
                swipe.axis = Some(sideways);
                if !sideways {
                    return false;
                }
                self.grab_space_pager(x, now);
                self.space_swipe.invalidate_pages();
                self.close_preview(cx);
                self.hovered_space = None;
            }
            Some(false) => {
                if event.touch_phase == TouchPhase::Ended {
                    swipe.lifted = true;
                }
                return false;
            }
            Some(true) => {
                swipe.travel += dx;
                swipe.samples.push_back((now, swipe.travel));
                while swipe.samples.len() > 2
                    && swipe
                        .samples
                        .front()
                        .is_some_and(|(at, _)| now.duration_since(*at) > VELOCITY_WINDOW)
                {
                    swipe.samples.pop_front();
                }
            }
        }
        if event.touch_phase == TouchPhase::Ended {
            self.release_space_swipe(window, cx);
        } else {
            self.schedule_space_swipe_release(window, cx);
        }
        self.sidebar_frame.update(cx, |_, cx| cx.notify());
        true
    }

    /// A swipe that starts while the strip is still settling catches it
    /// where it is, rather than waiting or jumping back.
    fn grab_space_pager(&mut self, travel: f32, now: Instant) {
        let caught = self
            .space_swipe
            .settle
            .take()
            .map_or(0., |settle| settle.spring.at(now).0);
        let swipe = &mut self.space_swipe;
        swipe.travel = caught + travel;
        swipe.samples.clear();
        swipe.samples.push_back((now, swipe.travel));
    }

    fn schedule_space_swipe_release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_swipe.idle = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(IDLE_RELEASE).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.space_swipe.dragging() {
                    this.release_space_swipe(window, cx);
                }
            });
        }));
    }

    fn release_space_swipe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let swipe = &mut self.space_swipe;
        swipe.lifted = true;
        swipe.idle = None;
        let velocity = swipe.velocity();
        swipe.samples.clear();
        let travel = std::mem::take(&mut swipe.travel);
        let width = self.sidebar_page_width();
        let step = Self::swipe_step(travel);
        let neighbour = self.swipe_neighbour(step);
        let shown = pulled(travel, width, neighbour.is_some());
        let projected = travel + velocity * PROJECTION;
        let reduced = cx.reduce_motion();
        match neighbour {
            Some((_, path))
                if travel.abs() > MIN_COMMIT_TRAVEL
                    && projected.signum() == travel.signum()
                    && projected.abs() > width * COMMIT_FRACTION =>
            {
                let previous = self.workspace.project.clone();
                self.select_project(path, window, cx);
                // The new page was already beside the old one, so the strip
                // re-bases on it without moving and settles from there.
                let from = shown + step as f32 * width;
                self.space_swipe.settle = (!reduced).then(|| Settle {
                    spring: PaneSlide::thrown(from, 0., velocity, SETTLE_FREQUENCY),
                    companion: Some((previous, -(step as f32))),
                });
            }
            _ => {
                self.space_swipe.settle = (shown.abs() > 0.5 && !reduced).then(|| Settle {
                    spring: PaneSlide::thrown(shown, 0., velocity, SETTLE_FREQUENCY),
                    companion: None,
                });
            }
        }
        self.sidebar_frame.update(cx, |_, cx| cx.notify());
    }

    /// A transparent layer over the sidebar pages that sees wheel events
    /// before the session list does, and the mouse's side buttons even while
    /// a neighbour page is sliding past with its overlay on top.
    fn space_swipe_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let side_buttons = weak.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || !bounds.contains(&event.position) {
                        return;
                    }
                    let step = match event.button {
                        MouseButton::Navigate(NavigationDirection::Back) => -1,
                        MouseButton::Navigate(NavigationDirection::Forward) => 1,
                        _ => return,
                    };
                    let took = side_buttons
                        .update(cx, |this, cx| this.step_space(step, window, cx))
                        .unwrap_or(false);
                    if took {
                        cx.stop_propagation();
                    }
                });
                let weak = weak.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || !bounds.contains(&event.position) {
                        return;
                    }
                    let took = weak
                        .update(cx, |this, cx| this.space_swipe_wheel(event, window, cx))
                        .unwrap_or(false);
                    if took {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        Bounds, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
        VisualTestContext, WindowHandle, point, px, size,
    };

    const NEXT: &str = "/work/another-project";

    fn wheel(visual: &mut VisualTestContext, phase: TouchPhase, dx: f32, dy: f32) {
        let at = visual.debug_bounds("sidebar-pages").unwrap().center();
        visual.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Default::default(),
            touch_phase: phase,
        });
    }

    fn swipe(visual: &mut VisualTestContext, moves: &[(f32, f32)], end: bool) {
        wheel(visual, TouchPhase::Started, 0., 0.);
        for &(dx, dy) in moves {
            wheel(visual, TouchPhase::Moved, dx, dy);
        }
        if end {
            wheel(visual, TouchPhase::Ended, 0., 0.);
        }
        visual.run_until_parked();
    }

    /// Three spaces, starting on the middle one. Only the next one, the
    /// fixture's other project, has a session in its list.
    fn spaces(
        cx: &mut TestAppContext,
        reduce_motion: bool,
    ) -> (WindowHandle<super::Shell>, VisualTestContext, i64, f32) {
        cx.update(|cx| cx.set_reduce_motion(reduce_motion));
        let (window, next_session, _) = super::super::tests::window(cx);
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(size(px(1400.), px(900.)));
        let width = window
            .update(&mut visual, |shell, _, cx| {
                shell.workspace.classic_layout = false;
                shell.workspace.projects = ["/work/first", "/work/monocode", NEXT]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                shell.workspace.project = "/work/monocode".into();
                shell.invalidate_chrome(cx);
                cx.notify();
                shell.workspace.sidebar_width
            })
            .unwrap();
        visual.run_until_parked();
        (window, visual, next_session, width)
    }

    fn project(window: WindowHandle<super::Shell>, visual: &mut VisualTestContext) -> String {
        window
            .update(visual, |shell, _, _| shell.workspace.project.clone())
            .unwrap()
    }

    fn page(visual: &mut VisualTestContext, project: &str) -> Option<Bounds<Pixels>> {
        visual.debug_bounds(format!("sidebar-page-{project}").leak())
    }

    #[gpui::test]
    fn the_neighbouring_space_follows_the_fingers_with_its_real_sessions(cx: &mut TestAppContext) {
        let (_, mut visual, next_session, width) = spaces(cx, true);
        let visual = &mut visual;
        let rest = visual.debug_bounds("sidebar-page-live").unwrap();
        assert!(
            page(visual, NEXT).is_none(),
            "neighbours only draw mid-swipe"
        );

        swipe(visual, &[(-30., 0.), (-30., 0.)], false);
        let live = visual.debug_bounds("sidebar-page-live").unwrap();
        let next = page(visual, NEXT).expect("the next space shows while dragging");
        assert_eq!(live.left(), rest.left() - px(60.));
        assert_eq!(next.left(), live.left() + px(width));
        let row = visual
            .debug_bounds(format!("sidebar-session-{next_session}").leak())
            .expect("the neighbour lists its own sessions");
        assert!(next.contains(&row.center()));
    }

    #[gpui::test]
    fn committing_rebases_the_strip_without_a_jump_and_a_new_swipe_catches_it(
        cx: &mut TestAppContext,
    ) {
        let (window, mut visual, _, width) = spaces(cx, false);
        let visual = &mut visual;
        let travel = (width * 0.6).round();
        swipe(visual, &[(-travel / 2., 0.), (-travel / 2., 0.)], false);
        let before = page(visual, NEXT).unwrap().left();

        wheel(visual, TouchPhase::Ended, 0., 0.);
        visual.run_until_parked();
        assert_eq!(project(window, visual), NEXT);
        let live = visual.debug_bounds("sidebar-page-live").unwrap().left();
        assert!(
            (live - before).abs() < px(4.),
            "the new space carries on from where the fingers left it"
        );
        let left = page(visual, "/work/monocode").expect("the old space slides out beside it");
        assert_eq!(left.left(), live - px(width));

        // Before it lands, a new swipe takes hold of the strip where it is.
        swipe(visual, &[(10., 0.)], false);
        let caught = visual.debug_bounds("sidebar-page-live").unwrap().left();
        assert!(caught > live, "the strip neither restarts nor waits");
        assert_eq!(project(window, visual), NEXT);
    }

    #[gpui::test]
    fn vertical_trackpad_scrolls_the_session_list(cx: &mut TestAppContext) {
        let (window, mut visual, _, _) = spaces(cx, false);
        let visual = &mut visual;
        window
            .update(visual, |shell, _, cx| {
                let store = super::super::locked(&shell.store).unwrap();
                let mut made = Vec::new();
                for i in 0..40 {
                    let mut summary = store.create("codex", "/work/monocode").unwrap();
                    summary.title = format!("Session {i}");
                    summary.has_messages = true;
                    made.push(summary);
                }
                drop(store);
                for summary in made {
                    shell.order.push(summary.id);
                    shell.session_summaries.insert(summary.id, summary);
                }
                shell.notify_sidebar(cx);
            })
            .unwrap();
        visual.run_until_parked();
        let scrolled = |visual: &mut VisualTestContext| {
            window
                .update(visual, |shell, _, _| {
                    -shell.sidebar_list.state.scroll_px_offset_for_scrollbar().y
                })
                .unwrap()
        };
        let before = scrolled(visual);
        swipe(visual, &[(0., -30.), (0., -30.), (0., -30.)], true);
        let after = scrolled(visual);
        assert!(after > before, "a vertical swipe scrolls: {before:?} -> {after:?}");
        // Momentum after the fingers lift keeps scrolling.
        wheel(visual, TouchPhase::Moved, 0., -30.);
        visual.run_until_parked();
        let momentum = scrolled(visual);
        assert!(momentum > after, "momentum scrolls: {after:?} -> {momentum:?}");
        // And the next gesture scrolls again.
        swipe(visual, &[(0., -30.), (0., -30.)], true);
        assert!(scrolled(visual) > momentum, "a second swipe scrolls");
    }

    #[gpui::test]
    fn sideways_swipes_step_between_neighbouring_spaces(cx: &mut TestAppContext) {
        let (window, mut visual, _, width) = spaces(cx, true);
        let visual = &mut visual;
        let far = width * 0.3;

        // Short of halfway, with no flick, it springs back.
        swipe(visual, &[(-far / 2., 0.), (-far / 2., 0.)], true);
        assert_eq!(project(window, visual), "/work/monocode");

        // A vertical scroll that drifts sideways stays a scroll.
        swipe(visual, &[(-10., -30.), (-40., -60.)], true);
        assert_eq!(project(window, visual), "/work/monocode");

        // Pulling the strip left brings in the next space, once, even with
        // momentum still arriving after the fingers lift.
        swipe(visual, &[(-far, 0.), (-far, 0.)], true);
        assert_eq!(project(window, visual), NEXT);
        for _ in 0..4 {
            wheel(visual, TouchPhase::Moved, -60., 0.);
        }
        visual.run_until_parked();
        assert_eq!(project(window, visual), NEXT);

        // The last space doesn't wrap around to the first.
        swipe(visual, &[(-far, 0.), (-far, 0.), (-far, 0.)], true);
        assert_eq!(project(window, visual), NEXT);

        // Pulling right goes back.
        swipe(visual, &[(far, 0.), (far, 0.)], true);
        assert_eq!(project(window, visual), "/work/monocode");

        // A swipe AppKit cancels never sends an end; it releases once idle.
        swipe(visual, &[(far, 0.), (far, 0.)], false);
        visual.executor().advance_clock(super::IDLE_RELEASE * 2);
        visual.run_until_parked();
        assert_eq!(project(window, visual), "/work/first");
    }

    #[gpui::test]
    fn the_spaces_pill_trails_the_pages_then_lands_with_them(cx: &mut TestAppContext) {
        let (window, mut visual, _, width) = spaces(cx, false);
        let visual = &mut visual;
        let pill = |visual: &mut VisualTestContext| {
            visual.debug_bounds("project-space-pill").unwrap().left()
        };
        let rest = pill(visual);
        let step = visual.debug_bounds("project-space-2").unwrap().left() - rest;

        // No time has passed on the test clock, so the pill hasn't moved yet.
        swipe(visual, &[(-60., 0.), (-60., 0.)], false);
        assert_eq!(pill(visual), rest);

        // With the fingers resting on the trackpad, it catches the pages up.
        for _ in 0..3 {
            visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            wheel(visual, TouchPhase::Moved, 0., 0.);
            visual.run_until_parked();
        }
        let expected = rest + step * (120. / width);
        assert!((pill(visual) - expected).abs() < px(1.));
        assert_eq!(project(window, visual), "/work/monocode");
    }

    #[gpui::test]
    fn mouse_side_buttons_over_the_sidebar_step_between_spaces(cx: &mut TestAppContext) {
        use gpui::{MouseButton, NavigationDirection};
        let (window, mut visual, _, _) = spaces(cx, true);
        let visual = &mut visual;
        let press = |visual: &mut VisualTestContext, over: &'static str, button| {
            let at = visual.debug_bounds(over).unwrap().center();
            visual.simulate_mouse_down(at, button, Default::default());
            visual.simulate_mouse_up(at, button, Default::default());
            visual.run_until_parked();
        };
        let back = MouseButton::Navigate(NavigationDirection::Back);
        let forward = MouseButton::Navigate(NavigationDirection::Forward);

        press(visual, "sidebar-pages", forward);
        assert_eq!(project(window, visual), NEXT);
        // Unlike a swipe, the buttons loop past the last space to the first.
        press(visual, "sidebar-pages", forward);
        assert_eq!(project(window, visual), "/work/first");

        // And back from the first to the last.
        press(visual, "sidebar-pages", back);
        assert_eq!(project(window, visual), NEXT);
        press(visual, "sidebar-pages", back);
        press(visual, "sidebar-pages", back);
        assert_eq!(project(window, visual), "/work/first");

        // The spaces bar sits inside the sidebar, so it takes them too.
        press(visual, "project-space-1", forward);
        assert_eq!(project(window, visual), "/work/monocode");

        // Over the conversation they are session history, not spaces.
        press(visual, "main-pane-frame", forward);
        assert_eq!(project(window, visual), "/work/monocode");
    }

    #[gpui::test]
    fn looping_past_an_end_slides_the_pill_off_one_side_and_in_from_the_other(
        cx: &mut TestAppContext,
    ) {
        use gpui::{MouseButton, NavigationDirection};
        let (window, mut visual, _, _) = spaces(cx, false);
        let visual = &mut visual;
        let forward = MouseButton::Navigate(NavigationDirection::Forward);
        let step = |visual: &mut VisualTestContext| {
            let at = visual.debug_bounds("sidebar-pages").unwrap().center();
            visual.simulate_mouse_down(at, forward, Default::default());
            visual.simulate_mouse_up(at, forward, Default::default());
        };
        // Frames come from animation requests, so redraw as the clock moves.
        let settle = |window: WindowHandle<super::Shell>, visual: &mut VisualTestContext| {
            for _ in 0..10 {
                visual
                    .executor()
                    .advance_clock(std::time::Duration::from_millis(100));
                window
                    .update(visual, |shell, _, cx| shell.invalidate_chrome(cx))
                    .unwrap();
                visual.run_until_parked();
            }
        };
        let strip = |visual: &mut VisualTestContext| {
            let first = visual.debug_bounds("project-space-0").unwrap();
            let last = visual.debug_bounds("project-space-2").unwrap();
            (first.left(), last.right())
        };

        step(visual);
        visual.run_until_parked();
        assert_eq!(project(window, visual), NEXT);
        settle(window, visual);
        assert!(visual.debug_bounds("project-space-pill-leaving").is_none());

        // Past the last space the pill keeps heading right and out of the
        // bar, while a second one arrives from beyond the left edge.
        step(visual);
        visual.run_until_parked();
        assert_eq!(project(window, visual), "/work/first");
        let (left, right) = strip(visual);
        let leaving = visual.debug_bounds("project-space-pill-leaving");
        let incoming = visual.debug_bounds("project-space-pill").unwrap();
        assert!(
            leaving.is_some_and(|b| b.left() >= left && b.left() > incoming.left()),
            "the old pill is still on its way out to the right"
        );
        assert!(incoming.right() <= right && incoming.left() <= left);

        // Once the pages land, one pill rests on the first space.
        settle(window, visual);
        assert!(visual.debug_bounds("project-space-pill-leaving").is_none());
        assert_eq!(
            visual.debug_bounds("project-space-pill").unwrap().left(),
            visual.debug_bounds("project-space-0").unwrap().left()
        );
    }

    #[gpui::test]
    fn side_buttons_walk_session_history_everywhere_but_the_sidebar(cx: &mut TestAppContext) {
        use gpui::{MouseButton, NavigationDirection};
        let (window, mut visual, next_session, _) = spaces(cx, true);
        let visual = &mut visual;
        let back = MouseButton::Navigate(NavigationDirection::Back);
        let forward = MouseButton::Navigate(NavigationDirection::Forward);
        let press = |visual: &mut VisualTestContext, over: &'static str, button| {
            let at = visual.debug_bounds(over).unwrap().center();
            visual.simulate_mouse_down(at, button, Default::default());
            visual.simulate_mouse_up(at, button, Default::default());
            visual.run_until_parked();
        };
        let active = |window: WindowHandle<super::Shell>, visual: &mut VisualTestContext| {
            window
                .update(visual, |shell, _, _| shell.workspace.active)
                .unwrap()
        };
        let start = active(window, visual);
        window
            .update(visual, |shell, window, cx| {
                shell.open(next_session, window, cx)
            })
            .unwrap();
        visual.run_until_parked();
        let visited = active(window, visual);
        assert_ne!(visited, start);

        press(visual, "main-pane-frame", back);
        assert_eq!(active(window, visual), start);
        press(visual, "main-pane-frame", forward);
        assert_eq!(active(window, visual), visited);

        // Over the sidebar the same buttons step spaces instead.
        press(visual, "main-pane-frame", back);
        let space = project(window, visual);
        press(visual, "sidebar-pages", forward);
        assert_ne!(project(window, visual), space);
        press(visual, "sidebar-pages", back);
        assert_eq!(project(window, visual), space);
        press(visual, "main-pane-frame", forward);

        // The original layout has no spaces to step, so history gets them.
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.classic_layout = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let here = active(window, visual);
        press(visual, "main-pane-frame", back);
        let held = active(window, visual);
        assert_ne!(held, here, "back moved through history");
        press(visual, "main-pane-frame", forward);
        assert_eq!(active(window, visual), here);
        press(visual, "main-pane-frame", back);

        // An open menu keeps them to itself.
        window
            .update(visual, |shell, window, cx| {
                shell.open_menu(
                    super::super::MenuTarget::Header,
                    crate::popover::Anchor::Point(point(px(200.), px(200.))),
                    window,
                    cx,
                );
            })
            .unwrap();
        visual.run_until_parked();
        press(visual, "main-pane-frame", forward);
        assert_eq!(active(window, visual), held);
    }
}
