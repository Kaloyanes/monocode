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
    /// The newest tool call of the running turn: what the agent is doing now,
    /// known whether or not it wrote a plan.
    pub now: Option<Now>,
}

/// A tool call as the rail's readout names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Now {
    pub action: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub list: TaskList,
    /// Tool calls the agent made after it last touched its list.
    pub activity: usize,
    /// A prompt arrived after the list was written: it describes an earlier turn.
    pub stale: bool,
    /// The agent wrote no list: the steps are the approved proposal's own and
    /// the current one is a guess from what its tool calls touched.
    pub inferred: bool,
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
    // Rows of tool calls newer than the proposal, newest first.
    let mut calls: Vec<usize> = Vec::new();
    let mut now_row: Option<usize> = None;
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
                    now_row.get_or_insert(at);
                }
                if !touched {
                    activity += 1;
                    if proposal.is_none() {
                        calls.push(at);
                    }
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
    let now = now_row.and_then(|at| {
        let call = serde_json::from_str::<ToolCall>(&rows[at].text).ok()?;
        let shown = call.presentation();
        Some(Now {
            action: shown.action,
            target: shown.target,
        })
    });
    let (mut list, stale) = match base {
        Some((list, prompts)) => (
            list,
            prompts > 0 && (increments.is_empty() || increments_before_prompt(rows)),
        ),
        None if increments.is_empty() => {
            let plan = proposal
                .as_ref()
                .filter(|p| p.building)
                .and_then(|p| estimate(rows, &p.text, &calls));
            return Snapshot {
                plan,
                proposal,
                turn_calls,
                now,
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
            now,
        };
    }
    Snapshot {
        plan: Some(Plan {
            list,
            activity,
            stale,
            inferred: false,
        }),
        proposal,
        turn_calls,
        now,
    }
}

/// The approved proposal's own steps with a guess at where the agent is. The
/// agent wrote no list, so nothing here says a step is done: the furthest step
/// its tool calls mention is current, earlier ones are taken as behind it, and
/// the last step is never marked complete.
fn estimate(rows: &[Row], text: &str, calls: &[usize]) -> Option<Plan> {
    let steps = proposal_steps(text);
    if steps.len() < 2 {
        return None;
    }
    let keys: Vec<StepKeys> = steps.iter().map(|(_, raw)| StepKeys::new(raw)).collect();
    let mut current = 0;
    let mut reached = 0;
    let mut seen = 0;
    for &at in calls.iter().rev() {
        let Ok(call) = serde_json::from_str::<ToolCall>(&rows[at].text) else {
            continue;
        };
        let touched = touched_text(&call);
        if let Some(hit) = (current..keys.len())
            .rev()
            .find(|&ix| keys[ix].hit(&touched))
        {
            if hit > current {
                current = hit;
                reached = seen;
            }
        }
        seen += 1;
    }
    let items = steps
        .into_iter()
        .enumerate()
        .map(|(ix, (text, _))| Task {
            id: None,
            text,
            status: match ix.cmp(&current) {
                std::cmp::Ordering::Less => TaskStatus::Completed,
                std::cmp::Ordering::Equal => TaskStatus::InProgress,
                std::cmp::Ordering::Greater => TaskStatus::Pending,
            },
        })
        .collect();
    Some(Plan {
        list: TaskList {
            items,
            explanation: String::new(),
        },
        activity: seen - reached,
        stale: false,
        inferred: true,
    })
}

const MAX_ESTIMATED_STEPS: usize = 12;

/// What a call touched, lowercase: its target, file and command, bounded so a
/// huge write costs nothing.
fn touched_text(call: &ToolCall) -> String {
    let shown = call.presentation();
    let mut text = [
        Some(shown.target),
        shown.file_path,
        shown.command,
        shown.description,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    text.truncate(text.floor_char_boundary(1_500));
    text
}

/// The words of a step that say where it happens.
struct StepKeys {
    /// Backticked names and file-like words: one mention is enough.
    strong: Vec<String>,
    /// Plain words: two mentions are needed.
    weak: Vec<String>,
    /// Commands a step like "run the tests" or "build" shows up as.
    commands: Vec<&'static str>,
}

impl StepKeys {
    fn new(raw: &str) -> Self {
        const STOP: [&str; 13] = [
            "update",
            "create",
            "implement",
            "ensure",
            "should",
            "before",
            "after",
            "existing",
            "through",
            "using",
            "change",
            "support",
            "handle",
        ];
        let mut strong = Vec::new();
        for (ix, span) in raw.split('`').enumerate() {
            if ix % 2 == 1 && !span.trim().is_empty() {
                strong.push(span.trim().to_lowercase());
            }
        }
        let plain = raw
            .split('`')
            .step_by(2)
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let mut weak = Vec::new();
        for word in plain.split(|c: char| c.is_whitespace() || ",;:()\"'".contains(c)) {
            let word = word.trim_matches(|c: char| c == '.' || c == '*');
            let pathlike =
                word.len() >= 4 && (word.contains('/') || word.contains('.') || word.contains('_'));
            if pathlike {
                strong.push(word.to_owned());
            } else if word.len() >= 6
                && word.chars().all(char::is_alphabetic)
                && !STOP.contains(&word)
            {
                weak.push(word.to_owned());
            }
        }
        // A path matches by its file name too: calls show paths shortened.
        let names: Vec<String> = strong
            .iter()
            .filter_map(|s| s.rsplit('/').next().filter(|n| n.len() >= 3 && *n != s))
            .map(str::to_owned)
            .collect();
        strong.extend(names);
        let mut commands = Vec::new();
        let words: Vec<&str> = plain.split_whitespace().collect();
        let has = |w: &[&str]| {
            words
                .iter()
                .any(|x| w.contains(&x.trim_matches(|c: char| !c.is_alphabetic())))
        };
        if has(&["test", "tests", "verify", "validate"]) {
            commands.push("test");
        }
        if has(&["build", "compile", "check"]) {
            commands.extend(["build", "cargo check", "compile"]);
        }
        if has(&["commit"]) {
            commands.push("git commit");
        }
        Self {
            strong,
            weak,
            commands,
        }
    }

    fn hit(&self, touched: &str) -> bool {
        self.strong.iter().any(|s| touched.contains(s.as_str()))
            || self.commands.iter().any(|c| touched.contains(c))
            || self
                .weak
                .iter()
                .filter(|w| touched.contains(w.as_str()))
                .count()
                >= 2
    }
}

/// The steps of a Markdown plan: its first numbered list, else its first
/// bullet list of two or more, top level only. Each is (shown, source).
fn proposal_steps(text: &str) -> Vec<(String, String)> {
    struct Item {
        indent: usize,
        numbered: bool,
        text: String,
    }
    // Lists split at headings, so "Files" bullets never join "Steps".
    let mut groups: Vec<Vec<Item>> = vec![Vec::new()];
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if trimmed.starts_with('#') {
            groups.push(Vec::new());
            continue;
        }
        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        let (numbered, rest) = if digits > 0
            && matches!(trimmed[digits..].chars().next(), Some('.' | ')'))
            && trimmed[digits + 1..].starts_with(' ')
        {
            (true, &trimmed[digits + 2..])
        } else if let Some(rest) = ["- ", "* ", "+ "]
            .iter()
            .find_map(|m| trimmed.strip_prefix(m))
        {
            (false, rest)
        } else {
            continue;
        };
        let rest = rest
            .trim_start_matches("[ ] ")
            .trim_start_matches("[x] ")
            .trim();
        if !rest.is_empty() {
            groups.last_mut().unwrap().push(Item {
                indent,
                numbered,
                text: rest.to_owned(),
            });
        }
    }
    // Top-level items only; `numbered` keeps the list's own numbered lines.
    let top = |group: &Vec<Item>, numbered: bool| -> Vec<(String, String)> {
        let Some(min) = group.iter().map(|i| i.indent).min() else {
            return Vec::new();
        };
        group
            .iter()
            .filter(|i| i.indent == min && (i.numbered || !numbered))
            .take(MAX_ESTIMATED_STEPS)
            .map(|i| {
                let shown: String = i
                    .text
                    .chars()
                    .filter(|c| !matches!(c, '*' | '`'))
                    .take(100)
                    .collect();
                (shown.trim().to_owned(), i.text.clone())
            })
            .collect()
    };
    let numbered = groups.iter().map(|g| top(g, true)).find(|s| !s.is_empty());
    numbered.unwrap_or_else(|| {
        groups
            .iter()
            .map(|g| top(g, false))
            .find(|steps| steps.len() >= 2)
            .unwrap_or_default()
    })
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
    if super::tasks::is_todo_tool(name) || text.contains("\"panel\":{\"kind\":\"tasks\"") {
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

    fn proposal_row(steps: &str) -> Row {
        Row {
            role: "assistant".into(),
            text: format!("{PROPOSED_OPEN}\n# Plan\n\nFiles:\n- the rail\n- the shell\n\n## Steps\n{steps}\n{PROPOSED_CLOSE}"),
            first: true,
            sent_at: None,
        }
    }
    const STEPS: &str =
        "1. Add the `plan_rail.rs` tooltip\n2. Wire `shell.rs` to open it\n3. Run the tests";
    fn building(calls: Vec<Row>) -> Snapshot {
        let mut rows = vec![prompt("plan it"), proposal_row(STEPS), prompt("Build it")];
        rows.extend(calls);
        snapshot(&rows, false)
    }
    fn edit(path: &str) -> Row {
        tool(
            "Edit",
            json!({"file_path": path, "old_string": "a", "new_string": "b"}),
            "",
        )
    }

    #[test]
    fn an_approved_proposal_gives_estimated_steps_and_the_furthest_touched_is_current() {
        let plan = building(vec![read("/w/crates/plan_rail.rs")]).plan.unwrap();
        assert!(plan.inferred && !plan.stale);
        assert_eq!(plan.list.items.len(), 3);
        assert_eq!(plan.list.items[0].text, "Add the plan_rail.rs tooltip");
        assert_eq!(
            statuses(&plan),
            [
                TaskStatus::InProgress,
                TaskStatus::Pending,
                TaskStatus::Pending
            ]
        );
        let plan = building(vec![
            read("/w/crates/plan_rail.rs"),
            edit("/w/crates/shell.rs"),
        ])
        .plan
        .unwrap();
        assert_eq!(
            statuses(&plan),
            [
                TaskStatus::Completed,
                TaskStatus::InProgress,
                TaskStatus::Pending
            ]
        );
        assert_eq!(plan.activity, 1);
        let ran = tool(
            "Bash",
            json!({"command": "cargo test -p monocode-core"}),
            "ok",
        );
        let plan = building(vec![
            edit("/w/crates/shell.rs"),
            ran,
            read("/w/crates/shell.rs"),
        ])
        .plan
        .unwrap();
        assert_eq!(plan.current().unwrap().text, "Run the tests");
        assert!(!plan.complete(), "a guess never claims the plan is done");
    }

    #[test]
    fn an_estimate_with_no_match_starts_at_the_first_step_and_never_runs_ahead() {
        let plan = building(vec![read("/w/README.md")]).plan.unwrap();
        assert_eq!(plan.current().unwrap().text, "Add the plan_rail.rs tooltip");
        assert_eq!(plan.activity, 1);
    }

    #[test]
    fn the_agents_own_list_beats_the_estimate() {
        let plan = building(vec![
            todos(&[("Only", "in_progress"), ("Two", "pending")]),
            edit("/w/crates/shell.rs"),
        ])
        .plan
        .unwrap();
        assert!(!plan.inferred);
        assert_eq!(plan.current().unwrap().text, "Only");
    }

    #[test]
    fn a_proposal_still_awaiting_review_has_no_estimate() {
        let rows = [prompt("plan it"), proposal_row(STEPS)];
        assert_eq!(snapshot(&rows, false).plan, None);
    }

    #[test]
    fn steps_come_from_the_numbered_list_else_the_first_bullet_list() {
        let steps = |text: &str| -> Vec<String> {
            proposal_steps(text)
                .into_iter()
                .map(|(shown, _)| shown)
                .collect()
        };
        assert_eq!(
            steps("- a\n- b\n\n## Steps\n1. **First** thing\n   - nested\n2. Second"),
            ["First thing", "Second"]
        );
        assert_eq!(steps("Intro\n- one\n- two\n"), ["one", "two"]);
        assert!(steps("Just prose, and one\n- lonely bullet").is_empty());
        assert!(steps("```\n1. not a step\n2. code\n```").is_empty());
    }

    #[test]
    fn the_newest_call_is_what_the_agent_is_doing_now() {
        let rows = [prompt("go"), read("/w/a.rs"), edit("/w/b.rs")];
        let now = snapshot(&rows, false).now.unwrap();
        assert!(now.target.contains("b.rs"), "{now:?}");
        assert_eq!(snapshot(&[prompt("go")], false).now, None);
        // An earlier turn's last call is not now.
        let rows = [prompt("one"), read("/w/a.rs"), prompt("two")];
        assert_eq!(snapshot(&rows, false).now, None);
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
    fn monocodes_own_plan_tool_is_a_list_under_any_provider_prefix() {
        let input = r#"{"plan":[{"step":"Read","status":"completed"},{"step":"Edit","status":"in_progress"}]}"#;
        for name in ["mcp__monocode__update_plan", "monocode:update_plan"] {
            let mut call = ToolCall::new("plan-1", name);
            call.input = input.into();
            let row = Row {
                role: "tool".into(),
                text: serde_json::to_string(&call).unwrap(),
                first: true,
                sent_at: None,
            };
            let plan = snapshot(&[prompt("go"), row], false).plan.unwrap();
            assert_eq!(plan.current().unwrap().text, "Edit", "{name}");
            assert_eq!((plan.done(), plan.actionable()), (1, 2), "{name}");
        }
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
