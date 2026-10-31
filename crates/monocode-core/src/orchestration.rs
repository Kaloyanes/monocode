//! Reviewable orchestration plans and the runs the app drives from them.
//! Workers share a checkout; overlapping scopes are serialized even when they
//! have no explicit dependency. The model only proposes the plan: dispatch,
//! dependencies, reports and the hand-back are all decided here.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Component, Path},
};

/// The MonoCode MCP tool a planning turn submits its plan with.
pub const TOOL: &str = "orchestration_plan";
/// Set on a planning turn's provider environment; the MCP server lists
/// [`TOOL`] only then.
pub const ENV: &str = "MONOCODE_ORCHESTRATOR";
/// Workers that may run at once.
pub const MAX_WORKERS: usize = 4;
const OPEN: &str = "<monocode_orchestration>";
const CLOSE: &str = "</monocode_orchestration>";
/// One worker's report, as kept on its assignment.
const REPORT_CHARS: usize = 12_000;
/// The whole hand-back to the lead.
const HAND_BACK_CHARS: usize = 48_000;

/// orchestrationSummary.ts: what the sidebar shows of a lead's run, kept after
/// it ends so the lead still lists its agents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Summary {
    pub tasks: Vec<SummaryTask>,
    /// The run is still dispatching or waiting on its workers.
    pub live: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SummaryTask {
    pub session: Option<i64>,
    pub title: String,
    pub provider: String,
    pub model: Option<String>,
    /// queued, running, completed, failed, blocked, stopped or interrupted.
    pub status: String,
}
impl SummaryTask {
    /// orchestrationTaskLabel, given whether the worker needs input.
    pub fn label(&self, summary: &Summary, needs_input: bool) -> &'static str {
        if needs_input {
            return "Needs input";
        }
        if !summary.live && matches!(self.status.as_str(), "running" | "queued") {
            return "Saved";
        }
        match self.status.as_str() {
            "queued" => "Queued",
            "running" => "Working",
            "completed" => "Done",
            "saved" => "Saved",
            "failed" => "Failed",
            "blocked" => "Needs review",
            "stopped" => "Stopped",
            "interrupted" => "Interrupted",
            _ => "Queued",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub prompt: String,
    /// Project-relative write scopes. Omitted means the whole project.
    #[serde(default = "whole_project")]
    pub files: Vec<String>,
    #[serde(default, alias = "dependsOn")]
    pub depends_on: Vec<String>,
    /// Another harness for this assignment; the lead's when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Another model for this assignment; the lead's when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
fn whole_project() -> Vec<String> {
    vec![".".into()]
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub tasks: Vec<Task>,
}
impl Plan {
    /// Drops an assignment from a plan under review, along with every
    /// dependency on it.
    pub fn remove(&mut self, id: &str) {
        self.tasks.retain(|task| task.id != id);
        for task in &mut self.tasks {
            task.depends_on.retain(|dependency| dependency != id);
        }
    }
}

pub const INSTRUCTIONS: &str = r#"Prepare an orchestration plan for the user's request. Inspect the project with read-only tools. Do not edit files or launch workers. The user reviews the plan and starts it in MonoCode.
Submit the plan by calling the orchestration_plan tool from the monocode MCP server. If it returns an error, fix the plan and call it again. After it succeeds, reply with a short explanation of the approach.
Only if that tool is unavailable, end your reply with exactly one JSON object between <monocode_orchestration> and </monocode_orchestration>, using the tool's schema: {"title":"short title","summary":"approach","tasks":[{"id":"task-1","title":"assignment title","prompt":"complete self-contained instructions, validation and expected report","files":["src/path"],"depends_on":[]}]}
Use 1 to 12 assignments with unique ASCII IDs, exact project-relative file or directory scopes ('.' for the whole project) and dependency IDs. Independent assignments run concurrently (up to four); overlapping scopes run one after another. Workers use this session's provider, model and access policy unless an assignment sets provider or model, which you should do only when the user asks for it. Each worker sees only its own prompt and the reports of the assignments it depends on, so make every prompt self-contained and include how to verify the work.
If the request is too unclear to plan, ask one short question instead of submitting a plan; the user's answer continues the planning. Never claim that workers have started."#;

/// The MCP definition of [`TOOL`].
pub fn tool_definition() -> Value {
    json!({
        "name": TOOL,
        "description": "Submit the orchestration plan for the user to review and start in MonoCode. Send the complete plan; a later call replaces it. Returns the problems to fix when the plan is invalid. Does not start any worker.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "maxLength": 160},
                "summary": {"type": "string", "maxLength": 4000, "description": "The approach, in a few sentences"},
                "tasks": {
                    "type": "array", "minItems": 1, "maxItems": 12,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "pattern": "^[A-Za-z0-9_-]{1,64}$"},
                            "title": {"type": "string", "maxLength": 160},
                            "prompt": {"type": "string", "maxLength": 30000, "description": "Complete, self-contained instructions, how to verify, and what to report"},
                            "files": {"type": "array", "minItems": 1, "maxItems": 64, "items": {"type": "string"}, "description": "Project-relative write scopes; '.' is the whole project"},
                            "depends_on": {"type": "array", "maxItems": 12, "items": {"type": "string"}},
                            "provider": {"type": "string", "description": "Only when the user asked for another harness"},
                            "model": {"type": "string", "description": "Only when the user asked for another model"}
                        },
                        "required": ["id", "title", "prompt", "files"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["title", "tasks"],
            "additionalProperties": false
        },
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false}
    })
}

/// A plan from [`TOOL`]'s input.
pub fn from_input(input: &Value) -> Result<Plan, String> {
    let mut plan: Plan =
        serde_json::from_value(input.clone()).map_err(|e| format!("Invalid task plan: {e}"))?;
    validate(&plan)?;
    normalize(&mut plan);
    Ok(plan)
}
/// The tagged fallback for providers without the MonoCode MCP server.
pub fn parse(text: &str) -> Result<Plan, String> {
    let body = text
        .rfind(OPEN)
        .map(|start| &text[start + OPEN.len()..])
        .and_then(|rest| rest.split_once(CLOSE).map(|(body, _)| body))
        .ok_or("The response did not include a task plan.")?;
    if body.len() > 256 * 1024 {
        return Err("Task plan is too large.".into());
    }
    let mut plan: Plan = serde_json::from_str(strip_fence(body.trim()))
        .map_err(|e| format!("Invalid task plan: {e}"))?;
    validate(&plan)?;
    normalize(&mut plan);
    Ok(plan)
}
fn strip_fence(body: &str) -> &str {
    let Some(rest) = body.strip_prefix("```") else {
        return body;
    };
    let rest = rest.split_once('\n').map_or("", |(_, rest)| rest);
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}
pub fn validate(plan: &Plan) -> Result<(), String> {
    if plan.title.trim().is_empty()
        || plan.title.len() > 160
        || plan.summary.len() > 4_000
        || plan.tasks.is_empty()
        || plan.tasks.len() > 12
    {
        return Err("Provide a title and 1–12 assignments.".into());
    }
    let mut ids = HashSet::new();
    for task in &plan.tasks {
        if task.id.is_empty()
            || task.id.len() > 64
            || !task
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(task.id.as_str())
        {
            return Err(
                "Task IDs must be unique and contain only letters, numbers, '-' and '_'.".into(),
            );
        }
        if task.title.trim().is_empty()
            || task.title.len() > 160
            || task.prompt.trim().is_empty()
            || task.prompt.len() > 30_000
            || task.files.is_empty()
            || task.files.len() > 64
            || task.depends_on.len() > 12
        {
            return Err(format!(
                "Assignment {} needs a title, instructions and file scopes.",
                task.id
            ));
        }
        if let Some(provider) = &task.provider {
            if crate::provider::Kind::parse(provider).is_none() {
                return Err(format!(
                    "Assignment {} names an unknown provider: {provider}",
                    task.id
                ));
            }
        }
        for file in &task.files {
            if file.trim().is_empty()
                || file.len() > 512
                || file.contains('\\')
                || file.contains(':')
                || Path::new(file).is_absolute()
                || Path::new(file).components().any(|c| {
                    matches!(
                        c,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err(format!("Invalid file scope: {file}"));
            }
        }
        if task.depends_on.iter().any(|id| *id == task.id) {
            return Err(format!("Assignment {} depends on itself.", task.id));
        }
    }
    let mut completed = HashSet::new();
    loop {
        let before = completed.len();
        for task in &plan.tasks {
            if task
                .depends_on
                .iter()
                .all(|id| completed.contains(id.as_str()))
            {
                completed.insert(task.id.as_str());
            }
        }
        if completed.len() == plan.tasks.len() {
            return Ok(());
        }
        if completed.len() == before {
            return Err("Task dependencies contain a cycle or unknown task ID.".into());
        }
    }
}
/// `./src/`, `src` and `src//` are one scope; an empty one is the project.
pub fn scope(path: &str) -> String {
    let parts: Vec<&str> = path
        .trim()
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}
fn normalize(plan: &mut Plan) {
    for task in &mut plan.tasks {
        let mut seen = HashSet::new();
        task.files = task
            .files
            .iter()
            .map(|file| scope(file))
            .filter(|file| seen.insert(file.clone()))
            .collect();
        let mut seen = HashSet::new();
        task.depends_on.retain(|id| seen.insert(id.clone()));
    }
}
/// Case-insensitive, because macOS checkouts usually are.
pub fn overlaps(a: &[String], b: &[String]) -> bool {
    fn within(path: &str, parent: &str) -> bool {
        path == parent
            || path
                .strip_prefix(parent)
                .is_some_and(|rest| rest.starts_with('/'))
    }
    a.iter().any(|a| {
        let a = scope(a).to_lowercase();
        b.iter().any(|b| {
            let b = scope(b).to_lowercase();
            a == "." || b == "." || within(&a, &b) || within(&b, &a)
        })
    })
}
/// Paths a worker edited outside its scopes. Paths are absolute or relative to
/// the checkout.
pub fn outside_scope(task: &Task, cwd: &str, edited: &[String]) -> Vec<String> {
    let root = Path::new(cwd);
    let mut outside: Vec<String> = edited
        .iter()
        .filter_map(|path| {
            let relative = Path::new(path)
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.clone());
            if Path::new(&relative).is_absolute() {
                // Outside the checkout altogether.
                return Some(relative);
            }
            let path = scope(&relative).to_lowercase();
            let inside = task.files.iter().any(|file| {
                let file = scope(file).to_lowercase();
                file == "." || path == file || path.starts_with(&format!("{file}/"))
            });
            (!inside).then_some(relative)
        })
        .collect();
    outside.sort();
    outside.dedup();
    outside
}
pub fn worker_prompt(task: &Task, dependency_reports: &str) -> String {
    format!(
        "You are completing assignment {} in a MonoCode orchestration: {}\n\n{}\n\nWrite only within these project-relative scopes: {}. Other agents may be working in this checkout at the same time: preserve their changes and never reset, stash or check out files. If you need a change outside your scopes, stop and report it as a blocker. Run focused checks. End with a short report of what changed, how you verified it and anything left to do.\n\nReports from the assignments this one depends on:\n{}",
        task.id,
        task.title,
        task.prompt,
        task.files.join(", "),
        if dependency_reports.is_empty() {
            "None."
        } else {
            dependency_reports
        }
    )
}
/// Sent to a worker whose turn was interrupted or stopped.
pub const CONTINUE_PROMPT: &str = "Continue your assignment from where you left off. If it is already complete, check it and give your final report.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Pending,
    /// Its worker session is being created.
    Starting,
    Running,
    Done,
    Failed,
    /// The user stopped it, or the run, before it finished.
    Stopped,
    /// An assignment it depends on did not complete.
    Blocked,
    /// The app quit while it ran. It waits for the run to resume.
    Interrupted,
}
impl State {
    /// Nothing more happens to it unless the user retries.
    pub fn settled(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Stopped | Self::Blocked
        )
    }
    fn unfinished(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped | Self::Blocked)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Queued",
            Self::Starting => "Starting",
            Self::Running => "Working",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Blocked => "Blocked",
            Self::Interrupted => "Interrupted",
        }
    }
    fn summary_status(self) -> &'static str {
        match self {
            Self::Pending | Self::Starting => "queued",
            Self::Running => "running",
            Self::Done => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Blocked => "blocked",
            Self::Interrupted => "interrupted",
        }
    }
}
/// How a worker's turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Failed,
    Stopped,
    /// The worker never received its prompt.
    NotStarted,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assignment {
    pub session: Option<i64>,
    pub state: State,
    pub report: String,
    /// Edits the worker made outside its scopes.
    pub outside_scope: Vec<String>,
    /// Its worker received the assignment, so a retry continues instead of
    /// starting over.
    pub launched: bool,
}

/// One started plan. Saved on every change, so a restart can resume it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub plan: Plan,
    /// The transcript tool call that submitted the plan, when there was one.
    #[serde(default)]
    pub call: Option<String>,
    pub cwd: String,
    pub provider: String,
    pub settings: crate::agent::Settings,
    pub tasks: Vec<Assignment>,
    #[serde(default)]
    pub stopped: bool,
    /// Restored after a restart and waiting for the user to resume.
    #[serde(default)]
    pub paused: bool,
    /// The final report reached the lead.
    #[serde(default)]
    pub reported: bool,
}
impl Run {
    pub fn new(
        plan: Plan,
        call: Option<String>,
        cwd: String,
        provider: String,
        settings: crate::agent::Settings,
    ) -> Self {
        let tasks = vec![Assignment::default(); plan.tasks.len()];
        Self {
            plan,
            call,
            cwd,
            provider,
            settings,
            tasks,
            stopped: false,
            paused: false,
            reported: false,
        }
    }
    pub fn finished(&self) -> bool {
        self.tasks.iter().all(|task| task.state.settled())
    }
    /// Still holds its checkout: workers may be editing it.
    pub fn active(&self) -> bool {
        !self.finished()
    }
    /// Waits for the final report to reach the lead.
    pub fn awaiting_report(&self) -> bool {
        self.finished() && !self.reported && !self.stopped
    }
    fn ix(&self, id: &str) -> Option<usize> {
        self.plan.tasks.iter().position(|task| task.id == id)
    }
    /// Blocks the dependants of unfinished assignments and unblocks those whose
    /// dependencies were retried, then marks and returns the assignments that
    /// may start now.
    pub fn dispatch(&mut self) -> Vec<usize> {
        let failed = |run: &Self, ix: usize| {
            run.plan.tasks[ix]
                .depends_on
                .iter()
                .filter_map(|id| run.ix(id))
                .any(|dependency| run.tasks[dependency].state.unfinished())
        };
        loop {
            let unblock: Vec<usize> = (0..self.tasks.len())
                .filter(|&ix| self.tasks[ix].state == State::Blocked && !failed(self, ix))
                .collect();
            if unblock.is_empty() {
                break;
            }
            for ix in unblock {
                self.tasks[ix].state = State::Pending;
                self.tasks[ix].report.clear();
            }
        }
        loop {
            let block: Vec<usize> = (0..self.tasks.len())
                .filter(|&ix| self.tasks[ix].state == State::Pending && failed(self, ix))
                .collect();
            if block.is_empty() {
                break;
            }
            for ix in block {
                self.tasks[ix].state = State::Blocked;
            }
        }
        if self.stopped || self.paused {
            return Vec::new();
        }
        let mut occupied: Vec<Vec<String>> = self
            .tasks
            .iter()
            .zip(&self.plan.tasks)
            .filter(|(state, _)| matches!(state.state, State::Starting | State::Running))
            .map(|(_, task)| task.files.clone())
            .collect();
        let mut started = Vec::new();
        for ix in 0..self.tasks.len() {
            if occupied.len() >= MAX_WORKERS {
                break;
            }
            let task = &self.plan.tasks[ix];
            let ready = self.tasks[ix].state == State::Pending
                && task.depends_on.iter().all(|id| {
                    self.ix(id)
                        .is_some_and(|dependency| self.tasks[dependency].state == State::Done)
                })
                && !occupied.iter().any(|files| overlaps(files, &task.files));
            if ready {
                occupied.push(task.files.clone());
                self.tasks[ix].state = State::Starting;
                started.push(ix);
            }
        }
        started
    }
    /// Why a queued assignment has not started.
    pub fn waiting_on(&self, ix: usize) -> Option<String> {
        if self.tasks[ix].state != State::Pending {
            return None;
        }
        let waiting: Vec<&str> = self.plan.tasks[ix]
            .depends_on
            .iter()
            .filter(|id| {
                self.ix(id)
                    .is_some_and(|dependency| self.tasks[dependency].state != State::Done)
            })
            .map(String::as_str)
            .collect();
        if !waiting.is_empty() {
            return Some(format!("after {}", waiting.join(", ")));
        }
        if self.paused || self.stopped {
            return None;
        }
        let busy = self
            .tasks
            .iter()
            .zip(&self.plan.tasks)
            .find(|(state, task)| {
                matches!(state.state, State::Starting | State::Running)
                    && overlaps(&task.files, &self.plan.tasks[ix].files)
            });
        Some(match busy {
            Some((_, task)) => format!("scope busy with {}", task.id),
            None => "waiting for a free worker".into(),
        })
    }
    pub fn finish(&mut self, ix: usize, outcome: Outcome, report: &str) {
        let task = &mut self.tasks[ix];
        task.launched |= outcome != Outcome::NotStarted;
        task.state = match outcome {
            Outcome::Completed => State::Done,
            Outcome::Failed | Outcome::NotStarted => State::Failed,
            Outcome::Stopped => State::Stopped,
        };
        task.report = crate::response::limit_middle(
            &if outcome == Outcome::NotStarted {
                format!("The worker did not start. {report}")
            } else {
                report.to_owned()
            },
            REPORT_CHARS,
        );
    }
    /// Stops dispatching. Queued assignments are stopped; the caller stops the
    /// running workers.
    pub fn stop(&mut self) {
        self.stopped = true;
        for task in &mut self.tasks {
            if matches!(task.state, State::Pending | State::Starting) {
                task.state = State::Stopped;
            }
        }
    }
    /// Returns to dispatching after a restart or a stop. The returned workers
    /// continue in their sessions; the others are queued again.
    pub fn resume(&mut self) -> Vec<usize> {
        self.paused = false;
        self.stopped = false;
        self.reported = false;
        let mut continued = Vec::new();
        for ix in 0..self.tasks.len() {
            if matches!(self.tasks[ix].state, State::Interrupted | State::Stopped) && self.retry(ix)
            {
                continued.push(ix);
            }
        }
        continued
    }
    /// Gives an unfinished assignment another go. True when it goes back to
    /// its existing worker session ([`Assignment::launched`] says whether that
    /// worker continues or receives the assignment again); otherwise it is
    /// queued for a new worker.
    pub fn retry(&mut self, ix: usize) -> bool {
        let task = &mut self.tasks[ix];
        if !matches!(
            task.state,
            State::Failed | State::Stopped | State::Interrupted
        ) {
            return false;
        }
        self.stopped = false;
        self.reported = false;
        task.outside_scope.clear();
        let continues = task.session.is_some();
        task.state = if continues {
            State::Running
        } else {
            task.session = None;
            State::Pending
        };
        task.report.clear();
        continues
    }
    /// After a restart: nothing was saved mid-flight, so running workers are
    /// interrupted and the run waits to be resumed.
    pub fn restored(mut self) -> Self {
        for task in &mut self.tasks {
            match task.state {
                State::Starting => task.state = State::Pending,
                State::Running => task.state = State::Interrupted,
                _ => {}
            }
        }
        self.paused = !self.finished();
        self
    }
    pub fn dependency_reports(&self, ix: usize) -> String {
        self.plan.tasks[ix]
            .depends_on
            .iter()
            .filter_map(|id| self.ix(id))
            .map(|dependency| {
                format!(
                    "{} ({}):\n{}",
                    self.plan.tasks[dependency].title,
                    self.plan.tasks[dependency].id,
                    self.tasks[dependency].report
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    /// The lead's hand-back: every assignment's outcome and report.
    pub fn report(&self) -> String {
        let share = (HAND_BACK_CHARS / self.tasks.len().max(1)).max(2_000);
        let body = self
            .plan
            .tasks
            .iter()
            .zip(&self.tasks)
            .map(|(task, state)| {
                let outcome = match state.state {
                    State::Done => "completed",
                    State::Failed => "failed",
                    State::Stopped => "stopped before finishing",
                    State::Blocked => "not started: a dependency did not complete",
                    _ => "incomplete",
                };
                let outside = if state.outside_scope.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nEdited outside its scopes: {}",
                        state.outside_scope.join(", ")
                    )
                };
                format!(
                    "## {} ({}) [{outcome}]{outside}\n{}",
                    task.title,
                    task.id,
                    crate::response::limit_middle(state.report.trim(), share)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        format!("The orchestration workers have finished. Check the combined result: read the changed files, run the relevant checks, fix small integration problems and identify incomplete work. Preserve the workers' changes. Then report the outcome to the user.\n\n{body}")
    }
    /// The sidebar's view of this run.
    pub fn summary(&self) -> Summary {
        Summary {
            live: self.active() && !self.paused,
            tasks: self
                .plan
                .tasks
                .iter()
                .zip(&self.tasks)
                .map(|(task, state)| SummaryTask {
                    session: state.session,
                    title: task.title.clone(),
                    provider: task
                        .provider
                        .clone()
                        .unwrap_or_else(|| self.provider.clone()),
                    model: task.model.clone().or_else(|| self.settings.model.clone()),
                    status: state.state.summary_status().into(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(id: &str, deps: &[&str], file: &str) -> Task {
        Task {
            id: id.into(),
            title: id.into(),
            prompt: "Work".into(),
            files: vec![file.into()],
            depends_on: deps.iter().map(|s| (*s).into()).collect(),
            provider: None,
            model: None,
        }
    }
    fn run(tasks: Vec<Task>) -> Run {
        Run::new(
            Plan {
                title: "Work".into(),
                summary: String::new(),
                tasks,
            },
            None,
            "/project".into(),
            "codex".into(),
            Default::default(),
        )
    }
    #[test]
    fn rejects_cycles_unknown_dependencies_and_scope_escapes() {
        let mut plan = Plan {
            title: "Work".into(),
            summary: String::new(),
            tasks: vec![task("a", &["b"], "src"), task("b", &["a"], "tests")],
        };
        assert!(validate(&plan).is_err());
        plan.tasks[1].depends_on.clear();
        assert!(validate(&plan).is_ok());
        plan.tasks[1].files = vec!["../elsewhere".into()];
        assert!(validate(&plan).is_err());
        plan.tasks[1].files = vec!["tests".into()];
        plan.tasks[1].provider = Some("nonsense".into());
        assert!(validate(&plan).is_err());
        assert!(overlaps(&["src".into()], &["src/main.rs".into()]));
        assert!(!overlaps(&["src".into()], &["src2/main.rs".into()]));
    }
    #[test]
    fn scopes_are_normalized_before_comparing() {
        assert!(overlaps(&["./src".into()], &["src".into()]));
        assert!(overlaps(&["Src/".into()], &["src/lib.rs".into()]));
        assert!(overlaps(&["./".into()], &["docs".into()]));
        assert_eq!(scope("./a//b/"), "a/b");
        assert_eq!(scope(""), ".");
    }
    #[test]
    fn parses_the_last_tagged_plan_with_fences_and_defaults() {
        let text = "I will mention <monocode_orchestration> in prose first.\n<monocode_orchestration>\n```json\n{\"title\":\"T\",\"tasks\":[{\"id\":\"a\",\"title\":\"A\",\"prompt\":\"Do\"}]}\n```\n</monocode_orchestration>";
        let plan = parse(text).unwrap();
        assert_eq!(plan.tasks[0].files, vec!["."]);
        assert!(plan.summary.is_empty());
        let input =
            json!({"title":"T","tasks":[{"id":"a","title":"A","prompt":"Do","files":["./src/"]}]});
        assert_eq!(from_input(&input).unwrap().tasks[0].files, vec!["src"]);
        assert!(from_input(&json!({"title":"T","tasks":[]})).is_err());
    }
    #[test]
    fn dispatch_caps_workers_serializes_scopes_and_blocks_dependants() {
        let mut run = run(vec![
            task("a", &[], "src"),
            task("b", &[], "./src/main.rs"),
            task("c", &["a"], "tests"),
            task("d", &[], "docs"),
            task("e", &[], "e"),
            task("f", &[], "f"),
        ]);
        assert_eq!(run.dispatch(), vec![0, 3, 4, 5]);
        assert_eq!(run.waiting_on(1).as_deref(), Some("scope busy with a"));
        assert_eq!(run.waiting_on(2).as_deref(), Some("after a"));
        for ix in [0, 3, 4, 5] {
            run.tasks[ix].state = State::Running;
        }
        run.finish(0, Outcome::Failed, "boom");
        assert_eq!(run.dispatch(), vec![1]);
        assert_eq!(run.tasks[2].state, State::Blocked);
        // Retrying the failed dependency unblocks its dependant.
        run.tasks[0].session = Some(7);
        assert!(run.retry(0));
        assert_eq!(run.tasks[0].state, State::Running);
        run.dispatch();
        assert_eq!(run.tasks[2].state, State::Pending);
        run.finish(0, Outcome::Completed, "ok");
        run.tasks[1].state = State::Done;
        assert_eq!(run.dispatch(), vec![2]);
    }
    #[test]
    fn a_stopped_worker_is_not_done_and_blocks_its_dependants() {
        let mut run = run(vec![task("a", &[], "src"), task("b", &["a"], "tests")]);
        run.dispatch();
        run.tasks[0].state = State::Running;
        run.tasks[0].session = Some(3);
        run.finish(0, Outcome::Stopped, "partial");
        run.dispatch();
        assert_eq!(run.tasks[0].state, State::Stopped);
        assert_eq!(run.tasks[1].state, State::Blocked);
        assert!(run.finished());
        assert!(run.report().contains("[stopped before finishing]"));
    }
    #[test]
    fn restore_interrupts_running_workers_and_resume_continues_them() {
        let mut first = run(vec![
            task("a", &[], "src"),
            task("b", &[], "docs"),
            task("c", &[], "x"),
        ]);
        first.dispatch();
        first.tasks[0].state = State::Running;
        first.tasks[0].session = Some(1);
        first.tasks[1].state = State::Done;
        let encoded = serde_json::to_string(&first).unwrap();
        let mut restored = serde_json::from_str::<Run>(&encoded).unwrap().restored();
        assert!(restored.paused);
        assert_eq!(restored.tasks[0].state, State::Interrupted);
        assert_eq!(restored.tasks[2].state, State::Pending);
        assert!(restored.dispatch().is_empty(), "paused runs wait");
        assert_eq!(restored.resume(), vec![0]);
        assert_eq!(restored.tasks[0].state, State::Running);
        assert_eq!(restored.dispatch(), vec![2]);
    }
    #[test]
    fn a_worker_that_never_started_receives_its_assignment_again() {
        let mut run = run(vec![task("a", &[], "src")]);
        run.dispatch();
        run.tasks[0].session = Some(4);
        run.finish(0, Outcome::NotStarted, "Account unavailable");
        assert_eq!(run.tasks[0].state, State::Failed);
        assert!(run.retry(0));
        assert!(
            !run.tasks[0].launched,
            "the worker gets the assignment again"
        );
        run.tasks[0].session = None;
        run.finish(0, Outcome::Failed, "Session creation failed");
        assert!(!run.retry(0));
        assert_eq!(run.tasks[0].state, State::Pending);
    }
    #[test]
    fn reports_keep_their_endings_and_only_dependencies_are_forwarded() {
        let mut run = run(vec![
            task("a", &[], "src"),
            task("b", &[], "docs"),
            task("c", &["a"], "x"),
        ]);
        let long = format!("{}FINAL SUMMARY", "x".repeat(20_000));
        run.finish(0, Outcome::Completed, &long);
        run.finish(1, Outcome::Completed, "docs report");
        assert!(run.tasks[0].report.ends_with("FINAL SUMMARY"));
        let reports = run.dependency_reports(2);
        assert!(reports.contains("FINAL SUMMARY") && !reports.contains("docs report"));
    }
    #[test]
    fn edits_outside_the_scopes_are_reported() {
        let task = task("a", &[], "src/lib.rs");
        let edited = vec![
            "/project/src/lib.rs".into(),
            "/project/src/other.rs".into(),
            "README.md".into(),
            "/elsewhere/file".into(),
        ];
        assert_eq!(
            outside_scope(&task, "/project", &edited),
            vec!["/elsewhere/file", "README.md", "src/other.rs"]
        );
        let mut whole = task.clone();
        whole.files = vec![".".into()];
        assert_eq!(
            outside_scope(&whole, "/project", &edited),
            vec!["/elsewhere/file"]
        );
    }
}
