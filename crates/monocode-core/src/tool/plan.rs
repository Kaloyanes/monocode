//! The session's current plan, exactly as its agent last wrote it with a todo
//! tool, for the title bar's plan rail. Never infer steps from prose: a Plan
//! mode proposal is kept whole, as the document the agent wrote.
use super::{
    tasks::{Panel, Task, TaskList, TaskStatus},
    ToolCall,
};
use crate::transcript::Row;
use serde_json::Value;

/// What the rail draws for one session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub plan: Option<Plan>,
    /// A Plan mode proposal newer than any todo list.
    pub proposal: Option<Proposal>,
    /// Tool calls since the latest prompt, counted up to the plan; the rail
    /// reads it only when there is none.
    pub turn_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub list: TaskList,
    /// Tool calls the agent made after it last touched its list.
    pub activity: usize,
    /// A prompt arrived after the list was written: it describes an earlier turn.
    pub stale: bool,
}

/// The plan an agent proposed for review in Plan mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The agent's Markdown, unchanged.
    pub text: String,
    /// Approved and being built: a prompt followed it, or Claude's
    /// ExitPlanMode was allowed and the agent went on working.
    pub building: bool,
}

impl Proposal {
    /// Its first heading, or else its first line, without Markdown marks.
    pub fn title(&self) -> String {
        let lines = || self.text.lines().map(str::trim).filter(|l| !l.is_empty());
        let line = lines()
            .find(|l| l.starts_with('#'))
            .or_else(|| lines().find(|l| !l.starts_with("```") && *l != "---"))
            .unwrap_or("Plan");
        let line = line
            .trim_start_matches('#')
            .trim_start_matches(|c: char| c == '-' || c == '*' || c.is_ascii_digit() || c == '.')
            .trim();
        line.chars()
            .filter(|c| !matches!(c, '*' | '`' | '_'))
            .take(80)
            .collect()
    }
}

impl Plan {
    pub fn done(&self) -> usize {
        self.list
            .items
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count()
    }
    /// Steps that still count towards the total; cancelled ones do not.
    pub fn actionable(&self) -> usize {
        self.list
            .items
            .iter()
            .filter(|t| t.status != TaskStatus::Cancelled)
            .count()
    }
    pub fn complete(&self) -> bool {
        let actionable = self.actionable();
        actionable > 0 && self.done() == actionable
    }
    pub fn current(&self) -> Option<&Task> {
        self.list
            .items
            .iter()
            .find(|t| t.status == TaskStatus::InProgress)
    }
}

/// Reads backwards from the newest row and stops at the plan or after the
/// previous turn, so a long history costs only its last two turns.
/// `planning` says the newest turn ran in Plan mode, so its final answer is
/// the proposal even when the provider marks it no other way.
pub fn snapshot(rows: &[Row], planning: bool) -> Snapshot {
    let mut prompts = 0;
    let mut turn_calls = 0;
    let mut activity = 0;
    let mut touched = false;
    // Claude's TaskCreate/TaskUpdate, newest first, folded once the scan ends.
    let mut increments: Vec<ToolCall> = Vec::new();
    let mut base: Option<(TaskList, usize)> = None;
    let mut proposal: Option<Proposal> = None;
    let mut answer: Option<String> = None;
    for (at, row) in rows.iter().enumerate().rev() {
        if row.is_prompt() {
            if row.first {
                prompts += 1;
                if prompts == 2 {
                    break;
                }
            }
            continue;
        }
        if !row.first {
            continue;
        }
        if row.role == "assistant" {
            if proposal.is_some() || touched {
                continue;
            }
            let end = (at + 1..rows.len())
                .find(|&next| rows[next].first)
                .unwrap_or(rows.len());
            let chunks = &rows[at..end];
            let tagged = chunks.iter().any(|r| r.text.contains(PROPOSED_OPEN));
            if !tagged && !(planning && prompts == 0 && answer.is_none()) {
                continue;
            }
            let text: String = chunks.iter().map(|r| r.text.as_str()).collect();
            if let Some(plan) = tagged.then(|| proposed_plan(&text)).flatten() {
                proposal = Some(Proposal {
                    text: plan,
                    building: prompts > 0,
                });
            } else if planning && prompts == 0 && !text.trim().is_empty() {
                answer = Some(text.trim().to_owned());
            }
            continue;
        }
        if row.role != "tool" {
            continue;
        }
        match classify(&row.text) {
            Kind::Call => {
                if prompts == 0 {
                    turn_calls += 1;
                }
                if !touched {
                    activity += 1;
                }
            }
            Kind::Milestone => {}
            Kind::Proposal => {
                if proposal.is_some() || touched {
                    continue;
                }
                let Ok(call) = serde_json::from_str::<ToolCall>(&row.text) else {
                    continue;
                };
                if let Some(text) = exit_plan_text(&call) {
                    // Allowed, then work followed in the same turn: building.
                    let approved = call.state == super::ToolState::Completed && activity > 0;
                    proposal = Some(Proposal {
                        text,
                        building: prompts > 0 || approved,
                    });
                }
            }
            Kind::Increment => {
                if let Ok(call) = serde_json::from_str::<ToolCall>(&row.text) {
                    touched = true;
                    increments.push(call);
                }
            }
            Kind::List => {
                let list = serde_json::from_str::<ToolCall>(&row.text)
                    .ok()
                    .and_then(|call| match call.panel() {
                        Some(Panel::Tasks(list)) => Some(list),
                        _ => None,
                    });
                if let Some(list) = list {
                    base = Some((list, prompts));
                    break;
                }
                if prompts == 0 {
                    turn_calls += 1;
                }
                if !touched {
                    activity += 1;
                }
            }
        }
    }
    let proposal = proposal.or_else(|| {
        answer.map(|text| Proposal {
            text,
            building: false,
        })
    });
    let (mut list, stale) = match base {
        Some((list, prompts)) => (
            list,
            prompts > 0 && (increments.is_empty() || increments_before_prompt(rows)),
        ),
        None if increments.is_empty() => {
            return Snapshot {
                plan: None,
                proposal,
                turn_calls,
            };
        }
        None => (TaskList::default(), increments_before_prompt(rows)),
    };
    for call in increments.iter().rev() {
        fold(&mut list, call);
    }
    if list.items.is_empty() {
        return Snapshot {
            plan: None,
            proposal,
            turn_calls,
        };
    }
    Snapshot {
        plan: Some(Plan {
            list,
            activity,
            stale,
        }),
        proposal,
        turn_calls,
    }
}

const PROPOSED_OPEN: &str = "<proposed_plan>";
const PROPOSED_CLOSE: &str = "</proposed_plan>";

/// Codex's Plan mode wraps the plan it proposes in `<proposed_plan>` tags.
fn proposed_plan(text: &str) -> Option<String> {
    let start = text.rfind(PROPOSED_OPEN)? + PROPOSED_OPEN.len();
    let body = &text[start..];
    let body = body.find(PROPOSED_CLOSE).map_or(body, |end| &body[..end]);
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_owned())
}

/// Claude's ExitPlanMode carries the plan as Markdown in its `plan` input.
fn exit_plan_text(call: &ToolCall) -> Option<String> {
    let raw: Value = serde_json::from_str(&call.input).ok()?;
    let input = if raw["__desktop"].is_object() {
        raw.get("rawInput").cloned().unwrap_or_default()
    } else {
        raw
    };
    input["plan"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Whether the newest TaskCreate/TaskUpdate predates the newest prompt.
fn increments_before_prompt(rows: &[Row]) -> bool {
    for row in rows.iter().rev() {
        if row.is_prompt() && row.first {
            return true;
        }
        if row.role == "tool" && row.first && classify(&row.text) == Kind::Increment {
            return false;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Call,
    Milestone,
    Proposal,
    List,
    Increment,
}

/// Decides from the serialized name, so ordinary tool calls (whose output can
/// be large) are never parsed. A provider can still attach a task panel to any
/// name; the panel's tag is the fallback check.
fn classify(text: &str) -> Kind {
    let name = name_of(text).unwrap_or_default();
    match name {
        "TaskCreate" | "TaskUpdate" => return Kind::Increment,
        super::HANDOFF | "contextCompaction" => return Kind::Milestone,
        "ExitPlanMode" => return Kind::Proposal,
        _ => {}
    }
    let normalized = name.to_lowercase().replace([' ', '-', '_'], "");
    let listed = ["todowrite", "writetodos", "updatetodos", "updateplan"]
        .iter()
        .any(|suffix| normalized == *suffix || normalized.ends_with(&format!(":{suffix}")));
    if listed || text.contains("\"panel\":{\"kind\":\"tasks\"") {
        Kind::List
    } else {
        Kind::Call
    }
}

fn name_of(text: &str) -> Option<&str> {
    const KEY: &str = "\"name\":\"";
    let start = text.find(KEY)? + KEY.len();
    let len = text[start..].find('"')?;
    Some(&text[start..start + len])
}

/// TaskCreate adds one item and learns its id from the result text
/// ("Task #3 created successfully: …"); TaskUpdate changes one item by id.
fn fold(list: &mut TaskList, call: &ToolCall) {
    let raw: Value = serde_json::from_str(&call.input).unwrap_or_default();
    let input = if raw["__desktop"].is_object() {
        raw.get("rawInput").cloned().unwrap_or_default()
    } else {
        raw
    };
    let text = |key: &str| {
        input[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if call.name == "TaskCreate" {
        let Some(subject) = text("subject")
            .or_else(|| text("activeForm"))
            .or_else(|| text("description"))
        else {
            return;
        };
        let Some(id) = created_id(&call.output) else {
            return;
        };
        list.items.retain(|t| t.id.as_deref() != Some(id.as_str()));
        list.items.push(Task {
            id: Some(id),
            text: subject,
            status: TaskStatus::Pending,
        });
        return;
    }
    let id = match &input["taskId"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().trim_start_matches('#').to_owned(),
        _ => return,
    };
    let Some(at) = list.items.iter().position(|t| t.id.as_deref() == Some(&id)) else {
        return;
    };
    match input["status"].as_str().map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("deleted") => {
            list.items.remove(at);
            return;
        }
        Some(s) => list.items[at].status = TaskStatus::parse(s),
        None => {}
    }
    if let Some(subject) = text("subject") {
        list.items[at].text = subject;
    }
}

fn created_id(output: &str) -> Option<String> {
    let rest = &output[output.find("Task #")? + "Task #".len()..];
    let id: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ':')
        .collect();
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolState;
    use serde_json::json;

    fn prompt(text: &str) -> Row {
        Row {
            role: "user".into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }
    fn tool(name: &str, input: Value, output: &str) -> Row {
        let mut call = ToolCall::new(&format!("{name}-{}", input), name);
        call.input = input.to_string();
        call.output = output.into();
        call.state = ToolState::Completed;
        Row {
            role: "tool".into(),
            text: serde_json::to_string(&call).unwrap(),
            first: true,
            sent_at: None,
        }
    }
    fn todos(items: &[(&str, &str)]) -> Row {
        let todos: Vec<Value> = items
            .iter()
            .map(|(text, status)| json!({"content": text, "status": status}))
            .collect();
        tool("TodoWrite", json!({ "todos": todos }), "")
    }
    fn read(path: &str) -> Row {
        tool(
            "Read",
            json!({ "file_path": path }),
            "x".repeat(4096).as_str(),
        )
    }
    fn statuses(plan: &Plan) -> Vec<TaskStatus> {
        plan.list.items.iter().map(|t| t.status).collect()
    }

    #[test]
    fn no_todo_call_is_no_plan_but_still_counts_the_turn() {
        let rows = [prompt("fix it"), read("a.rs"), read("b.rs")];
        let snap = snapshot(&rows, false);
        assert_eq!(snap.plan, None);
        assert_eq!(snap.turn_calls, 2);
    }

    #[test]
    fn the_newest_list_wins_and_later_calls_are_its_activity() {
        let rows = [
            prompt("go"),
            todos(&[("Read", "in_progress"), ("Edit", "pending")]),
            read("a.rs"),
            todos(&[("Read", "completed"), ("Edit", "in_progress")]),
            read("b.rs"),
            read("c.rs"),
        ];
        let plan = snapshot(&rows, false).plan.unwrap();
        assert_eq!(
            statuses(&plan),
            [TaskStatus::Completed, TaskStatus::InProgress]
        );
        assert_eq!(plan.activity, 2);
        assert!(!plan.stale);
        assert_eq!(plan.current().unwrap().text, "Edit");
    }

    #[test]
    fn a_new_prompt_makes_the_list_stale_and_an_older_turn_drops_it() {
        let mut rows = vec![
            prompt("one"),
            todos(&[("A", "completed")]),
            prompt("two"),
            read("x.rs"),
        ];
        let snap = snapshot(&rows, false);
        let plan = snap.plan.unwrap();
        assert!(plan.stale && plan.complete());
        assert_eq!(snap.turn_calls, 1);
        rows.push(prompt("three"));
        assert_eq!(snapshot(&rows, false).plan, None);
    }

    #[test]
    fn codex_plan_panels_are_read_from_any_tool_name() {
        let mut call = ToolCall::new("plan:turn-1", "update_plan");
        call.panel = Some(Panel::Tasks(TaskList {
            items: vec![Task {
                id: None,
                text: "Inspect".into(),
                status: TaskStatus::InProgress,
            }],
            explanation: "Found a migration".into(),
        }));
        let mut renamed = call.clone();
        renamed.name = "plan".into();
        for call in [call, renamed] {
            let row = Row {
                role: "tool".into(),
                text: serde_json::to_string(&call).unwrap(),
                first: true,
                sent_at: None,
            };
            let plan = snapshot(&[prompt("go"), row], false).plan.unwrap();
            assert_eq!(plan.list.explanation, "Found a migration");
            assert_eq!(plan.current().unwrap().text, "Inspect");
        }
    }

    #[test]
    fn claude_task_tools_fold_creates_updates_and_deletes() {
        let rows = [
            prompt("go"),
            tool(
                "TaskCreate",
                json!({"subject": "Read"}),
                "Task #1 created successfully: Read",
            ),
            tool(
                "TaskCreate",
                json!({"subject": "Edit"}),
                "Task #2 created successfully: Edit",
            ),
            tool(
                "TaskCreate",
                json!({"subject": "Drop me"}),
                "Task #3 created successfully",
            ),
            tool(
                "TaskUpdate",
                json!({"taskId": 1, "status": "completed"}),
                "",
            ),
            tool(
                "TaskUpdate",
                json!({"taskId": "#2", "status": "in_progress", "subject": "Edit store"}),
                "",
            ),
            tool(
                "TaskUpdate",
                json!({"taskId": "3", "status": "deleted"}),
                "",
            ),
            read("store.rs"),
        ];
        let plan = snapshot(&rows, false).plan.unwrap();
        let texts: Vec<_> = plan.list.items.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["Read", "Edit store"]);
        assert_eq!(
            statuses(&plan),
            [TaskStatus::Completed, TaskStatus::InProgress]
        );
        assert_eq!(plan.activity, 1);
        assert!(!plan.stale);
    }

    #[test]
    fn cancelled_steps_do_not_count_towards_completion() {
        let rows = [
            prompt("go"),
            todos(&[("A", "completed"), ("B", "cancelled"), ("C", "completed")]),
        ];
        let plan = snapshot(&rows, false).plan.unwrap();
        assert_eq!((plan.done(), plan.actionable()), (2, 2));
        assert!(plan.complete());
    }

    #[test]
    fn milestones_are_not_activity() {
        let rows = [
            prompt("go"),
            todos(&[("A", "in_progress")]),
            tool("contextCompaction", json!({}), ""),
        ];
        assert_eq!(snapshot(&rows, false).plan.unwrap().activity, 0);
    }

    fn answer(text: &str) -> Row {
        Row {
            role: "assistant".into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }
    fn exit_plan(state: ToolState) -> Row {
        let mut call = ToolCall::new("exit", "ExitPlanMode");
        call.input = json!({"plan": "# Plan\n\n1. Add the table\n2. Backfill"}).to_string();
        call.state = state;
        Row {
            role: "tool".into(),
            text: serde_json::to_string(&call).unwrap(),
            first: true,
            sent_at: None,
        }
    }

    #[test]
    fn claude_exit_plan_mode_is_ready_until_allowed_work_follows() {
        let mut rows = vec![
            prompt("plan it"),
            read("a.rs"),
            exit_plan(ToolState::Running),
        ];
        let proposal = snapshot(&rows, true).proposal.unwrap();
        assert!(proposal.text.starts_with("# Plan"));
        assert!(!proposal.building);
        rows[2] = exit_plan(ToolState::Completed);
        rows.push(read("b.rs"));
        assert!(snapshot(&rows, false).proposal.unwrap().building);
        // Its todo list then takes over.
        rows.push(todos(&[
            ("Add the table", "in_progress"),
            ("Backfill", "pending"),
        ]));
        let snap = snapshot(&rows, false);
        assert_eq!(snap.proposal, None);
        assert!(snap.plan.is_some());
    }

    #[test]
    fn codex_proposed_plan_tags_survive_storage_chunks() {
        let mut first = answer("Here is the plan.\n<proposed_");
        first
            .text
            .push_str("plan>\n# Store split\n- Add the table\n");
        let rest = Row {
            first: false,
            ..answer("- Backfill\n</proposed_plan>")
        };
        let rows = [prompt("plan it"), first, rest];
        let proposal = snapshot(&rows, false).proposal.unwrap();
        assert_eq!(proposal.text, "# Store split\n- Add the table\n- Backfill");
        assert!(!proposal.building);
    }

    #[test]
    fn a_plan_mode_answer_is_the_proposal_and_the_next_prompt_builds_it() {
        let mut rows = vec![prompt("plan it"), read("a.rs"), answer("1. Add\n2. Test")];
        assert_eq!(snapshot(&rows, false).proposal, None);
        assert_eq!(
            snapshot(&rows, true).proposal.unwrap().text,
            "1. Add\n2. Test"
        );
        rows[2] = answer("<proposed_plan>1. Add\n2. Test</proposed_plan>");
        rows.push(prompt("Build the approved plan."));
        rows.push(read("b.rs"));
        let snap = snapshot(&rows, false);
        assert!(snap.proposal.unwrap().building);
        assert_eq!(snap.turn_calls, 1);
    }

    #[test]
    fn a_proposal_is_titled_by_its_first_heading_or_line() {
        let title = |text: &str| {
            Proposal {
                text: text.into(),
                building: false,
            }
            .title()
        };
        assert_eq!(
            title("Intro line\n\n# Settings search\n\n1. Index"),
            "Settings search"
        );
        assert_eq!(
            title("\n1. **Retry** failed uploads"),
            "Retry failed uploads"
        );
        assert_eq!(title(""), "Plan");
    }
}
