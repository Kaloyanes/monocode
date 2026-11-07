//! LiveAgentsPreview.tsx: the sidebar's card of this project's working
//! sessions. A finished one goes back to its dated row, which shows Done.
use super::*;

// Tailwind amber-400.
const AMBER: u32 = 0xfbbf24;

/// The card's list grows and folds with the header's chevron.
#[derive(Default)]
pub(super) struct Fold {
    motion: Option<rails::Motion>,
    frames: Option<gpui::Task<()>>,
}

struct LiveAgent {
    id: i64,
    cwd: String,
    provider: String,
    title: String,
    activity: String,
    started_at: Option<u64>,
    needs_approval: bool,
    /// The lead it nests under, when that lead is in Working too.
    nested_in: Option<i64>,
    /// Subagents of this lead that are in Working.
    subagents: usize,
}

/// liveAgents.ts's formatLiveElapsed.
fn live_elapsed(ms: u64) -> String {
    let seconds = ((ms + 500) / 1000).max(1);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let (minutes, rest) = (seconds / 60, seconds % 60);
    if minutes < 60 {
        return if rest > 0 {
            format!("{minutes}m {rest}s")
        } else {
            format!("{minutes}m")
        };
    }
    let (hours, rest) = (minutes / 60, minutes % 60);
    if rest > 0 {
        format!("{hours}h {rest}m")
    } else {
        format!("{hours}h")
    }
}

/// Milliseconds until `live_elapsed` shows something else for a run this old:
/// labels count seconds for the first hour and minutes after that.
fn next_label_change(ms: u64) -> u64 {
    // live_elapsed rounds to the nearest second.
    let shown = ms + 500;
    let step = if shown >= 3_600_000 { 60_000 } else { 1_000 };
    step - shown % step
}

impl Shell {
    fn in_working(&self, id: i64, cx: &App) -> bool {
        self.sessions.get(&id).is_some_and(|view| {
            let view = view.read(cx);
            view.working()
                && !view.summary.archived
                && view.summary.cwd == self.workspace.project
                && self.in_sidebar_worktree_scope(id, cx)
        })
    }

    /// The sessions Working holds.
    pub(super) fn sync_live_sessions(&mut self, cx: &App) -> Vec<i64> {
        let preferences = &self.workspace.preferences;
        if !preferences.working_agents || !self.workspace.sidebar_activity_visible {
            self.live_tick = None;
            return Vec::new();
        }
        let mut ids: Vec<_> = self
            .order
            .iter()
            .copied()
            .filter(|id| self.in_working(*id, cx))
            .collect();
        ids.sort_unstable();
        if ids.is_empty() && !self.space_swipe.drawing_neighbour() {
            self.live_tick = None;
        }
        ids
    }

    fn live_agents(&self, cx: &App) -> Vec<LiveAgent> {
        let mut agents: Vec<_> = self
            .order
            .iter()
            .filter(|id| self.in_working(**id, cx))
            .filter_map(|id| {
                let view = self.sessions.get(id)?.read(cx);
                let (tool, started_at) = view.transcript.read(cx).live_turn(&view.summary.cwd);
                let needs_approval = view.needs_approval();
                Some(LiveAgent {
                    id: *id,
                    cwd: view.summary.cwd.clone(),
                    provider: view.summary.provider.clone(),
                    title: view.display_title(),
                    activity: if needs_approval {
                        "Need approval".into()
                    } else {
                        tool.unwrap_or_else(|| "Working".into())
                    },
                    started_at,
                    needs_approval,
                    nested_in: None,
                    subagents: 0,
                })
            })
            .collect();
        // compareLiveAgents: approvals first, then oldest first.
        agents.sort_by_key(|a| (!a.needs_approval, a.started_at.unwrap_or(u64::MAX)));
        // A subagent reads as part of its lead: it follows the lead's row,
        // nested. One whose lead isn't in Working stays a row of its own.
        let present: HashSet<i64> = agents.iter().map(|a| a.id).collect();
        for agent in &mut agents {
            agent.nested_in = self
                .subagent_lead(agent.id)
                .filter(|lead| present.contains(lead));
        }
        let mut ordered = Vec::with_capacity(agents.len());
        let (nested, top): (Vec<_>, Vec<_>) =
            agents.into_iter().partition(|a| a.nested_in.is_some());
        let mut nested: Vec<_> = nested.into_iter().map(Some).collect();
        for mut lead in top {
            let mine: Vec<_> = nested
                .iter_mut()
                .filter(|slot| slot.as_ref().is_some_and(|a| a.nested_in == Some(lead.id)))
                .filter_map(Option::take)
                .collect();
            lead.subagents = mine.len();
            ordered.push(lead);
            ordered.extend(mine);
        }
        // A lead that is itself nested can't happen, but never drop a row.
        ordered.extend(nested.into_iter().flatten());
        ordered
    }

    pub(super) fn live_agents_preview(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let agents = if self.workspace.preferences.working_agents
            && self.workspace.sidebar_activity_visible
        {
            self.live_agents(cx)
        } else {
            Vec::new()
        };
        let shown = !agents.is_empty();
        let collapsed = self.workspace.sidebar_activity_collapsed;
        let now_at = std::time::Instant::now();
        let reduced = cx.reduce_motion();
        let motion = match self.live_fold.motion {
            None => rails::Motion {
                open: !collapsed,
                from: if collapsed { 0. } else { 1. },
                start: now_at - rails::OPEN,
            },
            Some(m) if m.open == collapsed => rails::Motion {
                open: !collapsed,
                from: if reduced {
                    f32::from(!collapsed)
                } else {
                    m.value(now_at)
                },
                start: if reduced {
                    now_at - rails::OPEN
                } else {
                    now_at
                },
            },
            Some(m) => m,
        };
        self.live_fold.motion = Some(motion);
        let progress = motion.value(now_at);
        if !motion.settled(now_at) && self.live_fold.frames.is_none() {
            self.live_fold.frames = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.live_fold.frames = None;
                    this.sidebar_list.remeasure_working();
                    this.notify_sidebar(cx);
                });
            }));
        }
        let folding = progress > 0.;
        // The card a swipe draws beside this one lists another project, so it
        // leaves the marks and the clock to the live page.
        let neighbour = self.space_swipe.drawing_neighbour();
        let sessions = &self.sessions;
        self.live_marks
            .retain(|id, _| !collapsed && sessions.get(id).is_some_and(|s| s.read(cx).working()));
        let ticking = shown && !collapsed && agents.iter().any(|a| a.started_at.is_some());
        let now = monocode_core::time::now_ms();
        if !neighbour {
            self.live_next_change = agents
                .iter()
                .filter_map(|a| a.started_at)
                .map(|at| next_label_change(now.saturating_sub(at)))
                .min()
                .unwrap_or(1000);
            if !ticking {
                self.live_tick = None;
            } else if self.live_tick.is_none() {
                self.live_tick = Some(cx.spawn(async move |this, cx| {
                    loop {
                        let Ok(wait) = this.update(cx, |this, _| this.live_next_change) else {
                            break;
                        };
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(wait.max(50)))
                            .await;
                        // The card only matters while someone is looking at the app.
                        let result = this.update(cx, |this, cx| {
                            if crate::pulse::app_active(cx) {
                                this.notify_sidebar(cx);
                            }
                        });
                        if result.is_err() {
                            break;
                        }
                    }
                }));
            }
        }
        if !shown {
            return None;
        }
        let count = agents.len();
        let now = monocode_core::time::now_ms();
        let cards: Vec<_> = agents
            .into_iter()
            .filter(|_| folding)
            .map(|agent| self.live_agent_card(agent, now, cx))
            .collect();
        let width = px((self.workspace.sidebar_width - 16.).max(0.));
        // In the list it scrolls with the rows around it. The dock has no
        // room to give, so a long run scrolls inside its own space.
        let docked = self.workspace.preferences.docked_sections;
        let body = div()
            .id("live-agents-list")
            .flex()
            .flex_col()
            .gap_px()
            .px_1()
            .pb_1()
            .when(docked, |d| d.max_h(px(360.)).overflow_y_scroll())
            .children(cards);
        let body = if progress >= 1. {
            body.into_any_element()
        } else {
            div()
                .overflow_hidden()
                .opacity(progress)
                .child(crate::reveal::Disclosure::new(body, width, progress))
                .into_any_element()
        };
        let dot = div()
            .size(px(6.))
            .flex_shrink_0()
            .rounded_full()
            .bg(accent_color())
            .shadow(vec![
                BoxShadow::new(px(0.), px(0.), accent_color()).blur_radius(px(8.)),
            ]);
        let classic_row =
            self.workspace.classic_layout && !self.workspace.preferences.docked_sections;
        Some(
            div()
                .id("live-agents")
                .debug_selector(|| "sidebar-activity-card".into())
                .role(Role::Group)
                .aria_label(format!("{count} working agents"))
                .flex_shrink_0()
                .px(px(if classic_row { 6. } else { 8. }))
                .child(
                    sidebar_sections::section_header(
                        "live-agents-header",
                        "Working",
                        count,
                        Some(if collapsed {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        }),
                    )
                    .aria_expanded(!collapsed)
                    .child(dot)
                    .child(div().flex_1().min_w_0().truncate().child("Working"))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.sidebar_activity_collapsed =
                            !this.workspace.sidebar_activity_collapsed;
                        this.sidebar_list.remeasure();
                        this.persist(cx);
                        this.notify_sidebar(cx);
                    })),
                )
                .when(folding, |d| d.child(body))
                .into_any(),
        )
    }

    fn live_agent_card(
        &mut self,
        agent: LiveAgent,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = agent.id;
        let project = self.project_label(&agent.cwd);
        let elapsed = agent
            .started_at
            .map(|at| live_elapsed(now.saturating_sub(at)));
        let selected = self.selected_sessions.contains(&id)
            || (self.workspace.active == Some(id) && !self.mono_active(cx));
        let status: AnyElement = if agent.needs_approval {
            icon("alert-circle", 12.)
                .text_color(rgb(AMBER))
                .into_any_element()
        } else {
            let provider = agent.provider.clone();
            let mark = self
                .live_marks
                .entry(id)
                .or_insert_with(|| {
                    cx.new(|cx| {
                        let mut mark = SessionMark::tab(provider.clone());
                        mark.set(&provider, MarkState::Working, cx);
                        mark
                    })
                })
                .clone();
            div()
                .size(px(12.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(mark)
                .into_any_element()
        };
        let label = [
            Some(agent.title.clone()),
            Some(project.clone()),
            Some(agent.activity.clone()),
            elapsed.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
        let nested = agent.nested_in.is_some();
        let activity_color = if agent.needs_approval {
            rgb(AMBER).into()
        } else {
            content(0.5)
        };
        // Under its lead's row: no project (it is the lead's), and the time
        // sits beside what it is doing.
        let body: AnyElement = if nested {
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.9))
                        .child(agent.title.clone()),
                )
                .child(
                    div()
                        .mt_0p5()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(activity_color)
                        .child(status)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(agent.activity.clone()),
                        )
                        .children(
                            elapsed
                                .clone()
                                .map(|e| div().flex_shrink_0().text_color(content(0.45)).child(e)),
                        ),
                )
                .into_any_element()
        } else {
            // A thread the user owns names where it came from; a subagent whose
            // lead isn't in Working names its lead.
            let lead_line = self.subagent_lead(id).map(|lead| {
                let title = self.session_metadata(lead, cx).display_title();
                div()
                    .mt_1()
                    .h(px(16.))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.45))
                    .child(icon("arrow-turn-down-right", 12.).text_color(content(0.4)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(format!("Subagent of {title}")),
                    )
            });
            let origin = if lead_line.is_none() {
                self.started_from_line(id, cx).map(|line| line.mt_1())
            } else {
                None
            };
            let pill = (agent.subagents > 0).then(|| {
                let n = agent.subagents;
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded_full()
                    .bg(content(0.08))
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(content(0.6))
                    .child(format!("{n} subagent{}", if n == 1 { "" } else { "s" }))
            });
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.project_mark(&agent.cwd, 8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(1.))
                                .child(agent.title.clone()),
                        )
                        .children(pill),
                )
                .children(lead_line.map(|line| line.pl_4()))
                .children(origin.map(|line| line.pl_4()))
                .child(
                    div()
                        .mt_1()
                        .pl_4()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(activity_color)
                        .child(status)
                        .child(div().min_w_0().truncate().child(agent.activity.clone())),
                )
                .child(
                    div()
                        .mt_1()
                        .pl_4()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(content(0.45))
                        .child(provider_icon(&agent.provider, 12.))
                        .child(div().flex_1().min_w_0().truncate().child(project))
                        .children(elapsed.map(|e| div().flex_shrink_0().child(e))),
                )
                .into_any_element()
        };
        div()
            .id(("live-agent", id as usize))
            .debug_selector(move || format!("live-agent-{id}"))
            .role(Role::Button)
            .aria_label(label)
            .cursor_pointer()
            .flex()
            .flex_col()
            .rounded_md()
            .py_1p5()
            // A subagent sits inside a rail under its lead.
            .when(nested, |d| {
                d.ml_4()
                    .pl_2()
                    .pr_2()
                    .py_1()
                    .border_l_2()
                    .border_color(content(0.12))
            })
            .when(!nested, |d| d.w_full().px_2())
            .when(selected, |d| d.bg(content(0.1)))
            .when(!selected, |d| d.hover(|s| s.bg(content(0.08))))
            .child(body)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected_sessions.clear();
                this.selection_anchor = Some(id);
                this.open(id, window, cx);
                this.invalidate_chrome(cx);
                cx.notify();
            }))
            .into_any()
    }
}
