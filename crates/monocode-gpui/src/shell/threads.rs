use super::*;
use monocode_core::{
    app_access as app,
    mono::bridge::Call,
    subagents::{Summary, SummaryTask},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// What the finish message may carry across all its subagents.
const NEWS_CHARS: usize = 48_000;
const LISTED_FILES: usize = 30;
const LISTED_OVERLAPS: usize = 10;

#[derive(Clone, Copy)]
pub(super) struct ThreadTurn {
    started: u64,
    finished: Option<u64>,
}

/// The children a call names: `threadIds` on a wait, or its `threadId`.
fn called_threads(call: &Call) -> Vec<i64> {
    match call.input["threadIds"].as_array() {
        Some(ids) => ids.iter().filter_map(Value::as_i64).collect(),
        None => call.input["threadId"].as_i64().into_iter().collect(),
    }
}

/// The message that gives a lead the results of its finished subagents.
fn finish_message(news: &BTreeMap<i64, Value>) -> String {
    let share = (NEWS_CHARS / news.len().max(1)).max(2_000);
    let body = news
        .iter()
        .map(|(id, report)| {
            let outcome = if report["success"] == false {
                "failed"
            } else {
                "completed"
            };
            let title = report["title"].as_str().unwrap_or("Subagent");
            let mut notes = String::new();
            if let Some(worktree) = report["worktree"].as_str() {
                notes.push_str(&format!("\nWorktree: {worktree}"));
            }
            let files = |value: &Value| {
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if report["changedFiles"].is_array() {
                let listed = files(&report["changedFiles"]);
                notes.push_str(&format!("\nChanged: {listed}"));
            }
            for overlap in report["overlaps"].as_array().into_iter().flatten() {
                notes.push_str(&format!(
                    "\nAlso changed by thread {}: {}",
                    overlap["threadId"],
                    files(&overlap["files"])
                ));
            }
            let reply = report["reply"].as_str().unwrap_or_default().trim();
            format!(
                "## {title} (thread {id}) [{outcome}]{notes}\n{}",
                monocode_core::response::limit_middle(reply, share)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Your subagents have finished, so you don't need thread_wait for them. Check their work, then tell the user the outcome.\n\n{body}"
    )
}

impl Shell {
    pub(super) fn authorize_thread_caller(&self, call: &Call, cx: &App) -> Result<(), String> {
        let caller = self
            .sessions
            .get(&call.session)
            .ok_or("Caller is not available")?
            .read(cx);
        if self.thread_parents.contains_key(&call.session)
            || self.former_worker(call.session)
            || caller.settings.mono.is_some()
            || self
                .monos
                .as_ref()
                .is_some_and(|m| m.read(cx).is_mono_session(call.session))
        {
            return Err("Only top-level threads can use thread tools".into());
        }
        if matches!(
            call.action.as_str(),
            "threads.start" | "threads.new" | "threads.send"
        ) && caller.thread_stopping()
        {
            return Err("Lead thread was stopped".into());
        }
        if called_threads(call)
            .iter()
            .any(|id| self.thread_parents.get(id) != Some(&call.session))
        {
            return Err("threadId must be one of your own children".into());
        }
        Ok(())
    }

    pub(super) fn thread_status(&self, id: i64, cx: &App) -> Value {
        let metadata = self.session_metadata(id, cx);
        let view = self.sessions.get(&id).map(|v| v.read(cx));
        let waiting = view.and_then(|v| v.mobile_waiting(cx));
        let pending = view.is_some_and(|v| v.thread_pending());
        let outcome = if !pending && waiting.is_none() {
            view.filter(|v| v.history_loaded())
                .map(|v| v.thread_outcome(cx))
                .or_else(|| {
                    self.agent_summaries
                        .values()
                        .flat_map(|s| &s.tasks)
                        .find(|t| {
                            t.session == Some(id)
                                && matches!(t.status.as_str(), "completed" | "failed")
                        })
                        .map(|t| (t.status == "completed", String::new()))
                })
        } else {
            None
        };
        json!({"threadId":id,"title":metadata.summary.title,"harness":metadata.summary.provider,
            "model":view.and_then(|v| v.settings.model.clone()).or_else(||self.session_details.get(&id).and_then(|m|m.model.clone())),
            "status":if waiting.is_some(){"needsInput"}else if pending{"working"}else{"idle"},
            "pendingApproval":matches!(waiting,Some(crate::conversation::mobile::Waiting::Approval)),
            "pendingQuestion":matches!(waiting,Some(crate::conversation::mobile::Waiting::Question)),
            "success":outcome.as_ref().map(|(success,_)|success),"reply":outcome.as_ref().map(|(_,reply)|reply)})
    }

    /// What a lead is told about its children: `thread_status` plus how the
    /// work is going. It reads their transcripts, so only tool replies use it.
    fn thread_reports(&self, lead: i64, ids: &[i64], cx: &App) -> Vec<Value> {
        let edits: Vec<(i64, &str, Vec<String>)> = self
            .thread_parents
            .iter()
            .filter(|(child, parent)| **parent == lead && !self.thread_closed(**child, cx))
            .filter_map(|(&child, _)| {
                let view = self.sessions.get(&child)?.read(cx);
                let root = self.session_working_copy(child, cx);
                let mut files: Vec<String> = view
                    .edited_files(cx)
                    .iter()
                    .map(|path| {
                        super::app_control::checkout_path(path, Path::new(root))
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                files.sort_unstable();
                files.dedup();
                Some((child, root, files))
            })
            .collect();
        ids.iter()
            .map(|&id| {
                let mut report = self.thread_status(id, cx);
                if let Some(turn) = self.thread_turns.get(&id) {
                    let end = turn.finished.unwrap_or_else(monocode_core::time::now_ms);
                    report["elapsedSeconds"] = (end.saturating_sub(turn.started) / 1000).into();
                }
                let Some(view) = self.sessions.get(&id).map(|view| view.read(cx)) else {
                    return report;
                };
                report["access"] = view.settings.access.id().into();
                if let Some(worktree) = &view.settings.worktree_cwd {
                    report["worktree"] = worktree.as_str().into();
                }
                if !view.history_loaded() {
                    return report;
                }
                report["steps"] = view.turn_steps(cx).into();
                if report["status"] == "working" {
                    report["activity"] = view.turn_activity(cx).into();
                }
                let Some((_, root, files)) = edits.iter().find(|(child, ..)| *child == id) else {
                    return report;
                };
                if !files.is_empty() {
                    report["changedFiles"] = json!(&files[..files.len().min(LISTED_FILES)]);
                }
                if files.len() > LISTED_FILES {
                    report["moreChangedFiles"] = (files.len() - LISTED_FILES).into();
                }
                let overlaps: Vec<Value> = edits
                    .iter()
                    .filter(|(other, other_root, _)| *other != id && other_root == root)
                    .filter_map(|(other, _, theirs)| {
                        let shared: Vec<_> = files
                            .iter()
                            .filter(|file| theirs.contains(file))
                            .take(LISTED_OVERLAPS)
                            .collect();
                        (!shared.is_empty()).then(|| json!({"threadId":other,"files":shared}))
                    })
                    .collect();
                if !overlaps.is_empty() {
                    report["overlaps"] = overlaps.into();
                }
                report
            })
            .collect()
    }

    /// A lead that has seen a finished child's result needs no message for it.
    fn thread_results_seen(&mut self, lead: i64, reports: &[Value]) {
        let Some(news) = self.thread_news.get_mut(&lead) else {
            return;
        };
        for report in reports.iter().filter(|report| report["status"] == "idle") {
            if let Some(id) = report["threadId"].as_i64() {
                news.remove(&id);
            }
        }
    }

    fn thread_wait_result(&mut self, call: &Call, cx: &App) -> Value {
        let mut reports = self.thread_reports(call.session, &called_threads(call), cx);
        self.thread_results_seen(call.session, &reports);
        if call.input.get("threadIds").is_some() {
            json!({"threads":reports})
        } else {
            reports.pop().unwrap_or_default()
        }
    }

    /// Follows a child's turns, and holds its result for the lead when a turn
    /// ends. A stopped child has nothing to report.
    fn note_thread_turn(&mut self, id: i64, status: &Value, cx: &App) {
        let Some(&lead) = self.thread_parents.get(&id) else {
            return;
        };
        let now = monocode_core::time::now_ms();
        if status["status"] != "idle" {
            let active = self
                .sessions
                .get(&id)
                .is_some_and(|view| view.read(cx).thread_turn_active());
            if !active {
                return;
            }
            if self
                .thread_turns
                .get(&id)
                .is_none_or(|turn| turn.finished.is_some())
            {
                self.thread_turns.insert(
                    id,
                    ThreadTurn {
                        started: now,
                        finished: None,
                    },
                );
            }
            if let Some(news) = self.thread_news.get_mut(&lead) {
                news.remove(&id);
            }
            return;
        }
        let Some(turn) = self
            .thread_turns
            .get_mut(&id)
            .filter(|turn| turn.finished.is_none())
        else {
            return;
        };
        turn.finished = Some(now);
        if self
            .sessions
            .get(&id)
            .is_some_and(|view| view.read(cx).notification_was_cancelled())
        {
            return;
        }
        if let Some(report) = self.thread_reports(lead, &[id], cx).pop() {
            self.thread_news.entry(lead).or_default().insert(id, report);
        }
    }

    /// Gives the lead its subagents' results, once none is still running and
    /// the lead is free. Until then they wait for the next change to either.
    fn send_thread_news(&mut self, lead: i64, cx: &mut Context<Self>) {
        if self.thread_news.get(&lead).is_none_or(BTreeMap::is_empty) {
            return;
        }
        let Some(view) = self.sessions.get(&lead).cloned() else {
            return;
        };
        if view.read(cx).summary.archived {
            self.thread_news.remove(&lead);
            return;
        }
        let settled = self
            .thread_parents
            .iter()
            .filter(|(child, parent)| **parent == lead && !self.thread_closed(**child, cx))
            .all(|(child, _)| self.thread_status(*child, cx)["status"] == "idle");
        let free = view.read(cx).available_for_managed_run(cx) && !view.read(cx).has_saved_draft();
        if !settled || !free {
            return;
        }
        let Some(news) = self.thread_news.remove(&lead) else {
            return;
        };
        // No retry: a failed launch changes the session, which would land
        // here again. The results stay readable through thread_list.
        if !view.update(cx, |v, cx| {
            v.send_coordinator_report(finish_message(&news), cx)
        }) {
            let status = view.read(cx).status_text().to_owned();
            self.error = Some(format!("Could not send subagent results: {status}"));
            cx.notify();
        }
    }

    pub(super) fn forget_thread(&mut self, id: i64) {
        self.thread_waiters
            .retain(|call| !called_threads(call).contains(&id));
        self.thread_turns.remove(&id);
        self.thread_news.remove(&id);
        for news in self.thread_news.values_mut() {
            news.remove(&id);
        }
    }

    pub(super) fn dispatch_thread_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let ids = called_threads(&call);
        let id = ids.first().copied().unwrap_or_default();
        if call.action != "threads.list" {
            for &id in &ids {
                let Some(view) = self.ensure_session(id, cx) else {
                    self.finish_app_call(call, Err("Child thread was not found".into()), cx);
                    return;
                };
                view.update(cx, |v, cx| v.load(cx));
            }
        }
        let result = match call.action.as_str() {
            "threads.list" => {
                let mut ids: Vec<_> = self
                    .thread_parents
                    .iter()
                    .filter(|(_, parent)| **parent == call.session)
                    .map(|(id, _)| *id)
                    .filter(|id| {
                        self.session_summaries.contains_key(id) && !self.thread_closed(*id, cx)
                    })
                    .collect();
                ids.sort_unstable();
                let reports = self.thread_reports(call.session, &ids, cx);
                self.thread_results_seen(call.session, &reports);
                Ok(json!({"threads":reports}))
            }
            "threads.read" => {
                let reports = self.thread_reports(call.session, &[id], cx);
                self.thread_results_seen(call.session, &reports);
                let mut status = reports.into_iter().next().unwrap_or_default();
                // The page already carries the last reply.
                status.as_object_mut().map(|status| status.remove("reply"));
                let input = call.input.clone();
                let busy = status["status"] != "idle";
                self.app_background(
                    call,
                    move |store| {
                        let mut value =
                            locked(&store)?.app_conversation(id, &input, busy, false)?;
                        value["thread"] = status;
                        Ok((value, super::app_commands::Effect::None))
                    },
                    cx,
                );
                return;
            }
            "threads.wait" => {
                let seconds = call.input["timeoutSeconds"].as_u64().unwrap_or(50);
                let working = ids
                    .iter()
                    .all(|id| self.thread_status(*id, cx)["status"] == "working");
                if !working || seconds == 0 {
                    Ok(self.thread_wait_result(&call, cx))
                } else {
                    let session = call.session;
                    let request = call.request_id.clone();
                    self.thread_waiters.push(call);
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs(seconds))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(index) = this.thread_waiters.iter().position(|call| {
                                call.session == session && call.request_id == request
                            }) {
                                let call = this.thread_waiters.remove(index);
                                let result = this.thread_wait_result(&call, cx);
                                this.finish_app_call(call, Ok(result), cx);
                            }
                        });
                    })
                    .detach();
                    return;
                }
            }
            "threads.send" => {
                let view = self.sessions[&id].clone();
                if self.thread_closed(id, cx) {
                    Err("Child thread was closed".into())
                } else if !view.read(cx).available_for_managed_run(cx)
                    || view.read(cx).thread_pending()
                    || view.read(cx).has_saved_draft()
                {
                    Err("Child is busy, loading, or has an unsent draft".into())
                } else {
                    app::prompt(&call.input).map(|prompt| {
                        let sender = call.session;
                        view.update(cx, |v, cx| v.send_thread_message(prompt, sender, cx));
                        json!({"threadId":id,"accepted":true,"submitted":true})
                    })
                }
            }
            "threads.stop" => {
                self.sessions[&id].update(cx, |v, cx| v.stop_app_session(cx));
                self.release_thread_waiters(
                    |call| called_threads(call).contains(&id),
                    Err("Child thread was stopped".into()),
                    cx,
                );
                Ok(json!({"threadId":id,"stopped":true}))
            }
            "threads.close" => {
                if self.thread_status(id, cx)["status"] != "idle" {
                    Err("Child is still working or needs input; stop it first".into())
                } else {
                    self.close_threads(call.session, &[id], cx);
                    Ok(json!({"threadId":id,"closed":true}))
                }
            }
            _ => Err("Unknown thread action".into()),
        };
        self.finish_app_call(call, result, cx);
    }

    /// A closed child is archived: off its lead's list, rail and spawn tree.
    fn thread_closed(&self, id: i64, cx: &App) -> bool {
        (self.sessions.contains_key(&id) || self.session_summaries.contains_key(&id))
            && self.session_metadata(id, cx).summary.archived
    }

    /// Archives the settled children of `lead` among `ids`; a child still
    /// working or waiting for the user is left open.
    pub(super) fn close_threads(&mut self, lead: i64, ids: &[i64], cx: &mut Context<Self>) {
        let ids: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.thread_parents.get(id) == Some(&lead)
                    && self.session_summaries.contains_key(id)
                    && !self.thread_closed(*id, cx)
                    && self.thread_status(*id, cx)["status"] == "idle"
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        for &id in &ids {
            self.set_thread_archived(id, true, cx);
            if let Some(news) = self.thread_news.get_mut(&lead) {
                news.remove(&id);
            }
        }
        self.thread_session_changed(lead, cx);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            match locked(&store) {
                Ok(store) => ids
                    .into_iter()
                    .filter_map(|id| store.set_archived(id, true).err().map(|e| (id, e)))
                    .collect::<Vec<_>>(),
                Err(error) => ids.into_iter().map(|id| (id, error.clone())).collect(),
            }
        });
        cx.spawn(async move |this, cx| {
            let failed = work.await;
            let Some((_, error)) = failed.first().cloned() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                for (id, _) in &failed {
                    this.set_thread_archived(*id, false, cx);
                }
                this.thread_session_changed(lead, cx);
                this.error = Some(format!("Could not close agents: {error}"));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn set_thread_archived(&mut self, id: i64, archived: bool, cx: &mut Context<Self>) {
        if let Some(summary) = self.session_summaries.get_mut(&id) {
            summary.archived = archived;
        }
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |session, cx| {
                session.summary.archived = archived;
                if archived {
                    session.cancel_usage_resume(cx);
                }
            });
        }
        self.refresh_session_mark(id, cx);
    }

    fn take_thread_waiters(&mut self, wanted: impl Fn(&Call) -> bool) -> Vec<Call> {
        let (taken, kept) = std::mem::take(&mut self.thread_waiters)
            .into_iter()
            .partition(|call| wanted(call));
        self.thread_waiters = kept;
        taken
    }

    fn release_thread_waiters(
        &mut self,
        wanted: impl Fn(&Call) -> bool,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        for call in self.take_thread_waiters(wanted) {
            self.finish_app_call(call, result.clone(), cx);
        }
    }

    pub(super) fn thread_session_changed(&mut self, id: i64, cx: &mut Context<Self>) {
        let lead = self.thread_parents.get(&id).copied().unwrap_or(id);
        let mut all: Vec<_> = self
            .thread_parents
            .iter()
            .filter(|(_, parent)| **parent == lead)
            .map(|(id, _)| *id)
            .filter(|id| self.session_summaries.contains_key(id))
            .collect();
        if all.is_empty() {
            return;
        }
        all.sort_unstable();
        // Closed children stay in the spawn tree's states, marked so it hides
        // them, but leave the rail.
        let children: Vec<i64> = all
            .iter()
            .copied()
            .filter(|&child| !self.thread_closed(child, cx))
            .collect();
        let stopped = self
            .sessions
            .get(&lead)
            .is_none_or(|v| v.read(cx).thread_stopping());
        if stopped {
            self.thread_news.remove(&lead);
            for &child in &all {
                if id == lead
                    && let Some(view) = self.sessions.get(&child)
                {
                    view.update(cx, |v, cx| v.stop_app_session(cx));
                }
            }
            self.release_thread_waiters(
                |call| call.session == lead,
                Err("Lead thread was stopped".into()),
                cx,
            );
        } else {
            let status = self.thread_status(id, cx);
            self.note_thread_turn(id, &status, cx);
            if status["status"] != "working" {
                for call in self.take_thread_waiters(|call| called_threads(call).contains(&id)) {
                    let result = self.thread_wait_result(&call, cx);
                    self.finish_app_call(call, Ok(result), cx);
                }
            }
        }
        let states: HashMap<_, _> = all
            .iter()
            .map(|&child| {
                let mut status = self.thread_status(child, cx);
                if self.thread_closed(child, cx) {
                    status["closed"] = true.into();
                }
                (child, status)
            })
            .collect();
        if let Some(view) = self.sessions.get(&lead) {
            let transcript = view.read(cx).transcript.clone();
            transcript.update(cx, |t, cx| t.set_thread_states(states, cx));
        }
        for &child in &children {
            self.name_thread_senders(child, cx);
        }
        let mut tasks: Vec<_> = children
            .iter()
            .map(|&child| {
                let status = self.thread_status(child, cx);
                SummaryTask {
                    session: Some(child),
                    title: status["title"].as_str().unwrap_or("Child thread").into(),
                    provider: status["harness"].as_str().unwrap_or_default().into(),
                    model: status["model"].as_str().map(str::to_owned),
                    status: if status["status"] == "needsInput" {
                        "blocked"
                    } else if status["status"] == "working" {
                        "running"
                    } else if status["success"] == true {
                        "completed"
                    } else if status["success"] == false {
                        "failed"
                    } else {
                        "saved"
                    }
                    .into(),
                }
            })
            .collect();
        let live = self.sessions.get(&lead).is_some_and(|v| v.read(cx).busy)
            || tasks
                .iter()
                .any(|t| matches!(t.status.as_str(), "running" | "blocked"));
        if let Some(previous) = self.agent_summaries.get(&lead) {
            tasks.extend(
                previous
                    .tasks
                    .iter()
                    .filter(|t| {
                        t.session
                            .is_none_or(|child| !self.thread_parents.contains_key(&child))
                    })
                    .cloned(),
            );
        }
        let summary = Summary { live, tasks };
        self.set_agent_summary(lead, summary, cx);
        self.send_thread_news(lead, cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }
    /// The lead a subagent belongs to: the thread that started it with
    /// thread_start, or the lead of a former worker. A thread opened with
    /// new_thread has none; it is the user's own.
    pub(crate) fn subagent_lead(&self, id: i64) -> Option<i64> {
        self.thread_parents
            .get(&id)
            .copied()
            .or_else(|| self.former_worker_lead(id))
    }
    /// A subagent opens in a pane beside its lead, never as a tab of its own,
    /// and one at a time: opening another takes the shown one's place.
    pub(super) fn open_subagent(
        &mut self,
        lead: i64,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation_revision += 1;
        if self.workspace.tab_of(id).is_some()
            && self.workspace.tab_of(id) == self.workspace.tab_of(lead)
        {
            self.activate_session(id, window, cx);
            return;
        }
        if !self.session_summaries.contains_key(&lead) || self.deleting.contains(&lead) {
            // Its lead is gone; show it on its own rather than not at all.
            self.activate_session(id, window, cx);
            return;
        }
        if self.workspace.active != Some(lead) {
            self.replace_blank_pane(lead, cx);
            self.activate_session(lead, window, cx);
        }
        let Some(tab) = self.workspace.tab_of(lead) else {
            return;
        };
        let before = self.active_leaves();
        let shown = self
            .workspace
            .layout(tab)
            .leaf_ids()
            .into_iter()
            .find(|&pane| pane != id && self.subagent_lead(pane) == Some(lead));
        match shown {
            Some(shown) => {
                self.workspace.place(id, shown, Edge::Right);
                self.workspace.close_pane(shown);
            }
            None => self.workspace.place(id, lead, Edge::Right),
        }
        self.begin_pane_motion(before, cx);
        self.fit_sidebar(cx);
        self.activate_session(id, window, cx);
    }
    /// The lead's subagents for the strip above its composer.
    pub(super) fn subagent_strip(
        &self,
        lead: i64,
        cx: &App,
    ) -> Vec<crate::conversation::subagents::Subagent> {
        use crate::conversation::subagents::{Subagent, SubagentState};
        let Some(summary) = self.agent_summaries.get(&lead) else {
            return Vec::new();
        };
        summary
            .tasks
            .iter()
            .filter_map(|task| {
                let id = task.session?;
                let needs_input = self
                    .sessions
                    .get(&id)
                    .is_some_and(|view| view.read(cx).needs_approval());
                Some(Subagent {
                    id,
                    title: task.title.clone(),
                    provider: task.provider.clone(),
                    state: SubagentState::of(&task.status, summary.live, needs_input),
                })
            })
            .collect()
    }
    pub(super) fn push_subagent_strip(&self, lead: i64, cx: &mut Context<Self>) {
        let Some(view) = self.sessions.get(&lead).cloned() else {
            return;
        };
        let strip = self.subagent_strip(lead, cx);
        view.update(cx, |v, cx| v.set_subagents(strip, cx));
    }
    /// Every subagent of the lead, archived or closed ones included: its
    /// thread_start children, then its former workers.
    pub(crate) fn subagents(&self, lead: i64) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .thread_parents
            .iter()
            .filter(|(_, parent)| **parent == lead)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        if let Some(summary) = self.agent_summaries.get(&lead) {
            for id in summary.tasks.iter().filter_map(|t| t.session) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }
    /// The thread that started `id`, as messages it sent there name it.
    pub(super) fn thread_senders(
        &self,
        id: i64,
    ) -> HashMap<i64, crate::transcript::threads::Sender> {
        self.started_by
            .get(&id)
            .and_then(|from| Some((*from, self.session_summaries.get(from)?)))
            .map(|(from, summary)| {
                let sender = crate::transcript::threads::Sender {
                    title: monocode_core::session_title::display_title(
                        &summary.title,
                        &summary.provider,
                    )
                    .replace('\n', " ")
                    .into(),
                    provider: summary.provider.clone(),
                };
                (from, sender)
            })
            .into_iter()
            .collect()
    }
    /// "from X" under a session another thread started.
    pub(super) fn started_from_line(&self, id: i64, cx: &App) -> Option<gpui::Div> {
        let from = *self.started_by.get(&id)?;
        if !self.session_summaries.contains_key(&from) && !self.sessions.contains_key(&from) {
            return None;
        }
        let title = self.session_metadata(from, cx).display_title();
        Some(
            div()
                .debug_selector(move || format!("session-origin-{id}"))
                .h(px(16.))
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(content(0.45))
                .child(icon("arrow-turn-down-right", 12.).text_color(content(0.4)))
                .child(div().min_w_0().truncate().child(format!("from {title}"))),
        )
    }
    pub(super) fn name_thread_senders(&self, id: i64, cx: &mut Context<Self>) {
        let senders = self.thread_senders(id);
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |v, cx| {
                v.transcript.update(cx, |t, cx| {
                    if t.senders != senders {
                        t.senders = senders;
                        cx.notify();
                    }
                })
            });
        }
    }
    pub(super) fn set_agent_summary(
        &mut self,
        lead: i64,
        summary: Summary,
        cx: &mut Context<Self>,
    ) {
        if self.agent_summaries.get(&lead) == Some(&summary) {
            return;
        }
        let previous = self.agent_summary_writes.remove(&lead);
        let store = self.store.clone();
        let saved = summary.clone();
        let write = cx.spawn(async move |_, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            cx.background_executor()
                .spawn(async move {
                    let _ = locked(&store).and_then(|s| s.save_agent_summary(lead, &saved));
                })
                .await;
        });
        self.agent_summary_writes.insert(lead, write);
        self.agent_summaries.insert(lead, summary);
        self.push_subagent_strip(lead, cx);
        self.invalidate_chrome(cx);
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "macos"))]
mod shots;
