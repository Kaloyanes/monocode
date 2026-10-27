use super::*;
use monocode_core::tool::threads::ThreadRun;

impl gpui::EventEmitter<crate::conversation::OpenThread> for TranscriptView {}

impl TranscriptView {
    pub(crate) fn toggle_thread(&mut self, id: i64, start: usize, cx: &mut Context<Self>) {
        if !self.thread_expanded.remove(&id) {
            self.thread_expanded.insert(id);
        }
        self.list.pause_following_tail();
        for (ix, row) in self.rows.iter().enumerate() {
            if row.key.0 == start {
                self.list.remeasure_items(ix..ix + 1);
            }
        }
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

pub(super) fn render(
    view: &mut TranscriptView,
    call: &ToolCall,
    run: &ThreadRun,
    start: usize,
    cx: &mut Context<TranscriptView>,
) -> gpui::AnyElement {
    let id = run.id.unwrap_or_default();
    let state = view.thread_states.get(&id);
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
    let label = match status {
        "needsInput" => "Needs input",
        "working" => "Working",
        "starting" => "Starting",
        "failed" => "Failed",
        _ if state.is_some_and(|v| v["success"] == false) => "Failed",
        _ if state.is_some_and(|v| v["success"].is_null()) => "Saved",
        _ => "Idle",
    };
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
    div()
        .id(("live-thread", start))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
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
                                .text_size(px(11.))
                                .text_color(content(0.55))
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
        })
        .into_any_element()
}
