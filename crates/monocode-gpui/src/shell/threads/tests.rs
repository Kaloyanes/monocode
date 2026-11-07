use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::{agent::Settings, transcript::Row};
use std::sync::mpsc;

fn setup(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    let (window, child, lead) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.thread_parents.insert(child, lead);
            shell.started_by.insert(child, lead);
            locked(&shell.store)
                .unwrap()
                .save_thread_parent(child, lead)
                .unwrap();
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx)
            });
            shell.sessions[&child].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.driven_by = Some(lead);
                v.managed = true;
                v.busy = true;
            });
            shell.thread_session_changed(child, cx);
        })
        .unwrap();
    cx.run_until_parked();
    // A finish message would start a real lead turn, so only tests about it
    // follow the child's turn: see `watched`.
    window
        .update(cx, |shell, _, _| shell.thread_turns.clear())
        .unwrap();
    (window, child, lead)
}

/// `setup`, with the child's turn followed so finishing it reaches the lead.
fn watched(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    let (window, child, lead) = setup(cx);
    window
        .update(cx, |shell, _, cx| shell.thread_session_changed(child, cx))
        .unwrap();
    (window, child, lead)
}

fn call(
    window: WindowHandle<Shell>,
    caller: i64,
    request: &str,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> mpsc::Receiver<Result<Value, String>> {
    let (reply, result) = mpsc::channel();
    window
        .update(cx, |shell, window, cx| {
            shell.handle_app_call(
                Call {
                    session: caller,
                    request_id: request.into(),
                    action: action.into(),
                    input,
                    reply,
                },
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    result
}

#[gpui::test]
fn threads_are_top_level_owned_and_available_to_every_chat(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let denied = call(window, child, "child-list", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap_err();
    assert!(denied.contains("top-level"));
    let denied = call(
        window,
        lead,
        "not-child",
        "threads.read",
        json!({"threadId":lead}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(denied.contains("own children"));
    let value = call(window, lead, "list", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(value["threads"][0]["threadId"], child);
    assert_eq!(value["threads"][0]["status"], "working");
    let denied = call(
        window,
        lead,
        "busy",
        "threads.send",
        json!({"threadId":child,"prompt":"Explain"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(denied.contains("busy"));
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, _| {
                v.settings.access = monocode_core::agent::AccessMode::ReadOnly
            })
        })
        .unwrap();
    assert!(
        call(
            window,
            lead,
            "read-only-list",
            "threads.list",
            json!({}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(
        call(
            window,
            lead,
            "read-only-start",
            "threads.start",
            json!({"prompt":"Review"}),
            cx
        )
        .try_recv()
        .unwrap()
        .unwrap_err()
        .contains("read-only")
    );
}

#[gpui::test]
fn threads_wait_times_out_then_resolves_with_the_child_reply(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let result = call(
        window,
        lead,
        "timeout",
        "threads.wait",
        json!({"threadId":child,"timeoutSeconds":2}),
        cx,
    );
    assert!(result.try_recv().is_err());
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(result.try_recv().unwrap().unwrap()["status"], "working");
    window
        .update(cx, |shell, _, _| {
            shell
                .agent_summaries
                .get_mut(&lead)
                .unwrap()
                .tasks
                .push(SummaryTask {
                    session: Some(-1),
                    title: "Earlier worker".into(),
                    status: "running".into(),
                    ..Default::default()
                });
        })
        .unwrap();
    let result = call(
        window,
        lead,
        "reply",
        "threads.wait",
        json!({"threadId":child}),
        cx,
    );
    assert!(result.try_recv().is_err());
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, cx| {
                v.busy = false;
                v.transcript.update(cx, |t, cx| {
                    t.replace_history(
                        vec![
                            Row {
                                role: "user".into(),
                                text: "Review".into(),
                                first: true,
                                sent_at: None,
                            },
                            Row {
                                role: "assistant".into(),
                                text: "Ownership is enforced".into(),
                                first: true,
                                sent_at: None,
                            },
                        ],
                        cx,
                    )
                });
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let reply = result.try_recv().unwrap().unwrap();
    assert_eq!(reply["status"], "idle");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["reply"], "Ownership is enforced");
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.sessions[&child].read(cx).driven_by, Some(lead));
            assert!(!shell.agent_summaries[&lead].live);
        })
        .unwrap();
}

#[gpui::test]
fn threads_stopping_lead_stops_child_and_releases_waiters(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let result = call(
        window,
        lead,
        "wait-stop",
        "threads.wait",
        json!({"threadId":child}),
        cx,
    );
    assert!(result.try_recv().is_err());
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.busy = true;
                v.stop_if_busy(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        result
            .try_recv()
            .unwrap()
            .unwrap_err()
            .contains("Lead thread was stopped")
    );
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.sessions[&child].read(cx).busy);
            assert_eq!(shell.sessions[&child].read(cx).driven_by, Some(lead));
            assert!(shell.thread_waiters.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn restored_children_name_their_lead_as_the_sender(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions.remove(&child);
            shell.thread_parents = locked(&shell.store).unwrap().thread_parents().unwrap();
            let child_view = shell.ensure_session(child, cx).unwrap();
            assert_eq!(child_view.read(cx).driven_by, Some(lead));
            assert!(child_view.read(cx).managed);
            assert_eq!(shell.sessions[&lead].read(cx).driven_by, None);
            assert!(
                child_view
                    .read(cx)
                    .transcript
                    .read(cx)
                    .senders
                    .contains_key(&lead)
            );
            assert!(
                shell.sessions[&lead]
                    .read(cx)
                    .transcript
                    .read(cx)
                    .senders
                    .is_empty()
            );
        })
        .unwrap();
}

#[gpui::test]
fn threads_start_is_idempotent_and_marks_the_child_before_dispatch(cx: &mut TestAppContext) {
    let (window, _, lead) = crate::shell::tests::window(cx);
    let missing = std::env::temp_dir()
        .join(monocode_core::mono::new_id())
        .join("missing-project");
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.summary.cwd = missing.to_string_lossy().into_owned();
            });
        })
        .unwrap();
    let input = json!({"prompt":"Review ownership","harness":"codex","title":"Ownership review"});
    let first = call(
        window,
        lead,
        "start-once",
        "threads.start",
        input.clone(),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let second = call(window, lead, "start-once", "threads.start", input, cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(first, second);
    let child = first["threadId"].as_i64().unwrap();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.thread_parents.get(&child), Some(&lead));
            assert_eq!(shell.started_by.get(&child), Some(&lead));
            assert_eq!(shell.sessions[&child].read(cx).driven_by, Some(lead));
            assert!(shell.sessions[&child].read(cx).managed);
            assert_eq!(shell.subagent_lead(child), Some(lead));
            assert_eq!(
                locked(&shell.store)
                    .unwrap()
                    .thread_parents()
                    .unwrap()
                    .get(&child),
                Some(&lead)
            );
            assert!(
                shell.agent_summaries[&lead]
                    .tasks
                    .iter()
                    .any(|t| t.session == Some(child))
            );
        })
        .unwrap();
}

#[gpui::test]
fn threads_new_opens_a_top_level_thread_the_lead_does_not_own(cx: &mut TestAppContext) {
    let (window, _, lead) = crate::shell::tests::window(cx);
    let missing = std::env::temp_dir()
        .join(monocode_core::mono::new_id())
        .join("missing-project");
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.summary.cwd = missing.to_string_lossy().into_owned();
            });
        })
        .unwrap();
    // A lead opens a new thread through its thread tools.
    let value = call(
        window,
        lead,
        "new-once",
        "threads.new",
        json!({"prompt":"Plan the drafts","title":"Drafts","draft":true}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert!(value["threadId"].is_null());
    let id = value["sessionId"].as_i64().unwrap();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.thread_parents.contains_key(&id));
            assert!(!shell.sessions[&id].read(cx).managed);
            assert!(
                !locked(&shell.store)
                    .unwrap()
                    .thread_parents()
                    .unwrap()
                    .contains_key(&id)
            );
            assert!(
                shell
                    .agent_summaries
                    .get(&lead)
                    .is_none_or(|s| s.tasks.iter().all(|t| t.session != Some(id)))
            );
        })
        .unwrap();
    let denied = call(
        window,
        lead,
        "not-child",
        "threads.send",
        json!({"threadId":id,"prompt":"Go"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(denied.contains("own children"));
}

/// A lead whose project is missing, so a launched turn starts and then fails
/// without reaching a real provider.
fn lead_without_a_project(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64) {
    let (window, _, lead) = crate::shell::tests::window(cx);
    let missing = std::env::temp_dir()
        .join(monocode_core::mono::new_id())
        .join("missing-project");
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.summary.cwd = missing.to_string_lossy().into_owned();
            });
        })
        .unwrap();
    (window, lead)
}

#[gpui::test]
fn a_lead_opens_an_unlinked_thread(cx: &mut TestAppContext) {
    let (window, lead) = lead_without_a_project(cx);
    let id = call(
        window,
        lead,
        "open",
        "threads.new",
        json!({"prompt":"Split the audit"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.thread_parents.contains_key(&id), "not a subagent");
            let view = shell.sessions[&id].read(cx);
            assert!(!view.managed && view.driven_by.is_none());
            assert!(!view.app_read_only);
        })
        .unwrap();
    // Top level, so it may run subagents of its own.
    let own = call(window, id, "own-list", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap();
    assert!(own.is_ok(), "{own:?}");
}

#[gpui::test]
fn a_draft_thread_leaves_its_prompt_in_the_composer(cx: &mut TestAppContext) {
    let (window, lead) = lead_without_a_project(cx);
    let id = call(
        window,
        lead,
        "saved",
        "threads.new",
        json!({"prompt":"Split it","draft":true}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(
                shell.sessions[&id].read(cx).input.read(cx).content,
                "Split it"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_subagent_opens_read_only_beside_its_lead(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let other = window
        .update(cx, |shell, window, cx| {
            shell.sessions[&child].update(cx, |v, _| v.busy = false);
            shell.thread_session_changed(child, cx);
            let other = locked(&shell.store)
                .unwrap()
                .create("codex", &shell.workspace.project)
                .unwrap();
            let other_id = other.id;
            shell.thread_parents.insert(other_id, lead);
            shell.started_by.insert(other_id, lead);
            shell.add(other, cx);
            shell.thread_session_changed(other_id, cx);
            shell.open(lead, window, cx);
            shell.open(child, window, cx);
            other_id
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let tab = shell.workspace.tab_of(lead).unwrap();
            assert_eq!(
                shell.workspace.layout(tab).leaf_ids(),
                [lead, child],
                "beside its lead, in the lead's tab"
            );
            assert_eq!(shell.workspace.active, Some(child));
        })
        .unwrap();
    // The lead keeps its composer; the subagent only has its lock and Stop.
    assert!(visual.debug_bounds("thread-locked-composer").is_some());
    assert!(visual.debug_bounds("composer-box").is_some());
    assert!(visual.debug_bounds("subagent-strip").is_some());
    let chip: &'static str = Box::leak(format!("subagent-chip-{other}").into_boxed_str());
    let chip = visual
        .debug_bounds(chip)
        .expect("the lead's strip lists it");
    visual.simulate_click(chip.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            let tab = shell.workspace.tab_of(lead).unwrap();
            assert_eq!(
                shell.workspace.layout(tab).leaf_ids(),
                [lead, other],
                "one subagent at a time takes the pane"
            );
            assert!(shell.workspace.tab_of(child).is_none());
            let strip = shell.subagent_strip(lead, cx);
            assert_eq!(
                strip.iter().map(|s| s.id).collect::<Vec<_>>(),
                [child, other]
            );
        })
        .unwrap();
}

#[gpui::test]
fn an_agent_row_opens_its_conversation_beside_the_lead(cx: &mut TestAppContext) {
    use monocode_core::tool::{ToolCall, ToolState};
    let tool = |id: &str, state, output: Value| {
        let mut call = ToolCall::new(id, "mcp__monocode__thread_start");
        call.input = json!({"title":"Fix the export","prompt":"Go"}).to_string();
        call.output = output.to_string();
        call.state = state;
        Row {
            role: "tool".into(),
            text: serde_json::to_string(&call).unwrap(),
            first: true,
            sent_at: None,
        }
    };
    let (window, child, lead) = setup(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.open(lead, window, cx);
            shell.sessions[&lead].update(cx, |v, _| v.summary.has_messages = true);
            let transcript = shell.sessions[&lead].read(cx).transcript.clone();
            transcript.update(cx, |view, cx| {
                view.restore_history(
                    vec![
                        tool("failed", ToolState::Failed, json!("Unknown model")),
                        tool("started", ToolState::Completed, json!({"threadId":child})),
                    ],
                    cx,
                )
            });
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let click = |visual: &mut gpui::VisualTestContext, selector: &'static str| {
        let bounds = visual.debug_bounds(selector).expect(selector);
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.run_until_parked();
    };
    let state = |visual: &mut gpui::VisualTestContext| {
        window
            .update(visual, |shell, _, cx| {
                let tab = shell.workspace.tab_of(lead).unwrap();
                let transcript = shell.sessions[&lead].read(cx).transcript.read(cx);
                (
                    shell.workspace.layout(tab).leaf_ids(),
                    transcript.thread_expanded.clone(),
                )
            })
            .unwrap()
    };

    // The chevron shows the exchange here and leaves the conversation closed.
    click(visual, "thread-toggle-1");
    let (panes, expanded) = state(visual);
    assert_eq!(panes, [lead]);
    assert!(expanded.contains(&child));

    // A failed start has no conversation, so its row shows the error instead.
    click(visual, "thread-row-0");
    let (panes, expanded) = state(visual);
    assert_eq!(panes, [lead]);
    assert!(expanded.contains(&0));

    click(visual, "thread-row-1");
    let (panes, _) = state(visual);
    assert_eq!(panes, [lead, child]);
}

#[gpui::test]
fn archiving_a_lead_archives_its_subagents_and_restoring_brings_them_back(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, _| v.busy = false);
            shell.thread_session_changed(child, cx);
            shell.toggle_archive(lead, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.session_summaries[&lead].archived);
            assert!(
                shell.session_summaries[&child].archived,
                "it goes with its lead"
            );
            shell.toggle_archive(lead, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .read_with(cx, |shell, _| {
            assert!(!shell.session_summaries[&lead].archived);
            assert!(
                !shell.session_summaries[&child].archived,
                "and comes back with it"
            );
        })
        .unwrap();
}

#[gpui::test]
fn threads_start_does_not_launch_after_the_lead_is_stopped(cx: &mut TestAppContext) {
    let (window, _, lead) = crate::shell::tests::window(cx);
    let (reply, result) = mpsc::channel();
    window
        .update(cx, |shell, window, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx)
            });
            shell.handle_app_call(
                Call {
                    session: lead,
                    request_id: "cancel-launch".into(),
                    action: "threads.start".into(),
                    input: json!({"prompt":"Review","harness":"codex"}),
                    reply,
                },
                window,
                cx,
            );
            shell.sessions[&lead].update(cx, |v, cx| {
                v.busy = true;
                v.stop_if_busy(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(result.try_recv().unwrap().unwrap_err().contains("stopped"));
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.thread_parents.values().any(|parent| *parent == lead))
        })
        .unwrap();
}

#[gpui::test]
fn threads_stopping_an_idle_lead_stops_its_busy_children(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let result = call(
        window,
        lead,
        "idle-lead-wait",
        "threads.wait",
        json!({"threadId":child}),
        cx,
    );
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                assert!(!v.busy);
                v.stop_if_busy(cx);
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert!(result.try_recv().unwrap().unwrap_err().contains("stopped"));
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.sessions[&child].read(cx).busy)
        })
        .unwrap();
}

#[gpui::test]
fn threads_pending_send_cannot_outlive_stop_or_a_new_lead_turn(cx: &mut TestAppContext) {
    for stop in [true, false] {
        let (window, child, lead) = setup(cx);
        let (reply, result) = mpsc::channel();
        window
            .update(cx, |shell, window, cx| {
                shell.sessions[&child].update(cx, |v, _| v.busy = false);
                shell.handle_app_call(
                    Call {
                        session: lead,
                        request_id: "stale-send".into(),
                        action: "threads.send".into(),
                        input: json!({"threadId":child,"prompt":"Do not deliver"}),
                        reply,
                    },
                    window,
                    cx,
                );
                shell.sessions[&lead].update(cx, |v, cx| {
                    if stop {
                        v.stop_if_busy(cx)
                    } else {
                        v.submission_generation += 1
                    }
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            result
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("stopped or started another turn")
        );
        window
            .update(cx, |shell, _, cx| {
                assert!(!shell.sessions[&child].read(cx).busy)
            })
            .unwrap();
    }
}

#[gpui::test]
fn threads_close_archives_a_finished_child_and_takes_it_off_the_rail(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let close = |request: &str, cx: &mut TestAppContext| {
        call(
            window,
            lead,
            request,
            "threads.close",
            json!({"threadId":child}),
            cx,
        )
        .try_recv()
        .unwrap()
    };
    assert!(close("busy", cx).unwrap_err().contains("stop it first"));
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, cx| {
                v.busy = false;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(close("close", cx).unwrap()["closed"], true);
    cx.run_until_parked();
    assert_eq!(close("again", cx).unwrap()["closed"], true);
    let listed = call(window, lead, "list", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(listed["threads"], json!([]));
    let send = call(
        window,
        lead,
        "send",
        "threads.send",
        json!({"threadId":child,"prompt":"More"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(send.contains("closed"));
    window
        .update(cx, |shell, _, cx| {
            let session = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == child)
                .unwrap();
            assert!(session.archived);
            assert!(
                !shell.agent_summaries[&lead]
                    .tasks
                    .iter()
                    .any(|t| t.session == Some(child))
            );
            let transcript = shell.sessions[&lead].read(cx).transcript.clone();
            assert!(transcript.read(cx).thread_hidden(child));
            // Restoring it from the archive puts it back on the lead's rail.
            shell.toggle_archive(child, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(
                shell.agent_summaries[&lead]
                    .tasks
                    .iter()
                    .any(|t| t.session == Some(child))
            );
            // Removing it from the lead's spawn tree closes it too.
            let transcript = shell.sessions[&lead].read(cx).transcript.clone();
            transcript.update(cx, |t, cx| t.dismiss_threads(&[child], cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.session_metadata(child, cx).summary.archived);
        })
        .unwrap();
}

fn evict(window: WindowHandle<Shell>, child: i64, cx: &mut TestAppContext) {
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, cx| {
                v.busy = false;
                let row = Row {
                    role: "user".into(),
                    text: "Done".into(),
                    first: true,
                    sent_at: None,
                };
                v.transcript.update(cx, |t, cx| {
                    t.visible(false, cx);
                    t.patch(0, vec![row], cx);
                });
                assert!(v.unload_history(cx));
            });
        })
        .unwrap();
}

/// Every thread call restores an evicted history, and that must not read as work.
#[gpui::test]
fn a_finished_child_with_evicted_history_stays_idle(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    evict(window, child, cx);
    let (reply, result) = mpsc::channel();
    window
        .update(cx, |shell, window, cx| {
            shell.handle_app_call(
                Call {
                    session: lead,
                    request_id: "read".into(),
                    action: "threads.wait".into(),
                    input: json!({"threadId":child}),
                    reply,
                },
                window,
                cx,
            );
            assert!(
                shell.sessions[&child]
                    .read(cx)
                    .external_submission_pending()
            );
            assert_eq!(shell.thread_status(child, cx)["status"], "idle");
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(result.recv().unwrap().unwrap()["status"], "idle");

    evict(window, child, cx);
    let closed = call(
        window,
        lead,
        "close",
        "threads.close",
        json!({"threadId":child}),
        cx,
    );
    assert_eq!(closed.recv().unwrap().unwrap()["closed"], true);
}

#[gpui::test]
fn a_message_to_an_evicted_child_waits_for_its_history(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    evict(window, child, cx);
    let sent = call(
        window,
        lead,
        "send",
        "threads.send",
        json!({"threadId":child,"prompt":"More"}),
        cx,
    );
    assert!(sent.recv().unwrap().is_ok());
}

#[gpui::test]
fn thread_read_carries_the_reply_once(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    evict(window, child, cx);
    let read = call(
        window,
        lead,
        "read",
        "threads.read",
        json!({"threadId":child}),
        cx,
    );
    let read = read.recv().unwrap().unwrap();
    assert!(read["thread"].get("reply").is_none());
    assert_eq!(read["thread"]["status"], "idle");
}

fn second_child(window: WindowHandle<Shell>, lead: i64, cx: &mut TestAppContext) -> i64 {
    let id = window
        .update(cx, |shell, _, cx| {
            let other = locked(&shell.store)
                .unwrap()
                .create("codex", &shell.workspace.project)
                .unwrap();
            let id = other.id;
            shell.thread_parents.insert(id, lead);
            shell.started_by.insert(id, lead);
            shell.add(other, cx);
            let view = shell.ensure_session(id, cx).unwrap();
            view.update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.driven_by = Some(lead);
                v.managed = true;
                v.busy = true;
            });
            shell.thread_session_changed(id, cx);
            id
        })
        .unwrap();
    cx.run_until_parked();
    id
}

fn edit_row(path: &str) -> Row {
    let mut call = monocode_core::tool::ToolCall::new(path, "Edit");
    call.input = json!({"file_path":path}).to_string();
    call.state = monocode_core::tool::ToolState::Completed;
    Row {
        role: "tool".into(),
        text: serde_json::to_string(&call).unwrap(),
        first: true,
        sent_at: None,
    }
}

fn finish(
    window: WindowHandle<Shell>,
    child: i64,
    reply: &str,
    edits: &[&str],
    cx: &mut TestAppContext,
) {
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, cx| {
                v.busy = false;
                let mut rows = vec![Row {
                    role: "user".into(),
                    text: "Review".into(),
                    first: true,
                    sent_at: None,
                }];
                rows.extend(edits.iter().map(|path| edit_row(path)));
                rows.push(Row {
                    role: "assistant".into(),
                    text: reply.into(),
                    first: true,
                    sent_at: None,
                });
                v.transcript.update(cx, |t, cx| t.replace_history(rows, cx));
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
}

fn lead_turns(window: WindowHandle<Shell>, lead: i64, cx: &mut TestAppContext) -> u64 {
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].read(cx).turn_generation()
        })
        .unwrap()
}

fn lead_said(window: WindowHandle<Shell>, lead: i64, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |shell, _, cx| {
            let transcript = shell.sessions[&lead].read(cx).transcript.read(cx);
            transcript
                .source_rows()
                .iter()
                .map(|row| row.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap()
}

#[gpui::test]
fn a_lead_hears_once_when_all_its_subagents_have_finished(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    let other = second_child(window, lead, cx);
    let before = lead_turns(window, lead, cx);
    finish(window, child, "First result", &[], cx);
    assert_eq!(lead_turns(window, lead, cx), before, "one is still working");
    finish(window, other, "Second result", &[], cx);
    assert_eq!(lead_turns(window, lead, cx), before + 1);
    let said = lead_said(window, lead, cx);
    assert!(said.contains("First result") && said.contains("Second result"));
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.thread_news.get(&lead).is_none());
            shell.sessions[&lead].update(cx, |v, _| v.busy = false);
            shell.thread_session_changed(lead, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(lead_turns(window, lead, cx), before + 1, "said only once");
}

#[gpui::test]
fn a_result_the_lead_already_collected_sends_no_message(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    let before = lead_turns(window, lead, cx);
    let waiting = call(
        window,
        lead,
        "collect",
        "threads.wait",
        json!({"threadId":child}),
        cx,
    );
    finish(window, child, "Collected", &[], cx);
    assert_eq!(waiting.try_recv().unwrap().unwrap()["reply"], "Collected");
    assert_eq!(lead_turns(window, lead, cx), before);
}

#[gpui::test]
fn a_busy_or_drafting_lead_hears_after_it_is_free(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, _| v.busy = true);
        })
        .unwrap();
    let before = lead_turns(window, lead, cx);
    finish(window, child, "Held result", &[], cx);
    assert_eq!(lead_turns(window, lead, cx), before);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.busy = false;
                v.input.update(cx, |input, cx| {
                    input.set_text("Unsent words".into(), cx);
                });
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        lead_turns(window, lead, cx),
        before,
        "the composer is the user's"
    );
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.input.update(cx, |input, cx| {
                    input.set_text(String::new(), cx);
                });
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(lead_turns(window, lead, cx), before + 1);
    assert!(lead_said(window, lead, cx).contains("Held result"));
}

#[gpui::test]
fn stopped_subagents_and_stopped_leads_send_no_message(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    let before = lead_turns(window, lead, cx);
    assert!(
        call(
            window,
            lead,
            "stop-child",
            "threads.stop",
            json!({"threadId":child}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, _| v.end_stopped_turn());
        })
        .unwrap();
    finish(window, child, "Never mind", &[], cx);
    assert_eq!(lead_turns(window, lead, cx), before);
    window
        .update(cx, |shell, _, cx| {
            let view = shell.sessions[&child].read(cx);
            assert!(!view.thread_pending(), "the child's turn really ended");
            assert!(shell.thread_news.get(&lead).is_none());
        })
        .unwrap();

    let other = second_child(window, lead, cx);
    let third = second_child(window, lead, cx);
    finish(window, other, "Held for the lead", &[], cx);
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.thread_news[&lead].contains_key(&other));
            shell.sessions[&lead].update(cx, |v, cx| {
                v.busy = true;
                v.stop_if_busy(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    finish(window, third, "Also held", &[], cx);
    assert_eq!(lead_turns(window, lead, cx), before);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.thread_news.get(&lead).is_none());
        })
        .unwrap();
}

#[gpui::test]
fn closing_a_finished_child_drops_its_result(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    let other = second_child(window, lead, cx);
    finish(window, child, "Not wanted", &[], cx);
    let before = lead_turns(window, lead, cx);
    window
        .update(cx, |shell, _, cx| shell.close_threads(lead, &[child], cx))
        .unwrap();
    finish(window, other, "Wanted", &[], cx);
    assert_eq!(lead_turns(window, lead, cx), before + 1);
    let said = lead_said(window, lead, cx);
    assert!(said.contains("Wanted") && !said.contains("Not wanted"));
}

#[gpui::test]
fn one_wait_covers_several_children_and_returns_at_the_first_finish(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let other = second_child(window, lead, cx);
    let ids = json!({"threadIds":[child, other]});
    let waiting = call(window, lead, "many", "threads.wait", ids.clone(), cx);
    assert!(waiting.try_recv().is_err());
    finish(window, other, "Fast one", &[], cx);
    let result = waiting.try_recv().unwrap().unwrap();
    assert_eq!(result["threads"][0]["status"], "working");
    assert_eq!(result["threads"][1]["status"], "idle");
    assert_eq!(result["threads"][1]["reply"], "Fast one");

    let at_once = call(window, lead, "many-now", "threads.wait", ids, cx)
        .try_recv()
        .expect("one has already finished")
        .unwrap();
    assert_eq!(at_once["threads"][1]["reply"], "Fast one");

    let timed = call(
        window,
        lead,
        "many-timeout",
        "threads.wait",
        json!({"threadIds":[child],"timeoutSeconds":2}),
        cx,
    );
    assert!(timed.try_recv().is_err());
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(
        timed.try_recv().unwrap().unwrap()["threads"][0]["status"],
        "working"
    );
    let denied = call(
        window,
        lead,
        "many-denied",
        "threads.wait",
        json!({"threadIds":[child, lead]}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(denied.contains("own children"));
}

#[gpui::test]
fn stopping_the_lead_releases_a_wait_on_several_children(cx: &mut TestAppContext) {
    let (window, child, lead) = setup(cx);
    let other = second_child(window, lead, cx);
    let waiting = call(
        window,
        lead,
        "many-stop",
        "threads.wait",
        json!({"threadIds":[child, other]}),
        cx,
    );
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.busy = true;
                v.stop_if_busy(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        waiting
            .try_recv()
            .unwrap()
            .unwrap_err()
            .contains("Lead thread was stopped")
    );
}

#[gpui::test]
fn status_shows_progress_changed_files_and_overlaps(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    let other = second_child(window, lead, cx);
    let root = window
        .update(cx, |shell, _, cx| {
            shell.session_working_copy(child, cx).to_owned()
        })
        .unwrap();
    let shared = format!("{root}/src/shared.rs");
    let own = format!("{root}/src/own.rs");
    let other_root = window
        .update(cx, |shell, _, cx| {
            shell.session_working_copy(other, cx).to_owned()
        })
        .unwrap();
    assert_ne!(root, other_root);
    finish(window, child, "Edited", &[&shared, &own], cx);
    let elsewhere = format!("{other_root}/src/shared.rs");
    finish(window, other, "Edited elsewhere", &[&elsewhere], cx);
    let apart = call(window, lead, "apart", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(
        apart["threads"][1]["changedFiles"],
        json!(["src/shared.rs"])
    );
    assert!(
        apart["threads"][0].get("overlaps").is_none(),
        "the same path in another checkout is another file"
    );
    finish(window, other, "Edited too", &[&shared], cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&other].update(cx, |v, _| v.settings.worktree_cwd = Some(root.clone()));
            shell.session_worktrees.insert(other, Some(root.clone()));
        })
        .unwrap();
    let value = call(window, lead, "progress", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    let first = &value["threads"][0];
    assert_eq!(first["threadId"], child);
    assert_eq!(first["access"], "supervised");
    assert_eq!(first["steps"], 2);
    assert!(first["elapsedSeconds"].is_u64());
    assert_eq!(
        first["changedFiles"],
        json!(["src/own.rs", "src/shared.rs"])
    );
    assert_eq!(first["overlaps"][0]["threadId"], other);
    assert_eq!(first["overlaps"][0]["files"], json!(["src/shared.rs"]));
    assert_eq!(value["threads"][1]["overlaps"][0]["threadId"], child);
    assert_eq!(value["threads"][1]["worktree"], root.as_str());

    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&child].update(cx, |v, cx| {
                v.busy = true;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let value = call(window, lead, "live", "threads.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert!(
        value["threads"][0]["activity"]
            .as_str()
            .unwrap()
            .contains("own.rs")
            || value["threads"][0]["activity"]
                .as_str()
                .unwrap()
                .contains("shared.rs")
    );
}

#[gpui::test]
fn a_subagent_can_have_less_access_than_its_lead_but_never_more(cx: &mut TestAppContext) {
    let (window, _, lead) = crate::shell::tests::window(cx);
    let missing = std::env::temp_dir()
        .join(monocode_core::mono::new_id())
        .join("missing-project");
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.summary.cwd = missing.to_string_lossy().into_owned();
            });
        })
        .unwrap();
    let denied = call(
        window,
        lead,
        "more-access",
        "threads.start",
        json!({"prompt":"Review","access":"full-access"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(denied.contains("more access"), "{denied}");
    let started = call(
        window,
        lead,
        "less-access",
        "threads.start",
        json!({"prompt":"Review","access":"read-only"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(started["access"], "read-only");
    let child = started["threadId"].as_i64().unwrap();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(
                shell.sessions[&child].read(cx).settings.access,
                monocode_core::agent::AccessMode::ReadOnly
            );
            assert_eq!(
                shell.sessions[&lead].read(cx).settings.access,
                monocode_core::agent::AccessMode::Supervised
            );
        })
        .unwrap();
    let denied = call(
        window,
        lead,
        "new-access",
        "threads.new",
        json!({"prompt":"Review","access":"read-only"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(denied.contains("does not take access"), "{denied}");
}

#[gpui::test]
fn a_worktree_subagent_says_where_it_works(cx: &mut TestAppContext) {
    let (window, _, lead) = crate::shell::tests::window(cx);
    let base = std::env::temp_dir().join(monocode_core::mono::new_id());
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&project)
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "start"]);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&lead].update(cx, |v, cx| {
                v.install_transfer(Settings::default(), None, cx);
                v.summary.cwd = project.to_string_lossy().into_owned();
            });
        })
        .unwrap();
    // A draft shares thread_start's launch without running a real turn.
    let reply = call(
        window,
        lead,
        "worktree",
        "threads.new",
        json!({"prompt":"Fix the parser","workspaceMode":"worktree","draft":true}),
        cx,
    );
    // Creating the worktree is real work outside the test clock.
    let started = (0..200)
        .find_map(|_| {
            cx.run_until_parked();
            let result = reply.try_recv().ok();
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(25));
            }
            result
        })
        .expect("no reply")
        .unwrap();
    let worktree = started["worktree"].as_str().unwrap();
    assert_eq!(started["cwd"], worktree);
    assert!(std::path::Path::new(worktree).is_dir());
    assert!(!started["branch"].as_str().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// Opening a finished subagent loads its history, which reads as working for
/// a moment. That is not a turn, so its lead must not hear about it again.
#[gpui::test]
fn loading_a_finished_child_is_not_a_turn(cx: &mut TestAppContext) {
    let (window, child, lead) = watched(cx);
    let waiting = call(
        window,
        lead,
        "seen",
        "threads.wait",
        json!({"threadId":child}),
        cx,
    );
    finish(window, child, "Done once", &[], cx);
    assert!(waiting.try_recv().unwrap().is_ok());
    let before = lead_turns(window, lead, cx);
    for loading in [true, false] {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&child].update(cx, |v, cx| {
                    v.set_loading(loading);
                    cx.emit(SessionChanged);
                });
            })
            .unwrap();
        cx.run_until_parked();
    }
    assert_eq!(lead_turns(window, lead, cx), before);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.thread_news[&lead].is_empty());
        })
        .unwrap();
}
