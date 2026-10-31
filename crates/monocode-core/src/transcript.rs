//! Incremental, UTF-8-safe transcript blocks. Even a giant single answer is virtualizable.
use serde::{Deserialize, Serialize};

pub mod attachments;
pub mod images;

pub const ROW_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interjection {
    pub text: String,
    pub custom_type: String,
    pub severity: Option<String>,
}
impl Interjection {
    pub fn label(&self) -> &str {
        match self.custom_type.as_str() {
            "advisor" => "Advisor",
            "custom" => "Notice",
            label => match self.goal_change() {
                Some(GoalChange::Set) => "Goal set",
                Some(GoalChange::Check) => "Goal not met yet",
                Some(GoalChange::Paused) => "Goal paused",
                Some(GoalChange::Met) => "Goal met",
                Some(GoalChange::Cleared) => "Goal cleared",
                None => label,
            },
        }
    }

    pub fn goal(change: GoalChange, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            custom_type: change.custom_type().into(),
            severity: None,
        }
    }

    pub fn goal_change(&self) -> Option<GoalChange> {
        GoalChange::parse(&self.custom_type)
    }
}

/// A standing goal (`/goal`) the agent keeps working toward across turns.
/// Each change is kept as an interjection, so every client that shows the
/// thread also shows the goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalChange {
    /// The note's text is the objective.
    Set,
    /// The agent tried to stop early; the text says what is still missing.
    Check,
    /// The text names why the goal stopped running.
    Paused,
    Met,
    Cleared,
}
impl GoalChange {
    const ALL: [Self; 5] = [
        Self::Set,
        Self::Check,
        Self::Paused,
        Self::Met,
        Self::Cleared,
    ];

    pub fn custom_type(self) -> &'static str {
        match self {
            Self::Set => "goal",
            Self::Check => "goal-check",
            Self::Paused => "goal-paused",
            Self::Met => "goal-met",
            Self::Cleared => "goal-cleared",
        }
    }

    fn parse(custom_type: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|change| change.custom_type() == custom_type)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveGoal {
    pub objective: String,
    /// How many times the agent was sent back to keep working.
    pub checks: usize,
    pub paused: bool,
}

/// The goal still in force after these rows, if any.
pub fn active_goal(rows: &[Row]) -> Option<ActiveGoal> {
    let mut goal: Option<ActiveGoal> = None;
    let mut at = 0;
    while at < rows.len() {
        let end = (at + 1..rows.len())
            .find(|&i| rows[i].first)
            .unwrap_or(rows.len());
        if rows[at].role == "interjection" {
            let text: String = rows[at..end].iter().map(|r| r.text.as_str()).collect();
            let note = serde_json::from_str::<Interjection>(&text).ok();
            match note
                .as_ref()
                .and_then(|note| Some((note, note.goal_change()?)))
            {
                Some((note, GoalChange::Set)) => {
                    goal = Some(ActiveGoal {
                        objective: note.text.clone(),
                        checks: 0,
                        paused: false,
                    })
                }
                Some((_, GoalChange::Check)) => {
                    if let Some(goal) = &mut goal {
                        goal.checks += 1;
                        goal.paused = false;
                    }
                }
                Some((_, GoalChange::Paused)) => {
                    if let Some(goal) = &mut goal {
                        goal.paused = true;
                    }
                }
                Some((_, GoalChange::Met | GoalChange::Cleared)) => goal = None,
                None => {}
            }
        }
        at = end;
    }
    goal
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub role: String,
    pub text: String,
    pub first: bool,
    /// Wall-clock send time on the first row; older histories may not have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<u64>,
}
impl Row {
    /// Opens a turn's prompt block. A composer `!command` belongs to the
    /// message it precedes, not to the previous turn's response.
    pub fn is_prompt(&self) -> bool {
        matches!(self.role.as_str(), "user" | "mono-system" | "shell")
    }
}

#[derive(Default, Debug)]
pub struct Transcript {
    pub rows: Vec<Row>,
}

impl Transcript {
    pub fn begin(&mut self, role: &str) -> usize {
        let ix = self.rows.len();
        self.rows.push(Row {
            role: role.into(),
            text: String::new(),
            first: true,
            sent_at: Some(crate::time::now_ms()),
        });
        ix
    }

    /// Returns the first changed row. Earlier rows retain their identities and content.
    pub fn append(&mut self, mut text: &str) -> usize {
        assert!(!self.rows.is_empty(), "begin a message before appending");
        let changed = self.rows.len() - 1;
        while !text.is_empty() {
            let last = self.rows.last_mut().unwrap();
            let lines = last.text.matches('\n').count();
            let room = if lines >= 12 {
                0
            } else {
                ROW_BYTES - last.text.len()
            };
            let mut end = room.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            // Bound height as well as bytes, including pathological newline floods.
            if let Some((ix, _)) = text[..end].match_indices('\n').nth(11 - lines.min(11)) {
                end = ix + 1;
            }
            if end == 0 {
                let role = last.role.clone();
                self.rows.push(Row {
                    role,
                    text: String::new(),
                    first: false,
                    sent_at: None,
                });
                continue;
            }
            last.text.push_str(&text[..end]);
            text = &text[end..];
            if !text.is_empty() {
                let role = last.role.clone();
                self.rows.push(Row {
                    role,
                    text: String::new(),
                    first: false,
                    sent_at: None,
                });
            }
        }
        changed
    }
}

/// Port of integrations/harness/core/streamText.ts::snapshotRemainder.
/// Tokens append verbatim. Completed snapshots contribute only missing suffixes.
pub fn snapshot_remainder<'a>(already: &str, snapshot: &'a str) -> &'a str {
    if let Some(suffix) = snapshot.strip_prefix(already) {
        suffix
    } else if already.starts_with(snapshot) {
        ""
    } else {
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn giant_unicode_message_and_newline_flood_are_bounded() {
        let mut transcript = Transcript::default();
        transcript.begin("assistant");
        let source = "🙂e\u{301}漢字\n".repeat(40_000);
        transcript.append(&source);
        assert!(transcript.rows.iter().all(|r| r.text.len() <= ROW_BYTES));
        assert!(transcript
            .rows
            .iter()
            .all(|r| r.text.matches('\n').count() <= 12));
        assert_eq!(
            transcript
                .rows
                .iter()
                .map(|r| r.text.as_str())
                .collect::<String>(),
            source
        );
    }
    #[test]
    fn append_changes_only_tail_and_preserves_blank_lines() {
        let mut t = Transcript::default();
        t.begin("assistant");
        t.append(&"x".repeat(ROW_BYTES));
        let prefix = t.rows[0].clone();
        t.append("\n");
        t.append("\n");
        assert_eq!(t.rows[0], prefix);
        assert_eq!(t.rows[1].text, "\n\n");
        assert_eq!(snapshot_remainder("Hello", "Hello world"), " world");
        assert_eq!(snapshot_remainder("Hello world", "Hello"), "");
        assert_eq!(snapshot_remainder("Hello", "Hello"), "");
    }
    #[test]
    fn active_goal_follows_the_newest_goal_note() {
        fn note(t: &mut Transcript, change: GoalChange, text: &str) {
            t.begin("interjection");
            t.append(&serde_json::to_string(&Interjection::goal(change, text)).unwrap());
        }
        let mut t = Transcript::default();
        assert_eq!(active_goal(&t.rows), None);
        let objective = "tests pass ".repeat(400);
        note(&mut t, GoalChange::Set, &objective);
        assert!(t.rows.len() > 1, "a long objective spans rows");
        t.begin("assistant");
        t.append("Working on it");
        note(&mut t, GoalChange::Check, "two tests still fail");
        note(&mut t, GoalChange::Paused, "usage limit");
        assert_eq!(
            active_goal(&t.rows),
            Some(ActiveGoal {
                objective: objective.clone(),
                checks: 1,
                paused: true
            })
        );
        note(&mut t, GoalChange::Met, "");
        assert_eq!(active_goal(&t.rows), None);
        assert_eq!(
            Interjection::goal(GoalChange::Check, "x").label(),
            "Goal not met yet"
        );
    }
}
