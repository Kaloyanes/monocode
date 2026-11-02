//! `/goal` keeps an agent working until a condition holds. Claude reports it
//! only as local command output and Stop hook feedback; Codex has its own
//! thread goal notifications.
use super::*;
use crate::transcript::GoalChange;

const CLAUDE_SYNTHETIC: &str = "<synthetic>";
const CLAUDE_FEEDBACK: &str = "Stop hook feedback:\n[";

/// Claude's own output for `/goal <condition>` and `/goal clear`.
fn claude_goal_note(text: &str) -> Option<(GoalChange, &str)> {
    if let Some(objective) = text.strip_prefix("Goal set: ") {
        Some((GoalChange::Set, objective))
    } else {
        Some((GoalChange::Cleared, text.strip_prefix("Goal cleared: ")?))
    }
}

impl Protocol {
    /// Turns `Goal set:` and `Goal cleared:` into goal events instead of an
    /// answer. Every other assistant frame is left to the caller.
    pub(super) fn claude_goal_frame(&mut self, v: &Value, events: &mut Vec<Event>) -> bool {
        if v["message"]["model"] != CLAUDE_SYNTHETIC || !v["parent_tool_use_id"].is_null() {
            self.claude_model_turn |= v["parent_tool_use_id"].is_null();
            return false;
        }
        let text = v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect::<String>();
        let Some((change, objective)) = claude_goal_note(&text) else {
            return false;
        };
        self.goal = (change == GoalChange::Set).then(|| (objective.into(), "active".into()));
        events.push(Event::Goal(change, objective.into()));
        true
    }

    /// Whether a result's text repeats a goal note already shown.
    pub(super) fn claude_goal_result(text: &str) -> bool {
        claude_goal_note(text).is_some()
    }

    /// The goal's Stop hook sends Claude back with what is still missing.
    pub(super) fn claude_goal_check(&mut self, v: &Value, events: &mut Vec<Event>) {
        let Some((objective, _)) = &self.goal else {
            return;
        };
        let prefix = format!("{CLAUDE_FEEDBACK}{objective}]: ");
        let feedback = v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .chain(v["message"]["content"].as_str())
            .find_map(|text| text.strip_prefix(prefix.as_str()));
        if let Some(missing) = feedback {
            events.push(Event::Goal(GoalChange::Check, missing.trim().into()));
        }
    }

    /// Claude's goal hook lets a turn end only once the goal holds, so a turn
    /// the model answered that ends cleanly closes it.
    pub(super) fn claude_goal_end(&mut self, events: &mut Vec<Event>) {
        if self.claude_model_turn && self.goal.take().is_some() {
            events.push(Event::Goal(GoalChange::Met, String::new()));
        }
    }

    pub(super) fn codex_goal(&mut self, method: &str, p: &Value, events: &mut Vec<Event>) {
        if method == "thread/goal/cleared" {
            if self.goal.take().is_some() {
                events.push(Event::Goal(GoalChange::Cleared, String::new()));
            }
            return;
        }
        let goal = &p["goal"];
        let (Some(objective), Some(status)) = (goal["objective"].as_str(), goal["status"].as_str())
        else {
            return;
        };
        // Usage counters update the goal too; only its state is news.
        if self
            .goal
            .as_ref()
            .is_some_and(|(o, s)| o == objective && s == status)
        {
            return;
        }
        let event = match status {
            "active" => Event::Goal(GoalChange::Set, objective.into()),
            "complete" => Event::Goal(GoalChange::Met, String::new()),
            "paused" => Event::Goal(GoalChange::Paused, "Paused".into()),
            "blocked" => Event::Goal(GoalChange::Paused, "Blocked".into()),
            "usageLimited" => Event::Goal(GoalChange::Paused, "Usage limit reached".into()),
            "budgetLimited" => Event::Goal(GoalChange::Paused, "Token budget used up".into()),
            _ => return,
        };
        self.goal = (status != "complete").then(|| (objective.into(), status.into()));
        events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protocol(kind: Kind, goal: Option<&str>) -> Protocol {
        let mut settings = crate::agent::Settings::default();
        settings.goal = goal.map(Into::into);
        Protocol::new(Request {
            kind,
            binary: "unused".into(),
            cwd: "/tmp".into(),
            prompt: "Hello".into(),
            provider_id: None,
            model: None,
            settings,
        })
    }

    fn goals(events: &[Event]) -> Vec<(GoalChange, String)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Goal(change, text) => Some((*change, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn synthetic(text: &str) -> Value {
        json!({"type":"assistant","parent_tool_use_id":null,"message":{
            "model":"<synthetic>","role":"assistant","content":[{"type":"text","text":text}]}})
    }

    fn answer(text: &str) -> Value {
        json!({"type":"assistant","parent_tool_use_id":null,"message":{
            "model":"claude-haiku-5-5","role":"assistant","content":[{"type":"text","text":text}]}})
    }

    fn feedback(text: &str) -> Value {
        json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":text}]}})
    }

    fn result(text: &str) -> Value {
        json!({"type":"result","subtype":"success","is_error":false,"result":text,"usage":{}})
    }

    #[test]
    fn claude_goal_is_set_checked_and_met_without_showing_its_note_as_an_answer() {
        let mut p = protocol(Kind::Claude, None);
        let mut out = Vec::new();
        let mut events = Vec::new();
        for frame in [
            synthetic("Goal set: tests pass"),
            answer("Running tests"),
            feedback("Stop hook feedback:\n[some-command.sh]: unrelated"),
            feedback("Stop hook feedback:\n[tests pass]: two tests still fail"),
            answer("All green"),
            result("All green"),
        ] {
            events.extend(p.frame(frame, &mut out).unwrap());
        }
        assert_eq!(
            goals(&events),
            [
                (GoalChange::Set, "tests pass".into()),
                (GoalChange::Check, "two tests still fail".into()),
                (GoalChange::Met, String::new()),
            ]
        );
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Running testsAll green");
    }

    #[test]
    fn claude_goal_clear_ends_the_goal_and_is_not_repeated_from_the_result() {
        let mut p = protocol(Kind::Claude, Some("tests pass"));
        let mut out = Vec::new();
        let mut events = p
            .frame(synthetic("Goal cleared: tests pass"), &mut out)
            .unwrap();
        events.extend(
            p.frame(result("Goal cleared: tests pass"), &mut out)
                .unwrap(),
        );
        assert_eq!(goals(&events), [(GoalChange::Cleared, "tests pass".into())]);
        assert!(!events.iter().any(|e| matches!(e, Event::Text(_))));
        assert!(events.contains(&Event::Done));
    }

    #[test]
    fn claude_goal_from_an_earlier_turn_closes_when_a_later_turn_ends() {
        let mut p = protocol(Kind::Claude, Some("tests pass"));
        let mut out = Vec::new();
        let mut events = p.frame(answer("Done now"), &mut out).unwrap();
        events.extend(p.frame(result("Done now"), &mut out).unwrap());
        assert_eq!(goals(&events), [(GoalChange::Met, String::new())]);

        // Local command output alone is not the model finishing the goal.
        let mut p = protocol(Kind::Claude, Some("tests pass"));
        let events = p.frame(result("Compacted"), &mut out).unwrap();
        assert!(goals(&events).is_empty());
    }

    #[test]
    fn codex_goal_reports_only_state_changes() {
        let mut p = protocol(Kind::Codex, Some("ship it"));
        let mut events = Vec::new();
        let goal = |status: &str, tokens: u64| json!({"goal":{"objective":"ship it","status":status,"tokensUsed":tokens}});
        for (method, params) in [
            ("thread/goal/updated", goal("active", 10)),
            ("thread/goal/updated", goal("active", 20)),
            ("thread/goal/updated", goal("budgetLimited", 30)),
            ("thread/goal/updated", goal("complete", 40)),
            ("thread/goal/cleared", json!({})),
        ] {
            p.codex_goal(method, &params, &mut events);
        }
        assert_eq!(
            goals(&events),
            [
                (GoalChange::Paused, "Token budget used up".into()),
                (GoalChange::Met, String::new()),
            ]
        );
    }
}
