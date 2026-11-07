//! Keeps phone alerts for when the user is away from the Mac. While they use
//! it, an alert is held here; once the Mac has sat idle (or locked) long
//! enough, the ones still worth hearing go out. The app tells the host which
//! of the three it is in; the host does the holding.
use super::notify::{Alert, Notice};
use serde_json::Value;
use std::time::{Duration, Instant};

/// A held alert older than this is no news, however long the user stayed.
const MAX_AGE: Duration = Duration::from_secs(10 * 60);
/// A phone is sent a few alerts one by one; more than this, one summary.
pub(super) const DIGEST_AFTER: usize = 3;

/// Whether the user is at the Mac, as far as phone alerts care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Attendance {
    /// The setting is off: every alert goes straight to the phone.
    #[default]
    Ignore,
    /// Using the Mac: alerts wait.
    AtDesk,
    /// Idle long enough, or locked: alerts go out, held ones with them.
    Away,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Held {
    /// From a change in the published catalog.
    Alert(Alert),
    /// From a desktop event, in a category the phone chose.
    Notice { category: String, notice: Notice },
}

impl Held {
    pub(super) fn category(&self) -> &str {
        match self {
            Held::Alert(alert) => alert.category,
            Held::Notice { category, .. } => category,
        }
    }

    fn title(&self) -> &str {
        match self {
            Held::Alert(alert) => &alert.title,
            Held::Notice { notice, .. } => &notice.title,
        }
    }

    fn session(&self) -> Option<&str> {
        match self {
            Held::Alert(alert) => Some(&alert.session_id),
            Held::Notice { notice, .. } => notice.data["sessionId"].as_str(),
        }
    }

    /// What it is about, so a newer alert replaces an older one: a session's
    /// "finished" supersedes its "needs approval", an Inbox item stands alone.
    fn key(&self) -> String {
        match (self.session(), self) {
            (Some(session), _) => format!("session:{session}"),
            (None, Held::Notice { notice, .. }) if notice.data["key"].is_string() => {
                format!("item:{}", notice.data["key"].as_str().unwrap_or_default())
            }
            (None, Held::Notice { category, notice }) => {
                format!("{category}:{}:{}", notice.title, notice.body)
            }
            (None, Held::Alert(_)) => unreachable!("an alert always names its session"),
        }
    }

    /// Still worth a push now, judging by what the catalog shows.
    fn current(&self, catalog: &Value) -> bool {
        let (Some(id), "agentInput" | "agentFinished") = (self.session(), self.category()) else {
            return true;
        };
        let Some(session) = catalog["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|session| session["id"] == id)
        else {
            // Not in the catalog (a Mono's chat, say): nothing to check it by.
            return !matches!(self, Held::Alert(_));
        };
        if self.category() == "agentInput" {
            session["waiting"].is_string()
        } else {
            session["waiting"].is_null() && session["busy"] != true
        }
    }
}

/// One push standing in for a crowd of alerts.
#[derive(Debug, PartialEq)]
pub(super) struct Digest {
    pub title: String,
    pub body: String,
    /// Something in it waits on the user.
    pub urgent: bool,
}

/// Counts what the alerts are and names the one to look at first: a session
/// waiting on the user, else the first.
pub(super) fn digest(items: &[&Held]) -> Digest {
    let count = |category: &str| items.iter().filter(|h| h.category() == category).count();
    let (input, done) = (count("agentInput"), count("agentFinished"));
    let other = items.len() - input - done;
    let parts: Vec<String> = [
        (input, "waiting on you"),
        (done, "finished"),
        (other, "other"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    let first = items
        .iter()
        .find(|h| h.category() == "agentInput")
        .or(items.first())
        .map(|h| h.title())
        .filter(|title| !title.is_empty())
        .unwrap_or("Session");
    Digest {
        title: format!("{} updates while you were away", items.len()),
        body: format!("{}. Start with {first}.", parts.join(", ")),
        urgent: input > 0,
    }
}

#[derive(Default)]
pub(super) struct Presence {
    attendance: Attendance,
    held: Vec<(Instant, Held)>,
}

impl Presence {
    pub(super) fn attendance(&self) -> Attendance {
        self.attendance
    }

    /// Records the app's report. Returns what to send if it frees held alerts.
    pub(super) fn set(&mut self, attendance: Attendance, catalog: &Value) -> Vec<Held> {
        self.attendance = attendance;
        if attendance == Attendance::AtDesk {
            return Vec::new();
        }
        self.release(catalog)
    }

    /// Holds `held` if the user is at the Mac. Returns it back when it should
    /// go now.
    pub(super) fn hold(&mut self, held: Held) -> Option<Held> {
        if self.attendance != Attendance::AtDesk {
            return Some(held);
        }
        let key = held.key();
        self.held
            .retain(|(at, other)| at.elapsed() < MAX_AGE && other.key() != key);
        self.held.push((Instant::now(), held));
        None
    }

    /// The user looked at `session` on the desktop: a finish they saw is old
    /// news. A question still unanswered stays.
    pub(super) fn seen(&mut self, session: &str) {
        self.held.retain(|(_, held)| {
            held.session() != Some(session) || held.category() != "agentFinished"
        });
    }

    fn release(&mut self, catalog: &Value) -> Vec<Held> {
        std::mem::take(&mut self.held)
            .into_iter()
            .filter(|(at, held)| at.elapsed() < MAX_AGE && held.current(catalog))
            .map(|(_, held)| held)
            .collect()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn alert(category: &'static str, session: &str) -> Held {
        Held::Alert(Alert {
            category,
            session_id: session.into(),
            project_id: "p".into(),
            title: "Fix push".into(),
            subtitle: None,
            body: "Finished".into(),
            urgent: false,
        })
    }

    fn catalog(sessions: Value) -> Value {
        json!({"projects":[],"monos":[],"sessions":sessions})
    }

    #[test]
    fn nothing_is_held_when_the_setting_is_off_or_the_user_is_away() {
        let mut presence = Presence::default();
        assert!(presence.hold(alert("agentFinished", "a")).is_some());
        presence.set(Attendance::Away, &catalog(json!([])));
        assert!(presence.hold(alert("agentFinished", "a")).is_some());
        assert_eq!(presence.len(), 0);
    }

    #[test]
    fn alerts_wait_at_the_desk_and_go_out_when_the_user_leaves() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        assert!(presence.hold(alert("agentFinished", "a")).is_none());
        assert!(presence.hold(alert("agentInput", "b")).is_none());
        let now = catalog(json!([
            {"id":"a","busy":false},
            {"id":"b","busy":true,"waiting":"approval"},
        ]));
        let sent = presence.set(Attendance::Away, &now);
        assert_eq!(sent.len(), 2);
        assert_eq!(presence.len(), 0);
    }

    #[test]
    fn a_question_answered_at_the_desk_is_not_sent_later() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        presence.hold(alert("agentInput", "b"));
        let answered = catalog(json!([{"id":"b","busy":true}]));
        assert!(presence.set(Attendance::Away, &answered).is_empty());
    }

    #[test]
    fn a_finish_that_started_working_again_is_not_sent() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        presence.hold(alert("agentFinished", "a"));
        let again = catalog(json!([{"id":"a","busy":true}]));
        assert!(presence.set(Attendance::Away, &again).is_empty());
    }

    #[test]
    fn a_session_has_one_held_alert_and_the_newest_wins() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        presence.hold(alert("agentInput", "a"));
        presence.hold(alert("agentFinished", "a"));
        assert_eq!(presence.len(), 1);
        let done = catalog(json!([{"id":"a","busy":false}]));
        let sent = presence.set(Attendance::Away, &done);
        assert!(matches!(&sent[0], Held::Alert(a) if a.category == "agentFinished"));
    }

    #[test]
    fn a_finish_seen_on_the_desktop_is_dropped_but_a_question_stays() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        presence.hold(alert("agentFinished", "a"));
        presence.hold(alert("agentInput", "b"));
        presence.seen("a");
        presence.seen("b");
        assert_eq!(presence.len(), 1);
    }

    #[test]
    fn notices_without_a_session_are_held_and_sent_as_they_are() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        let mono = Held::Notice {
            category: "monoMessages".into(),
            notice: Notice {
                title: "Scout".into(),
                subtitle: None,
                body: "Found it".into(),
                data: json!({"monoId":"m"}),
            },
        };
        assert!(presence.hold(mono.clone()).is_none());
        assert_eq!(presence.set(Attendance::Away, &catalog(json!([]))), [mono]);
    }

    #[test]
    fn a_crowd_becomes_one_summary_that_leads_with_what_waits_on_the_user() {
        let items = [
            alert("agentFinished", "a"),
            alert("agentFinished", "b"),
            alert("agentInput", "c"),
            alert("agentFinished", "d"),
        ];
        let refs: Vec<&Held> = items.iter().collect();
        let digest = digest(&refs);
        assert_eq!(digest.title, "4 updates while you were away");
        assert_eq!(
            digest.body,
            "1 waiting on you, 3 finished. Start with Fix push."
        );
        assert!(digest.urgent);
    }

    #[test]
    fn going_back_to_the_desk_keeps_what_is_held() {
        let mut presence = Presence::default();
        presence.set(Attendance::AtDesk, &catalog(json!([])));
        presence.hold(alert("agentFinished", "a"));
        assert!(presence
            .set(Attendance::AtDesk, &catalog(json!([])))
            .is_empty());
        assert_eq!(presence.len(), 1);
    }
}
