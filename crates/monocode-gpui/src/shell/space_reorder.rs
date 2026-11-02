//! Dragging a project space sideways reorders the projects. The other spaces
//! slide aside as the dragged one passes them, and holding the drag past
//! either end of the strip pages the five visible spaces along.
use super::*;
use std::time::{Duration, Instant};

/// How long a drag rests past the strip's edge before each page turn.
const EDGE_PAGE_DELAY: Duration = Duration::from_millis(350);
/// The strip's `gap_1` between spaces.
const SPACE_GAP: f32 = 4.;

pub(super) struct SpaceDrag {
    path: String,
    from: usize,
    to: usize,
    origin: Pixels,
    pointer: Pixels,
    /// Where the press landed inside the dragged space.
    grab: Pixels,
    strip: Bounds<Pixels>,
    /// One space's width plus the gap after it.
    step: f32,
    active: bool,
    paging: bool,
}

#[derive(Default)]
pub(super) struct SpaceReorder {
    drag: Option<SpaceDrag>,
    shifts: HashMap<String, TabShift>,
    skip_click: bool,
    /// Counts drags so a page timer stops once its own drag has ended.
    generation: usize,
}

impl SpaceReorder {
    pub(super) fn active(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.active)
    }
    pub(super) fn animating(&self) -> bool {
        let now = Instant::now();
        self.shifts.values().any(|shift| !shift.at(now).1)
    }
    /// The projects in the order the strip draws them: mid-drag, the dragged
    /// space already sits in the slot it would drop into.
    pub(super) fn order(&self, projects: &[String]) -> Vec<String> {
        let mut order = projects.to_vec();
        if let Some(drag) = self.drag.as_ref().filter(|drag| drag.active)
            && drag.from < order.len()
        {
            let moved = order.remove(drag.from);
            order.insert(drag.to.min(order.len()), moved);
        }
        order
    }
    /// The sideways offset to draw a space at, `ix` places into the order.
    pub(super) fn offset(&self, path: &str, ix: usize, first: usize) -> f32 {
        if let Some(drag) = self
            .drag
            .as_ref()
            .filter(|drag| drag.active && drag.path == path)
        {
            return Self::dragged_offset(drag, ix, first);
        }
        self.shifts
            .get(path)
            .map_or(0., |shift| shift.at(Instant::now()).0)
    }
    pub(super) fn dragging(&self, path: &str) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|drag| drag.active && drag.path == path)
    }
    /// A drag just ended on this space; its click must not also select it.
    pub(super) fn take_click(&mut self) -> bool {
        std::mem::take(&mut self.skip_click)
    }
    fn dragged_offset(drag: &SpaceDrag, ix: usize, first: usize) -> f32 {
        let slot = drag.strip.left().as_f32() + (ix as f32 - first as f32) * drag.step;
        (drag.pointer - drag.grab).as_f32() - slot
    }
    /// Starts the slide of every space whose place changed between orders.
    fn slide(&mut self, before: &[String], after: &[String], step: f32, reduce: bool) {
        let now = Instant::now();
        for (to, path) in after.iter().enumerate() {
            let Some(from) = before.iter().position(|p| p == path) else {
                continue;
            };
            if from == to || self.dragging(path) {
                continue;
            }
            let current = self.shifts.get(path).map_or(0., |shift| shift.at(now).0);
            let start = current + (from as f32 - to as f32) * step;
            self.shifts.insert(
                path.clone(),
                TabShift {
                    from: if reduce { 0. } else { start },
                    to: 0.,
                    start: now,
                },
            );
        }
    }
}

impl Shell {
    pub(super) fn start_space_drag(&mut self, path: &str, event: &MouseDownEvent) {
        self.space_reorder.skip_click = false;
        let projects = &self.workspace.projects;
        let (Some(from), Some(strip)) = (
            projects.iter().position(|p| p == path),
            self.space_swipe.spaces_strip.get(),
        ) else {
            return;
        };
        if projects.len() < 2 {
            return;
        }
        let count = projects.len().min(5) as f32;
        let width = (strip.size.width.as_f32() - SPACE_GAP * (count - 1.)) / count;
        let step = width + SPACE_GAP;
        let left = strip.left().as_f32() + (from as f32 - self.spaces_start as f32) * step;
        self.space_reorder.shifts.clear();
        self.space_reorder.generation += 1;
        self.space_reorder.drag = Some(SpaceDrag {
            path: path.into(),
            from,
            to: from,
            origin: event.position.x,
            pointer: event.position.x,
            grab: event.position.x - px(left),
            strip,
            step,
            active: false,
            paging: false,
        });
    }

    pub(super) fn move_space_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.space_reorder.drag.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.cancel_space_drag(cx);
            return;
        }
        let drag = self.space_reorder.drag.as_mut().unwrap();
        drag.pointer = event.position.x;
        if !drag.active && (drag.pointer - drag.origin).abs() < px(5.) {
            return;
        }
        if !drag.active {
            drag.active = true;
            self.hovered_space = None;
            // Escape reaches the workspace to cancel, as in the project rail.
            window.focus(&self.focus, cx);
        }
        self.space_reorder.skip_click = true;
        self.retarget_space_drag(cx);
        let drag = self.space_reorder.drag.as_mut().unwrap();
        if Self::space_drag_edge(drag) != 0 && !drag.paging {
            drag.paging = true;
            let generation = self.space_reorder.generation;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(EDGE_PAGE_DELAY).await;
                    let paging = this
                        .update(cx, |this, cx| {
                            this.space_reorder.generation == generation && this.page_space_drag(cx)
                        })
                        .unwrap_or(false);
                    if !paging {
                        break;
                    }
                }
            })
            .detach();
        }
        self.invalidate_chrome(cx);
    }

    /// -1 or 1 while the pointer is held past the strip's start or end.
    fn space_drag_edge(drag: &SpaceDrag) -> isize {
        if drag.pointer < drag.strip.left() {
            -1
        } else if drag.pointer > drag.strip.right() {
            1
        } else {
            0
        }
    }

    /// Turns one page towards the edge the drag is held past. Returns whether
    /// the drag is still waiting at an edge.
    fn page_space_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.space_reorder.drag.as_mut().filter(|drag| drag.active) else {
            return false;
        };
        let edge = Self::space_drag_edge(drag);
        if edge == 0 {
            drag.paging = false;
            return false;
        }
        let last = self.workspace.projects.len().saturating_sub(5);
        let next = self.spaces_start.saturating_add_signed(edge).min(last);
        if next != self.spaces_start {
            self.spaces_start = next;
            self.retarget_space_drag(cx);
            self.invalidate_chrome(cx);
        }
        true
    }

    /// Points the drag at the visible slot nearest to the dragged space.
    fn retarget_space_drag(&mut self, cx: &mut Context<Self>) {
        let reduce = cx.reduce_motion();
        let projects = self.workspace.projects.clone();
        let first = self.spaces_start;
        let Some(drag) = self.space_reorder.drag.as_ref() else {
            return;
        };
        let count = projects.len().min(5);
        let left = (drag.pointer - drag.grab - drag.strip.left()).as_f32();
        let slot = (left / drag.step).round().clamp(0., count as f32 - 1.) as usize;
        let to = (first + slot).min(projects.len() - 1);
        if to == drag.to {
            return;
        }
        let step = drag.step;
        let before = self.space_reorder.order(&projects);
        self.space_reorder.drag.as_mut().unwrap().to = to;
        let after = self.space_reorder.order(&projects);
        self.space_reorder.slide(&before, &after, step, reduce);
    }

    pub(super) fn finish_space_drag(&mut self, cx: &mut Context<Self>) {
        if !self.space_reorder.active() {
            self.space_reorder.drag = None;
            return;
        }
        let drag = self.space_reorder.drag.take().unwrap();
        // The dropped space glides from under the pointer into its slot.
        let landed = SpaceReorder::dragged_offset(&drag, drag.to, self.spaces_start);
        if !cx.reduce_motion() {
            self.space_reorder.shifts.insert(
                drag.path.clone(),
                TabShift {
                    from: landed,
                    to: 0.,
                    start: Instant::now(),
                },
            );
        }
        if self.workspace.move_project(&drag.path, drag.to) {
            self.persist(cx);
        }
        self.invalidate_chrome(cx);
    }

    /// Escape puts every space back where it was. Returns whether a drag ended.
    pub(super) fn cancel_space_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.space_reorder.drag.as_ref() else {
            return false;
        };
        if !drag.active {
            self.space_reorder.drag = None;
            return true;
        }
        let projects = self.workspace.projects.clone();
        let before = self.space_reorder.order(&projects);
        let landed = SpaceReorder::dragged_offset(drag, drag.to, self.spaces_start);
        let (path, from, to, step) = (drag.path.clone(), drag.from, drag.to, drag.step);
        self.space_reorder.drag = None;
        let reduce = cx.reduce_motion();
        self.space_reorder.slide(&before, &projects, step, reduce);
        if !reduce {
            let start = landed + (to as f32 - from as f32) * step;
            self.space_reorder.shifts.insert(
                path,
                TabShift {
                    from: start,
                    to: 0.,
                    start: Instant::now(),
                },
            );
        }
        self.space_reorder.skip_click = true;
        self.invalidate_chrome(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MenuTarget, locked};
    use crate::workspace::Workspace;
    use gpui::{
        Modifiers, MouseButton, TestAppContext, VisualTestContext, WindowHandle, point, px, size,
    };

    const PROJECTS: [&str; 7] = [
        "/work/a", "/work/b", "/work/c", "/work/d", "/work/e", "/work/f", "/work/g",
    ];

    fn spaces(cx: &mut TestAppContext) -> (WindowHandle<super::Shell>, VisualTestContext) {
        cx.update(super::super::bind_keys);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _) = super::super::tests::window(cx);
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(size(px(1400.), px(900.)));
        window
            .update(&mut visual, |shell, _, cx| {
                shell.workspace.classic_layout = false;
                shell.workspace.projects = PROJECTS.map(str::to_owned).to_vec();
                shell.workspace.project = "/work/a".into();
                shell.spaces_start = 0;
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        (window, visual)
    }

    fn projects(window: WindowHandle<super::Shell>, visual: &mut VisualTestContext) -> Vec<String> {
        window
            .update(visual, |shell, _, _| shell.workspace.projects.clone())
            .unwrap()
    }

    fn space(visual: &mut VisualTestContext, ix: usize) -> gpui::Bounds<gpui::Pixels> {
        visual
            .debug_bounds(format!("project-space-{ix}").leak())
            .unwrap()
    }

    #[gpui::test]
    fn dragging_a_space_reorders_and_saves_the_projects(cx: &mut TestAppContext) {
        let (window, mut visual) = spaces(cx);
        let visual = &mut visual;
        let from = space(visual, 1).center();
        let to = space(visual, 3).center();
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(from.x + px(20.), from.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let dragged = space(visual, 3);
        assert!(
            dragged.contains(&to),
            "the dragged space follows the pointer into its new slot"
        );
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            projects(window, visual),
            [
                "/work/a", "/work/c", "/work/d", "/work/b", "/work/e", "/work/f", "/work/g"
            ]
        );
        window
            .update(visual, |shell, _, _| {
                assert_eq!(
                    shell.workspace.project, "/work/a",
                    "letting go of a drag is not a click"
                );
                let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                let restored = Workspace::restore(Some(&saved), &[], "/work/a");
                assert_eq!(restored.projects, shell.workspace.projects);
            })
            .unwrap();

        // A plain click afterwards still selects.
        let c = space(visual, 1).center();
        visual.simulate_click(c, Modifiers::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.project, "/work/c")
            })
            .unwrap();
    }

    #[gpui::test]
    fn escape_puts_a_dragged_space_back(cx: &mut TestAppContext) {
        let (window, mut visual) = spaces(cx);
        let visual = &mut visual;
        let from = space(visual, 0).center();
        let to = space(visual, 2).center();
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(projects(window, visual), PROJECTS.map(str::to_owned));
        window
            .update(visual, |shell, _, _| assert!(!shell.space_reorder.active()))
            .unwrap();
    }

    #[gpui::test]
    fn holding_a_drag_past_the_end_pages_along(cx: &mut TestAppContext) {
        let (window, mut visual) = spaces(cx);
        let visual = &mut visual;
        let from = space(visual, 4).center();
        let strip = window
            .update(visual, |shell, _, _| {
                shell.space_swipe.spaces_strip.get().unwrap()
            })
            .unwrap();
        let past = point(strip.right() + px(12.), from.y);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(past, MouseButton::Left, Modifiers::default());
        for _ in 0..2 {
            visual.executor().advance_clock(super::EDGE_PAGE_DELAY);
            visual.run_until_parked();
        }
        window
            .update(visual, |shell, _, _| assert_eq!(shell.spaces_start, 2))
            .unwrap();
        visual.simulate_mouse_up(past, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            projects(window, visual),
            [
                "/work/a", "/work/b", "/work/c", "/work/d", "/work/f", "/work/g", "/work/e"
            ]
        );
    }

    #[gpui::test]
    fn the_project_menu_and_shortcut_move_a_space(cx: &mut TestAppContext) {
        let (window, mut visual) = spaces(cx);
        let visual = &mut visual;
        window
            .update(visual, |shell, window, cx| {
                let target = MenuTarget::Project {
                    path: "/work/c".into(),
                    rail: false,
                };
                shell.run_menu(target.clone(), "move:start", None, window, cx);
                assert_eq!(shell.workspace.projects[0], "/work/c");
                shell.run_menu(target, "move:end", None, window, cx);
                assert_eq!(shell.workspace.projects[6], "/work/c");
                shell.select_project("/work/c".into(), window, cx);
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| shell.move_current_project(-1, cx))
            .unwrap();
        visual.run_until_parked();
        assert_eq!(projects(window, visual)[5], "/work/c");
        window
            .update(visual, |shell, _, _| {
                let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                let restored = Workspace::restore(Some(&saved), &[], "/work/c");
                assert_eq!(restored.projects, shell.workspace.projects);
            })
            .unwrap();
    }
}
