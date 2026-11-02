//! Local reference layout for the chat presentation.
use monocode_core::{
    tool::{ToolCall, ToolState},
    transcript::Transcript,
};
pub(crate) fn chat() -> Transcript {
    let mut t = Transcript::default();
    t.begin("assistant");
    t.append("The new regression tests pass. They cover Markdown across storage chunks, repeated tool snapshots, and expanding reasoning while output changes. I also fixed a scroll issue where opening a disclosure could jump to the tail, and limited syntax highlighting to one background job at a time.");
    for (id, name, input, output, state) in [
        (
            "read",
            "Read",
            r#"{"file_path":"docs/native/migration.md"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "edit1",
            "Edit",
            r#"{"file_path":"crates/monocode-core/src/provider.rs"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "cmd1",
            "Bash",
            r#"{"command":"python3 — <<'PY'"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "cmd2",
            "Bash",
            r#"{"command":"cargo fmt --all"}"#,
            "error: let chains are only allowed in Rust 2024 or later",
            ToolState::Failed,
        ),
        (
            "edit2",
            "Edit",
            r#"{"file_path":"crates/monocode-core/src/provider.rs"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "find",
            "Grep",
            r#"{"pattern":"fn simulate_click|fn bounds_for|debug_bounds"}"#,
            "",
            ToolState::Running,
        ),
    ] {
        t.begin("tool");
        t.rows.last_mut().unwrap().text = serde_json::to_string(&ToolCall {
            id: id.into(),
            name: name.into(),
            input: input.into(),
            output: output.into(),
            state,
            panel: None,
            display: None,
        })
        .unwrap();
    }
    t
}

/// Normal-session examples; no provider credentials or live turn are needed.
pub(crate) fn visuals() -> Transcript {
    let mut transcript = Transcript::default();
    transcript.begin("user");
    transcript
        .append("Show interactive examples for exploring data and three-dimensional geometry.");
    for (id, title, source) in [
        (
            "example-dashboard",
            "Throughput explorer",
            include_str!("../../../docs/native/examples/visual-dashboard.js"),
        ),
        (
            "example-geometry",
            "Geometry explorer",
            include_str!("../../../docs/native/examples/visual-scene.js"),
        ),
    ] {
        let document = monocode_core::visual::VisualDocument {
            id: id.into(),
            title: title.into(),
            source: source.into(),
        };
        transcript.begin("visual");
        transcript.append(&serde_json::to_string(&document).unwrap());
    }
    transcript.begin("assistant");
    transcript.append("The examples use a sample dataset. Their controls update locally, so you can explore them without starting an agent turn.");
    transcript
}

/// Title-bar plan rail examples: each session's last todo list in another
/// state. Written as the providers write them; no live turn is needed.
const SETTINGS_SEARCH_PLAN: &str = "# Settings search

Search every settings page from one field, and jump to the matching row.

## Approach

1. **Index rows, not pages.** Give each settings row a stable id, its title and description, and the page it lives on.
2. **One field in the sidebar.** Filter the page list as you type; pages with no match fade out.
3. **Jump and highlight.** Selecting a result opens its page, scrolls the row into view and flashes it once.
4. **Keyboard first.** `Cmd+F` focuses the field, arrows move through results, `Enter` jumps.

## Files

- `settings.rs`: the index and the search field
- `settings/pages/*.rs`: register each row's id and text
- `settings/preferences.rs`: no change

## Not in scope

- Searching inside dropdown option labels
- Fuzzy matching beyond case-insensitive substrings";

pub(crate) fn plans() -> Vec<(&'static str, &'static str, Transcript)> {
    use monocode_core::tool::tasks::{Panel, Task, TaskList, TaskStatus};
    fn prompt(t: &mut Transcript, text: &str) {
        t.begin("user");
        t.append(text);
    }
    fn say(t: &mut Transcript, text: &str) {
        t.begin("assistant");
        t.append(text);
    }
    fn call(t: &mut Transcript, id: &str, name: &str, input: serde_json::Value) {
        t.begin("tool");
        t.rows.last_mut().unwrap().text = serde_json::to_string(&ToolCall {
            id: id.into(),
            name: name.into(),
            input: input.to_string(),
            output: String::new(),
            state: ToolState::Completed,
            panel: None,
            display: None,
        })
        .unwrap();
    }
    fn todos(t: &mut Transcript, id: &str, items: &[(&str, &str)]) {
        let todos: Vec<_> = items
            .iter()
            .map(|(content, status)| serde_json::json!({"content": content, "status": status}))
            .collect();
        call(t, id, "TodoWrite", serde_json::json!({ "todos": todos }));
    }
    fn reads(t: &mut Transcript, id: &str, files: &[&str]) {
        for (i, file) in files.iter().enumerate() {
            let (name, input) = if file.starts_with("cargo") || file.starts_with("bun") {
                ("Bash", serde_json::json!({ "command": file }))
            } else if i % 3 == 2 {
                ("Edit", serde_json::json!({ "file_path": file }))
            } else {
                ("Read", serde_json::json!({ "file_path": file }))
            };
            call(t, &format!("{id}-{i}"), name, input);
        }
    }
    let steps = [
        "Read store.rs and thread_files.rs",
        "Move thread file IO into the store module",
        "Wire the quick composer to the new store API",
        "Update the GitChangesPanel row window",
        "Run cargo test and bun test",
        "Write a summary",
    ];
    let with = |current: usize| -> Vec<(&str, &str)> {
        steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let status = match i.cmp(&current) {
                    std::cmp::Ordering::Less => "completed",
                    std::cmp::Ordering::Equal => "in_progress",
                    std::cmp::Ordering::Greater => "pending",
                };
                (*s, status)
            })
            .collect()
    };

    let mut mid = Transcript::default();
    prompt(
        &mut mid,
        "Move thread files into the store and wire the quick composer to it.",
    );
    say(
        &mut mid,
        "I'll start by reading the current store and thread file code.",
    );
    todos(&mut mid, "plan-0", &with(0));
    reads(
        &mut mid,
        "a",
        &[
            "crates/monocode-core/src/store.rs",
            "crates/monocode-core/src/store/thread_files.rs",
            "crates/monocode-core/src/store.rs",
        ],
    );
    todos(&mut mid, "plan-1", &with(1));
    reads(
        &mut mid,
        "b",
        &[
            "crates/monocode-core/src/store/thread_files.rs",
            "crates/monocode-core/src/store.rs",
            "crates/monocode-core/src/store/thread_files.rs",
            "cargo check -p monocode-core",
        ],
    );
    todos(&mut mid, "plan-2", &with(2));
    reads(
        &mut mid,
        "c",
        &[
            "crates/monocode-gpui/src/quick_composer.rs",
            "crates/monocode-gpui/src/shell/quick_composer.rs",
            "crates/monocode-gpui/src/quick_composer.rs",
            "crates/monocode-gpui/src/conversation/composer_features.rs",
            "cargo check -p monocode-gpui",
        ],
    );

    let mut done = Transcript::default();
    prompt(
        &mut done,
        "Split the GPUI branch into reviewable upstream PRs.",
    );
    let pr = [
        "Inventory commits on feat/gpui",
        "Group them into 4 PRs",
        "Create a branch per PR",
        "Write PR descriptions",
    ];
    todos(
        &mut done,
        "pr-0",
        &pr.iter()
            .map(|s| (*s, "in_progress"))
            .take(1)
            .chain(pr[1..].iter().map(|s| (*s, "pending")))
            .collect::<Vec<_>>(),
    );
    reads(
        &mut done,
        "d",
        &[
            "git log --oneline",
            "docs/native/migration.md",
            "git diff --stat",
        ],
    );
    todos(
        &mut done,
        "pr-1",
        &pr.iter().map(|s| (*s, "completed")).collect::<Vec<_>>(),
    );
    say(
        &mut done,
        "Four branches are ready, each with a description.",
    );

    let mut stale = Transcript::default();
    prompt(
        &mut stale,
        "Slide and collapse an archived thread's sidebar row.",
    );
    todos(
        &mut stale,
        "arch-0",
        &[
            ("Find the sidebar row renderer", "completed"),
            ("Add slide and collapse motion", "completed"),
            ("Add a small burst", "completed"),
        ],
    );
    say(
        &mut stale,
        "Archived rows now slide out and collapse with a small burst.",
    );
    prompt(&mut stale, "Now make the burst respect reduced motion.");
    reads(
        &mut stale,
        "e",
        &["crates/monocode-gpui/src/shell/archive_motion.rs"],
    );

    let mut revised = Transcript::default();
    prompt(&mut revised, "Give thread files their own table.");
    let mut plan = ToolCall {
        id: "plan:turn-7".into(),
        name: "update_plan".into(),
        input: String::new(),
        output: String::new(),
        state: ToolState::Completed,
        panel: None,
        display: None,
    };
    plan.panel = Some(Panel::Tasks(TaskList {
        items: [
            ("Read the store schema", TaskStatus::Completed),
            (
                "Add migration for the thread_files table",
                TaskStatus::Completed,
            ),
            ("Backfill existing threads", TaskStatus::InProgress),
            ("Update GitChangesPanel row window", TaskStatus::Cancelled),
            ("Move thread file IO into the store", TaskStatus::Pending),
            ("Run the core tests", TaskStatus::Pending),
        ]
        .into_iter()
        .map(|(text, status)| Task {
            id: None,
            text: text.into(),
            status,
        })
        .collect(),
        explanation:
            "The store split needs a migration first; the panel change is not part of this.".into(),
    }));
    reads(
        &mut revised,
        "f",
        &[
            "crates/monocode-core/src/store.rs",
            "crates/monocode-core/src/store/migrations.rs",
        ],
    );
    revised.begin("tool");
    revised.rows.last_mut().unwrap().text = serde_json::to_string(&plan).unwrap();
    reads(
        &mut revised,
        "g",
        &[
            "crates/monocode-core/src/store/migrations.rs",
            "crates/monocode-core/src/store.rs",
        ],
    );

    let mut none = Transcript::default();
    prompt(&mut none, "Fix the clippy warning in fs.rs.");
    reads(&mut none, "h", &["src-tauri/src/fs.rs", "cargo clippy"]);
    say(
        &mut none,
        "Fixed: the needless borrow in fs.rs is gone and clippy is clean.",
    );

    // Plan mode: Claude proposes with ExitPlanMode, Codex in <proposed_plan>.
    let mut proposed = Transcript::default();
    prompt(&mut proposed, "Plan how settings search should work.");
    reads(
        &mut proposed,
        "i",
        &[
            "crates/monocode-gpui/src/settings.rs",
            "crates/monocode-gpui/src/settings/pages/general.rs",
            "crates/monocode-gpui/src/settings/preferences.rs",
            "crates/monocode-gpui/src/settings/pages/chat.rs",
        ],
    );
    call(
        &mut proposed,
        "exit-plan",
        "ExitPlanMode",
        serde_json::json!({ "plan": SETTINGS_SEARCH_PLAN }),
    );

    let mut codex_plan = Transcript::default();
    prompt(&mut codex_plan, "Plan the retry for failed image uploads.");
    reads(
        &mut codex_plan,
        "j",
        &[
            "src-tauri/src/inbox_media.rs",
            "src/features/inbox/model/inboxMedia.ts",
        ],
    );
    say(
        &mut codex_plan,
        "I looked at how inbox media is uploaded today.\n\n<proposed_plan>\n\
         # Retry failed inbox uploads\n\n\
         1. Keep the failed file in the thread folder instead of dropping it.\n\
         2. Retry up to three times with backoff in `inbox_media.rs`.\n\
         3. Show a retry chip on the attachment when all tries fail.\n\
         4. Cover it with a test in `inboxMedia.test.ts`.\n\
         </proposed_plan>",
    );

    vec![
        ("claude", "Thread files into the store", mid),
        ("codex", "Thread files table", revised),
        ("claude", "Upstream PR split", done),
        ("claude", "Archive row motion", stale),
        ("codex", "Clippy fix in fs.rs", none),
        ("claude", "Settings search", proposed),
        ("codex", "Upload retry", codex_plan),
    ]
}

/// An answer that alternates prose with the visuals it placed, as an agent
/// writes one after publishing each visual with visual_show.
pub(crate) fn visual_story() -> Transcript {
    let mut transcript = Transcript::default();
    transcript.begin("user");
    transcript.append("How did deploys go this quarter? Anything I should worry about?");
    transcript.begin("assistant");
    transcript.append("I'll pull the deploy history and chart what changed.");
    let tool = |id: &str, name: &str, input: String| {
        let mut call = ToolCall::new(id, name);
        call.input = input;
        call.state = ToolState::Completed;
        serde_json::to_string(&call).unwrap()
    };
    transcript.begin("tool");
    transcript.append(&tool(
        "read-deploys",
        "Read",
        r#"{"file_path":"ops/deploys.json"}"#.into(),
    ));
    transcript.begin("tool");
    transcript.append(&tool(
        "count-deploys",
        "Bash",
        r#"{"command":"jq length ops/deploys.json"}"#.into(),
    ));
    for (id, title, source) in [
        (
            "deploys-weekly",
            "Deploys per week, last 12 weeks",
            r#"return () => visual.chart('bar', [{label: 'Deploys', values: [42, 48, 45, 55, 51, 58, 54, 63, 52, 61, 68, 74]}], {labels: ['W1','W2','W3','W4','W5','W6','W7','W8','W9','W10','W11','W12'], height: 200});"#,
        ),
        (
            "deploy-health",
            "Pipeline health this week",
            r#"return () => visual.grid([
  visual.stat('Success rate', '97.3%', {delta: '+1.2%', detail: 'vs last week', trend: [94, 95, 95.5, 94.8, 96, 96.4, 97.3]}),
  visual.stat('Median build', '6m 12s', {delta: '-48s', better: 'lower', detail: 'after the cache fix', trend: [8.1, 7.9, 7.6, 7.7, 6.9, 6.4, 6.2]}),
  visual.gauge('Runner capacity used', 82, {unit: '%', color: 'warning'}),
], {columns: 3, gap: 16});"#,
        ),
        (
            "failure-causes",
            "Where the 23 failed deploys came from",
            r#"return () => visual.donut([
  {label: 'Flaky e2e tests', value: 11},
  {label: 'Migrations', value: 6},
  {label: 'Config drift', value: 4},
  {label: 'Other', value: 2},
], {label: 'Failures', value: '23'});"#,
        ),
    ] {
        let document = monocode_core::visual::VisualDocument {
            id: id.into(),
            title: title.into(),
            source: source.into(),
        };
        let input = serde_json::json!({"title": title, "source": source}).to_string();
        transcript.begin("tool");
        transcript.append(&tool(
            &format!("check-{id}"),
            "monocode_visual:visual_check",
            input.clone(),
        ));
        transcript.begin("tool");
        transcript.append(&tool(
            &format!("show-{id}"),
            "monocode_visual:visual_show",
            input,
        ));
        transcript.begin("visual");
        transcript.append(&serde_json::to_string(&document).unwrap());
    }
    transcript.begin("assistant");
    transcript.append(
        "Deploys had a strong quarter: volume climbed steadily and finished at a record 74 in the last week.

![Deploys per week, last 12 weeks](visual:deploys-weekly)

The dips in weeks 3, 5 and 9 line up with the release freezes, so they aren't a reliability signal. What changed is the pipeline itself. The CI cache fix landed in week 10, and builds have been faster and steadier since:

![Pipeline health this week](visual:deploy-health)

The one thing to watch is runner capacity. At 82% you'll start queueing builds during the Monday rush, and the trend is still rising.

Failures are low overall, but they're concentrated:

![Where the 23 failed deploys came from](visual:failure-causes)

Nearly half come from flaky end-to-end tests rather than real regressions. Quarantining the five worst tests would remove most of that noise. I'd do that before adding runners.",
    );
    transcript
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_visual_examples_pass_the_real_runtime_and_persist_as_messages() {
        let rows = super::visuals().rows;
        let mut index = 0;
        let mut count = 0;
        while index < rows.len() {
            let end = (index + 1..rows.len())
                .find(|&next| rows[next].first)
                .unwrap_or(rows.len());
            if rows[index].role == "visual" {
                let source = rows[index..end]
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<String>();
                let document: monocode_core::visual::VisualDocument =
                    serde_json::from_str(&source).unwrap();
                document.validate().unwrap();
                monocode_core::visual::check(&document.source).unwrap();
                count += 1;
            }
            index = end;
        }
        assert_eq!(count, 2);
    }

    #[test]
    fn the_story_answer_places_every_visual_it_publishes() {
        let rows = super::visual_story().rows;
        let answer: String = rows
            .iter()
            .skip_while(|row| row.role != "assistant")
            .map(|row| row.text.as_str())
            .collect();
        let mut placed = std::collections::HashSet::new();
        crate::transcript::placements(&answer, &mut placed);
        assert_eq!(placed.len(), 3);
        let mut index = 0;
        let mut published = 0;
        while index < rows.len() {
            let end = (index + 1..rows.len())
                .find(|&next| rows[next].first)
                .unwrap_or(rows.len());
            if rows[index].role == "visual" {
                let source: String = rows[index..end].iter().map(|r| r.text.as_str()).collect();
                let document: monocode_core::visual::VisualDocument =
                    serde_json::from_str(&source).unwrap();
                monocode_core::visual::check(&document.source).unwrap();
                assert!(placed.contains(&document.id));
                published += 1;
            }
            index = end;
        }
        assert_eq!(published, 3);
    }
}
