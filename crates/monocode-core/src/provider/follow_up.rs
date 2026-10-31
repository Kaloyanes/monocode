//! The same mid-turn wire messages used by the React provider adapters.
use super::*;
use crate::follow_up::Message;

static STEER_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Kind {
    pub fn can_steer(self) -> bool {
        matches!(
            self,
            Self::Codex
                | Self::Claude
                | Self::Cursor
                | Self::Pi
                | Self::Omp
                | Self::Hermes
                | Self::OpenCode
        )
    }
}

impl Protocol {
    pub(super) fn ready(&self) -> bool {
        match self.request.kind {
            Kind::Codex => self.active_turn.is_some(),
            Kind::Claude | Kind::Pi | Kind::Omp => self.follow_up_ready,
            _ => self.acp_active_session().is_some(),
        }
    }
    pub(super) fn steer(
        &mut self,
        message: Message,
        stdin: &mut impl Write,
    ) -> Result<Vec<Event>, String> {
        if !self.request.kind.can_steer() || !self.ready() || self.done {
            return Ok(vec![Event::FollowUpFailed {
                id: message.id,
                error: "The provider is not ready for a follow-up. Resume the queue to retry."
                    .into(),
            }]);
        }
        let mut request = self.request.clone();
        request.settings.attachments = message.attachments.clone();
        request.prompt = super::commands::prepare_prompt(
            request.kind,
            message.prepared_prompt.as_deref().unwrap_or(&message.text),
            &request.cwd.to_string_lossy(),
            &message.attachments,
        )?;
        // A retained message can be retried after its acknowledgement deadline.
        // Never let a late reply from that attempt acknowledge the retry.
        let wire = format!(
            "native-steer-{}-{}",
            message.id,
            STEER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        match request.kind {
            Kind::Codex => write_json(
                stdin,
                json!({"id":wire,"method":"turn/steer","params":{"threadId":self.request.provider_id,"expectedTurnId":self.active_turn,"input":request.codex_input()}}),
            )?,
            Kind::Claude => {
                let prompt = if request.settings.effort.as_deref() == Some("ultrathink") {
                    if request.prompt.is_empty() {
                        "Ultrathink:".into()
                    } else {
                        format!("Ultrathink:\n{}", request.prompt)
                    }
                } else {
                    request.prompt.clone()
                };
                // Claude takes a message between steps, or as a new turn once
                // this one ends, and echoes its uuid then. Until that echo the
                // message is not delivered.
                let uuid = uuid::Uuid::new_v4().to_string();
                write_json(
                    stdin,
                    json!({"type":"user","uuid":uuid,"session_id":"","parent_tool_use_id":null,"message":{"role":"user","content":request.claude_content(&prompt)?}}),
                )?;
                self.steering.insert(uuid, (message, Instant::now()));
                return Ok(Vec::new());
            }
            Kind::Pi | Kind::Omp => {
                let mut command = json!({"id":wire,"type":"steer","message":request.prompt.trim()});
                let images = request.pi_images()?;
                if images.as_array().is_some_and(|images| !images.is_empty()) {
                    command["images"] = images;
                }
                // OMP routes slash commands through prompt even in flight.
                // A steer command would deliver the slash text as user prose.
                if request.kind == Kind::Omp && request.prompt.trim_start().starts_with('/') {
                    command["type"] = "prompt".into();
                    command["streamingBehavior"] = "steer".into();
                }
                write_json(stdin, command)?;
            }
            Kind::Cursor => {
                let params =
                    json!({"sessionId":self.acp_active_session(),"prompt":request.acp_content()?});
                // Match Cursor's adapter: retry the older notification name
                // only when writing the current one fails.
                if write_json(
                    stdin,
                    json!({"jsonrpc":"2.0","method":"session/steer","params":params}),
                )
                .is_err()
                {
                    write_json(
                        stdin,
                        json!({"jsonrpc":"2.0","method":"_session/steer","params":params}),
                    )?;
                }
                self.text_key = None;
                return Ok(vec![Event::FollowUp(message)]);
            }
            Kind::Hermes | Kind::OpenCode => write_json(
                stdin,
                json!({"jsonrpc":"2.0","id":wire,"method":"session/prompt","params":{"sessionId":self.acp_active_session(),"prompt":request.acp_content()?}}),
            )?,
            _ => unreachable!(),
        }
        self.steering.insert(wire, (message, Instant::now()));
        Ok(Vec::new())
    }
    pub(super) fn steer_response(&mut self, value: &Value) -> Option<Vec<Event>> {
        let wire = value["id"].as_str()?;
        let error = value.get("error").filter(|error| !error.is_null());
        let response = if self.request.kind.is_pi() {
            value["type"] == "response"
        } else {
            value.get("result").is_some() || error.is_some()
        };
        if !response {
            return None;
        }
        // A timed-out request can answer later. It must never be interpreted as
        // a fatal error for the unrelated turn still running.
        if wire.starts_with("native-steer-") && !self.steering.contains_key(wire) {
            return Some(Vec::new());
        }
        let (message, _) = self.steering.remove(wire)?;
        if error.is_some() || (self.request.kind.is_pi() && value["success"] != true) {
            let error = value["error"]["message"]
                .as_str()
                .or_else(|| value["error"].as_str())
                .unwrap_or("The provider could not accept this follow-up");
            Some(vec![Event::FollowUpFailed {
                id: message.id,
                error: error.into(),
            }])
        } else {
            self.text_key = None;
            Some(vec![Event::FollowUp(message)])
        }
    }
    /// Claude's echo of a steer it has just taken.
    pub(super) fn claude_replay(&mut self, value: &Value) -> Option<Vec<Event>> {
        let (message, _) = self.steering.remove(value["uuid"].as_str()?)?;
        self.text_key = None;
        Some(vec![Event::FollowUp(message)])
    }
    /// Claude still runs a message it was handed after an interrupt or a turn
    /// that ended without taking it, so that process must not be reused.
    pub(super) fn holds_steers(&self) -> bool {
        self.request.kind == Kind::Claude && (self.claude_stale || !self.steering.is_empty())
    }
    /// A finished Claude turn with steers still out starts another turn for
    /// them. If it never does, hand them back to be sent as the next turn.
    pub(super) fn expire_claude_steers(&mut self) -> Vec<Event> {
        if !self
            .claude_steer_deadline
            .is_some_and(|at| Instant::now() >= at)
        {
            return Vec::new();
        }
        self.claude_steer_deadline = None;
        self.claude_stale = true;
        self.steering
            .drain()
            .map(|(_, (message, _))| Event::FollowUpFailed {
                id: message.id,
                error: "Claude did not take this message before its turn ended".into(),
            })
            .collect()
    }
    pub(super) fn expire_steering(&mut self) -> Vec<Event> {
        // These are acknowledgement deadlines, not turn deadlines. Codex's
        // JSON-RPC steer request deliberately waits without a response timer.
        let timeout = match self.request.kind {
            Kind::Pi | Kind::Omp => Duration::from_secs(15),
            Kind::Hermes => Duration::from_secs(20),
            Kind::OpenCode => Duration::from_secs(30),
            _ => return Vec::new(),
        };
        let expired: Vec<_> = self
            .steering
            .iter()
            .filter(|(_, (_, at))| at.elapsed() > timeout)
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.steering.remove(&id))
            .map(|(message, _)| Event::FollowUpFailed {
                id: message.id,
                error: "Timed out waiting for the provider to accept this follow-up".into(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn protocol(kind: Kind) -> Protocol {
        let mut p = Protocol::new(Request {
            kind,
            binary: "unused".into(),
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            provider_id: Some("thread".into()),
            model: None,
            settings: Default::default(),
        });
        p.active_turn = Some("turn".into());
        p.follow_up_ready = true;
        p
    }
    #[test]
    fn codex_keeps_messages_until_ack_and_rejection_does_not_end_the_turn() {
        let mut p = protocol(Kind::Codex);
        let message = Message {
            id: "one".into(),
            text: "new direction 🙂".into(),
            ..Default::default()
        };
        let mut out = Vec::new();
        assert!(p.steer(message.clone(), &mut out).unwrap().is_empty());
        let wire: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(wire["method"], "turn/steer");
        assert_eq!(wire["params"]["expectedTurnId"], "turn");
        assert_eq!(wire["params"]["input"][0]["text"], message.text);
        let events = p
            .frame(
                json!({"id":wire["id"],"error":{"message":"turn is closing"}}),
                &mut out,
            )
            .unwrap();
        assert!(
            matches!(&events[0], Event::FollowUpFailed { id, error } if id == "one" && error == "turn is closing")
        );
        assert!(!p.done);
        out.clear();
        p.steer(message.clone(), &mut out).unwrap();
        let retry: Value = serde_json::from_slice(&out).unwrap();
        assert_ne!(retry["id"], wire["id"]);
        assert!(p
            .frame(json!({"id":wire["id"],"result":{}}), &mut out)
            .unwrap()
            .is_empty());
        assert_eq!(
            p.frame(json!({"id":retry["id"],"result":{}}), &mut out)
                .unwrap(),
            vec![Event::FollowUp(message)]
        );
    }
    #[test]
    fn claude_and_pi_use_their_existing_stream_protocols() {
        for kind in [Kind::Claude, Kind::Pi, Kind::Omp] {
            let mut p = protocol(kind);
            let mut out = Vec::new();
            let message = Message {
                id: "two".into(),
                text: "follow up".into(),
                ..Default::default()
            };
            let events = p.steer(message.clone(), &mut out).unwrap();
            let wire: Value = serde_json::from_slice(&out).unwrap();
            if kind == Kind::Claude {
                assert_eq!(wire["type"], "user");
                assert!(events.is_empty(), "sent is not yet taken");
                assert_eq!(
                    p.frame(
                        json!({"type":"user","isReplay":true,"uuid":wire["uuid"],"parent_tool_use_id":null,"message":{"role":"user","content":"follow up"}}),
                        &mut out
                    )
                    .unwrap(),
                    vec![Event::FollowUp(message)]
                );
            } else {
                assert_eq!(wire["type"], "steer");
                assert!(events.is_empty());
                assert_eq!(
                    p.frame(
                        json!({"type":"response","id":wire["id"],"success":true}),
                        &mut out
                    )
                    .unwrap(),
                    vec![Event::FollowUp(message)]
                );
            }
        }
    }

    #[test]
    fn a_claude_steer_that_misses_the_turn_keeps_it_open_for_the_next() {
        let mut p = protocol(Kind::Claude);
        let mut out = Vec::new();
        let message = Message {
            id: "late".into(),
            text: "also say BANANA".into(),
            ..Default::default()
        };
        p.steer(message.clone(), &mut out).unwrap();
        let wire: Value = serde_json::from_slice(&out).unwrap();
        let ended = p
            .frame(json!({"type":"result","result":"one two three"}), &mut out)
            .unwrap();
        assert!(!ended.contains(&Event::Done) && !p.done);
        assert!(p.holds_steers());

        // Claude starts a turn of its own for the message, as it does live.
        p.frame(json!({"type":"system","subtype":"init"}), &mut out)
            .unwrap();
        let taken = p
            .frame(
                json!({"type":"user","isReplay":true,"uuid":wire["uuid"],"parent_tool_use_id":null,"message":{"role":"user","content":"also say BANANA"}}),
                &mut out,
            )
            .unwrap();
        assert_eq!(taken, vec![Event::FollowUp(message)]);
        assert!(!p.holds_steers());
        let ended = p
            .frame(json!({"type":"result","result":"BANANA"}), &mut out)
            .unwrap();
        assert!(ended.contains(&Event::Done));
    }
    #[test]
    fn an_untaken_claude_steer_is_handed_back_and_retires_the_process() {
        let mut p = protocol(Kind::Claude);
        let mut out = Vec::new();
        p.steer(
            Message {
                id: "lost".into(),
                text: "hello?".into(),
                ..Default::default()
            },
            &mut out,
        )
        .unwrap();
        p.frame(json!({"type":"result","result":"done"}), &mut out)
            .unwrap();
        p.claude_steer_deadline = Some(Instant::now() - Duration::from_secs(1));
        let events = p.lifecycle_tick(Instant::now(), &mut out).unwrap();
        assert!(matches!(&events[0], Event::FollowUpFailed { id, .. } if id == "lost"));
        assert!(events.contains(&Event::Done));
        assert!(p.holds_steers(), "Claude may still run it on this process");
    }

    #[cfg(unix)]
    #[test]
    fn running_process_waits_for_turn_readiness_and_streams_after_steering() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("provider");
        std::fs::write(&binary, r##"#!/usr/bin/env python3
import json, sys

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get('method')
    if method == 'initialize':
        send({'id': frame['id'], 'result': {}})
    elif method == 'thread/start':
        send({'id': frame['id'], 'result': {'thread': {'id': 'thread'}}})
    elif method == 'turn/start':
        send({'id': frame['id'], 'result': {'turn': {'id': 'turn'}}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'before', 'delta': 'Before'}})
    elif method == 'turn/steer':
        send({'id': frame['id'], 'result': {}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'after', 'delta': 'After'}})
        send({'method': 'turn/completed', 'params': {'turn': {'status': 'completed'}}})
"##).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (controls, receive) = mpsc::channel();
        let mut delivered = false;
        let mut output = String::new();
        run_with_controls(
            Request {
                kind: Kind::Codex,
                binary,
                cwd: temp.path().into(),
                prompt: "hello".into(),
                provider_id: None,
                model: None,
                settings: Default::default(),
            },
            Arc::new(AtomicBool::new(false)),
            Some(receive),
            |events| {
                for event in events {
                    match event {
                        Event::Ready => controls
                            .send(Control {
                                id: "test".into(),
                                decision: Decision::Steer(Message {
                                    id: "test".into(),
                                    text: "new direction".into(),
                                    ..Default::default()
                                }),
                            })
                            .unwrap(),
                        Event::FollowUp(message) => {
                            assert_eq!(message.id, "test");
                            delivered = true;
                        }
                        Event::Text(text) => {
                            if text == "After" {
                                assert!(
                                    delivered,
                                    "the accepted user message must precede its response"
                                );
                            }
                            output.push_str(&text);
                        }
                        _ => {}
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(delivered);
        assert_eq!(output, "BeforeAfter");
    }
}
