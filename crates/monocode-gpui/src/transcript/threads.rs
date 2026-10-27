use super::*;
use monocode_core::tool::threads::ThreadRun;

impl gpui::EventEmitter<crate::conversation::OpenThread> for TranscriptView {}

/// How a child thread reads in the spawn tree.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Running,
    Attention,
    Failed,
    Done,
}
impl Tone {
    fn color(self) -> gpui::Hsla {
        gpui::rgb(match self {
            Tone::Running => 0x7dd3fc,
            Tone::Attention => 0xfbbf24,
            Tone::Failed => 0xf87171,
            Tone::Done => 0x6ee7b7,
        })
        .into()
    }
    /// Only settled children can be removed from the tree.
    fn finished(self) -> bool {
        matches!(self, Tone::Failed | Tone::Done)
    }
}
struct Child {
    id: Option<i64>,
    title: String,
    harness: String,
    model: String,
    label: &'static str,
    tone: Tone,
}

impl TranscriptView {
    fn thread_run_at(&self, ix: usize) -> Option<(&ToolCall, ThreadRun)> {
        let Body::Tool { call, .. } = &self.rows.get(ix)?.body else {
            return None;
        };
        match call.panel()? {
            Panel::Thread(run) => Some((call, run)),
            _ => None,
        }
    }
    fn remeasure_threads(&mut self, ids: &HashSet<i64>) {
        let rows: Vec<usize> = (0..self.rows.len())
            .filter(|&ix| {
                self.thread_run_at(ix)
                    .is_some_and(|(_, run)| run.id.is_some_and(|id| ids.contains(&id)))
            })
            .collect();
        for ix in rows {
            self.list.remeasure_items(ix..ix + 1);
        }
    }
    /// The visible children of the spawn tree around row `ix`: adjacent child
    /// rows form one tree under the lead, minus those the user removed.
    fn thread_tree(&self, ix: usize) -> Vec<(usize, Child)> {
        let mut from = ix;
        while from > 0 && self.thread_run_at(from - 1).is_some() {
            from -= 1;
        }
        let mut to = ix;
        while self.thread_run_at(to + 1).is_some() {
            to += 1;
        }
        (from..=to)
            .filter_map(|at| {
                let (call, run) = self.thread_run_at(at)?;
                let child = child(self, call, &run);
                (at == ix
                    || !child
                        .id
                        .is_some_and(|id| self.thread_dismissed.contains(&id)))
                .then_some((at, child))
            })
            .collect()
    }
    /// One child's details open at a time, so a long tree never expands whole.
    pub(crate) fn toggle_thread(&mut self, id: i64, start: usize, cx: &mut Context<Self>) {
        let _ = start;
        let opening = !self.thread_expanded.contains(&id);
        let mut touched: HashSet<i64> = std::mem::take(&mut self.thread_expanded);
        touched.insert(id);
        if opening {
            self.thread_expanded.insert(id);
        }
        self.list.pause_following_tail();
        self.remeasure_threads(&touched);
        cx.notify();
    }
    pub(crate) fn dismiss_threads(&mut self, ids: &[i64], cx: &mut Context<Self>) {
        let ids: HashSet<i64> = ids.iter().copied().collect();
        for id in &ids {
            self.thread_dismissed.insert(*id);
            self.thread_expanded.remove(id);
        }
        self.remeasure_threads(&ids);
        cx.notify();
    }
    pub(crate) fn set_thread_states(
        &mut self,
        states: HashMap<i64, serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        if self.thread_states != states {
            self.thread_states = states;
            for (ix, row) in self.rows.iter().enumerate() {
                if matches!(&row.body, Body::Tool{call,..} if call.thread_action().is_some()) {
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
            cx.notify();
        }
    }
}

fn child(view: &TranscriptView, call: &ToolCall, run: &ThreadRun) -> Child {
    let state = view.thread_states.get(&run.id.unwrap_or_default());
    let title = state
        .and_then(|v| v["title"].as_str())
        .unwrap_or(&run.title)
        .to_owned();
    let harness = state
        .and_then(|v| v["harness"].as_str())
        .or(run.harness.as_deref())
        .unwrap_or("codex")
        .to_owned();
    let model = state
        .and_then(|v| v["model"].as_str())
        .or(run.model.as_deref())
        .unwrap_or_default()
        .to_owned();
    let status =
        state
            .and_then(|v| v["status"].as_str())
            .unwrap_or(if call.state == ToolState::Running {
                "starting"
            } else if call.state == ToolState::Failed {
                "failed"
            } else {
                "idle"
            });
    let (label, tone) = match status {
        "needsInput" => ("Needs input", Tone::Attention),
        "working" => ("Working", Tone::Running),
        "starting" => ("Starting", Tone::Running),
        "failed" => ("Failed", Tone::Failed),
        _ if state.is_some_and(|v| v["success"] == false) => ("Failed", Tone::Failed),
        _ if state.is_some_and(|v| v["success"].is_null()) => ("Saved", Tone::Done),
        _ => ("Idle", Tone::Done),
    };
    Child {
        id: run.id,
        title,
        harness,
        model,
        label,
        tone,
    }
}

fn lead_header(
    view: &TranscriptView,
    tree: &[(usize, Child)],
    start: usize,
    cx: &mut Context<TranscriptView>,
) -> gpui::AnyElement {
    let count = |f: fn(Tone) -> bool| tree.iter().filter(|(_, c)| f(c.tone)).count();
    let working = count(|t| matches!(t, Tone::Running | Tone::Attention));
    let failed = count(|t| t == Tone::Failed);
    let mut summary = vec![format!(
        "{} agent{}",
        tree.len(),
        if tree.len() == 1 { "" } else { "s" }
    )];
    if working > 0 {
        summary.push(format!("{working} working"));
    }
    if failed > 0 {
        summary.push(format!("{failed} failed"));
    }
    let finished: Vec<i64> = tree
        .iter()
        .filter(|(_, c)| c.tone.finished())
        .filter_map(|(_, c)| c.id)
        .collect();
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_1p5()
        .py_1()
        .child(provider_icon(&view.provider, 16.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(content(0.8))
                .child("Lead"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .text_color(content(0.45))
                .child(summary.join(" · ")),
        )
        .when(!finished.is_empty(), |d| {
            d.child(
                div()
                    .id(("thread-clear", start))
                    .role(gpui::Role::Button)
                    .aria_label("Clear finished agents")
                    .px_2()
                    .py_0p5()
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .text_color(content(0.55))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.08)).text_color(content(0.85)))
                    .child("Clear finished")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dismiss_threads(&finished, cx);
                    })),
            )
        })
        .into_any_element()
}

pub(super) fn render(
    view: &mut TranscriptView,
    call: &ToolCall,
    run: &ThreadRun,
    start: usize,
    cx: &mut Context<TranscriptView>,
) -> gpui::AnyElement {
    let id = run.id.unwrap_or_default();
    if run.id.is_some_and(|id| view.thread_dismissed.contains(&id)) {
        return div()
            .id(("live-thread", start))
            .h(px(0.))
            .into_any_element();
    }
    let Child {
        title,
        harness,
        model,
        label,
        tone,
        ..
    } = child(view, call, run);
    let tree = view
        .rows
        .iter()
        .position(|row| row.key.0 == start)
        .map(|ix| view.thread_tree(ix))
        .unwrap_or_default();
    let place = tree
        .iter()
        .position(|(_, c)| c.id == run.id && run.id.is_some())
        .unwrap_or(0);
    let (first, last) = (place == 0, place + 1 >= tree.len());
    let header = first.then(|| lead_header(view, &tree, start, cx));
    let remove_title = title.clone();
    let hover_group = SharedString::from(format!("thread-row-{start}"));
    let state = view.thread_states.get(&id);
    let open = view.thread_expanded.contains(&id);
    let mut exchange = Vec::new();
    if open {
        let mut last_reply = String::new();
        for row in &view.source {
            if row.role != "tool" {
                continue;
            }
            let Ok(tool) = serde_json::from_str::<ToolCall>(&row.text) else {
                continue;
            };
            match tool.thread_action() {
                Some("start" | "send") => {
                    if let Some(Panel::Thread(thread)) = tool.panel()
                        && thread.id == run.id
                        && !thread.prompt.is_empty()
                    {
                        exchange.push(("Lead", thread.prompt));
                        last_reply.clear();
                    }
                }
                Some("wait") => {
                    let result = tool.thread_result();
                    if result["threadId"].as_i64() == run.id
                        && result["status"] == "idle"
                        && let Some(reply) = result["reply"]
                            .as_str()
                            .filter(|s| !s.is_empty() && *s != last_reply)
                    {
                        exchange.push(("Child", reply.to_owned()));
                        last_reply = reply.to_owned();
                    }
                }
                _ => (),
            }
        }
        if let Some(reply) = state
            .and_then(|v| v["reply"].as_str())
            .filter(|s| !s.is_empty() && *s != last_reply)
        {
            exchange.push(("Child", reply.to_owned()));
        } else if call.state == ToolState::Failed {
            exchange.push(("Error", call.output.clone()));
        }
    }
    let line = content(0.1);
    div()
        .id(("live-thread", start))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .children(header)
        .child(
            div()
                .relative()
                .pl(px(22.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .absolute()
                        .left(px(8.))
                        .top(px(-4.))
                        .w(px(12.))
                        .h(px(20.))
                        .border_l_1()
                        .border_b_1()
                        .border_color(line)
                        .rounded_bl(px(7.)),
                )
                .when(!last, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(8.))
                            .top(px(16.))
                            .bottom_0()
                            .border_l_1()
                            .border_color(line),
                    )
                })
                .child(
                    div()
                        .group(hover_group.clone())
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_1()
                        .child(
                            div()
                                .id(("thread-toggle", start))
                                .role(gpui::Role::Button)
                                .aria_label(format!(
                                    "{} {title}'s exchange",
                                    if open { "Hide" } else { "Show" }
                                ))
                                .aria_expanded(open)
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_1p5()
                                .py_1()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.06)))
                                .child(provider_icon(&harness, 16.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(14.))
                                        .text_color(content(0.8))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(content(0.45))
                                        .child(model),
                                )
                                .child(
                                    div()
                                        .px_1p5()
                                        .rounded_full()
                                        .border_1()
                                        .border_color(tone.color().opacity(0.25))
                                        .bg(tone.color().opacity(0.1))
                                        .text_size(px(11.))
                                        .text_color(tone.color())
                                        .child(label),
                                )
                                .child(
                                    icon(
                                        if open {
                                            "chevron-down"
                                        } else {
                                            "chevron-right"
                                        },
                                        12.,
                                    )
                                    .text_color(content(0.4)),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_thread(id, start, cx);
                                })),
                        )
                        .when(tone.finished() && run.id.is_some(), |d| {
                            d.child(
                                div()
                                    .id(("thread-remove", start))
                                    .role(gpui::Role::Button)
                                    .aria_label(format!("Remove {remove_title}"))
                                    .p_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .opacity(0.)
                                    .group_hover(hover_group.clone(), |s| s.opacity(1.))
                                    .hover(|s| s.bg(content(0.08)))
                                    .child(icon("close", 12.).text_color(content(0.55)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dismiss_threads(&[id], cx);
                                    })),
                            )
                        })
                        .when(run.id.is_some(), |d| {
                            d.child(
                                div()
                                    .id(("thread-open", start))
                                    .role(gpui::Role::Button)
                                    .aria_label("Open child thread")
                                    .p_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.08)))
                                    .child(icon("arrow-up-right", 14.).text_color(content(0.5)))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(crate::conversation::OpenThread(id))
                                    })),
                            )
                        }),
                )
                .when(open, |d| {
                    d.child(
                        div()
                            .pl(px(20.))
                            .border_l_1()
                            .border_color(content(0.1))
                            .children(exchange.into_iter().map(|(speaker, text)| {
                                div()
                                    .py_1()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .child(div().text_color(content(0.45)).child(speaker))
                                    .child(div().text_color(content(0.8)).child(text))
                            })),
                    )
                }),
        )
        .into_any_element()
}
