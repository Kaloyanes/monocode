//! A paired phone opens a conversation's drafts. It draws its own copy of the
//! page and changes the same record the desktop's card does, so the notes, the
//! page's state and who is driving are one thing on either screen.
use super::*;
use crate::draft::Draft;
use monocode_core::visual_bridge::{DraftAnchor, DraftContent, DraftFeedback};

/// How long a phone's read waits for the draft to change before answering.
const READ_WAIT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(150);
/// What one phone report may carry.
const MAX_NOTES: usize = 10;
const MAX_ACTIONS: usize = 20;
/// The widest picture of a dev-server draft a phone gets: a phone's width at
/// three pixels a point.
const FRAME_WIDTH: u32 = 1290;

impl Shell {
    /// `draft.read`: the draft's record, and its page when the phone does not
    /// have this version. With `since`, it answers once the record moves past
    /// that revision, or after a while with the same one.
    pub(super) fn mobile_draft_read(
        &mut self,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = match self.mobile_draft(&request.params, window, cx) {
            Ok(Some(draft)) => draft,
            Ok(None) => return request.reply(Ok(json!({"live": false}))),
            Err(error) => return request.reply(Err(error)),
        };
        let since = request.params["since"].as_u64();
        let known = request.params["generation"].as_u64();
        let light = request.params["light"].as_bool().unwrap_or(false);
        let weak = draft.downgrade();
        cx.spawn(async move |_, cx| {
            let started = std::time::Instant::now();
            loop {
                let Ok(revision) = weak.read_with(cx, |draft, _| draft.shared().revision()) else {
                    return request.reply(Ok(json!({"live": false})));
                };
                if since != Some(revision) || started.elapsed() >= READ_WAIT {
                    break;
                }
                cx.background_executor().timer(READ_POLL).await;
            }
            let reply = weak
                .read_with(cx, |draft, _| read(draft, known, light))
                .unwrap_or_else(|_| json!({"live": false}));
            request.reply(Ok(reply));
        })
        .detach();
    }

    /// `draft.frame`: a picture of a dev-server draft, which the phone cannot
    /// load from this Mac's localhost. The phone asks again while it shows it;
    /// each ask leases the page, so it runs with its card out of sight.
    pub(super) fn mobile_draft_frame(
        &mut self,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = match self.mobile_draft(&request.params, window, cx) {
            Ok(Some(draft)) => draft,
            Ok(None) => return request.reply(Ok(json!({"live": false}))),
            Err(error) => return request.reply(Err(error)),
        };
        if draft.read(cx).is_html() {
            return request.reply(Err("The phone draws this draft's page itself".into()));
        }
        let width = request.params["width"]
            .as_u64()
            .map_or(FRAME_WIDTH, |width| (width as u32).clamp(320, FRAME_WIDTH));
        #[cfg_attr(not(target_os = "macos"), allow(clippy::let_unit_value))]
        let lease = draft.update(cx, |draft, cx| draft.lease(window, cx));
        let weak = draft.downgrade();
        cx.spawn(async move |_, cx| {
            let result = Draft::frame(&weak, width, cx).await;
            drop(lease);
            request.reply(result.map(|(jpeg, (width, height))| {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(jpeg);
                json!({
                    "live": true,
                    "frame": format!("data:image/jpeg;base64,{data}"),
                    "width": width,
                    "height": height,
                })
            }));
        })
        .detach();
    }

    /// `draft.action`: the person did something in the phone's copy.
    pub(super) fn mobile_draft_action(
        &mut self,
        params: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let draft = self
            .mobile_draft(params, window, cx)?
            .ok_or("That draft was closed")?;
        let action = params["action"].as_str().unwrap_or_default();
        draft.update(cx, |draft, cx| -> Result<(), String> {
            match action {
                "pin" => {
                    let notes: Vec<DraftFeedback> = serde_json::from_value(params["notes"].clone())
                        .map_err(|_| "Invalid notes")?;
                    let notes = notes.into_iter().take(MAX_NOTES).map(clean).collect();
                    draft.pin_from_phone(notes, cx);
                }
                "send" => draft.send(cx),
                "input" => {
                    let actions = params["actions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .take(MAX_ACTIONS)
                        .map(|action| clip(action, 100))
                        .collect();
                    draft.touch_from_phone(actions, cx);
                }
                "state" => {
                    let values = params["values"]
                        .as_object()
                        .ok_or("values must be an object")?;
                    draft.patch_from_phone(values, cx)?;
                }
                "message" => {
                    let text = params["text"].as_str().unwrap_or_default().trim();
                    if !text.is_empty() {
                        draft.tell_from_phone(clip(text, 2000), cx);
                    }
                }
                "handBack" => draft.hand_back(cx),
                _ => return Err("Unsupported draft action".into()),
            }
            Ok(())
        })?;
        let known = params["generation"].as_u64();
        let light = params["light"].as_bool().unwrap_or(false);
        Ok(read(draft.read(cx), known, light))
    }

    /// The draft a phone names: a live one in the conversation, or a kept
    /// one's page, which opens live for both screens.
    fn mobile_draft(
        &mut self,
        params: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Option<Entity<Draft>>, String> {
        let conversation = string(params, "conversationId").unwrap_or_default();
        let Target::Session(session) = self.mobile_target(&conversation, cx)? else {
            return Err("A Mono's conversation has no drafts".into());
        };
        let id = string(params, "draft").ok_or("Missing draft")?;
        let Some(name) = id.strip_prefix(turns::KEPT_DRAFT) else {
            // A saved draft opens again for both screens, as its card does.
            return Ok(self.reopen_draft(session, &id, window, cx));
        };
        let page = locked(&self.store)?
            .thread_image_page(session, name)?
            .ok_or("That draft's page is no longer available")?;
        self.open_kept_draft_in(session, &page, window, cx)
            .map(Some)
            .map_err(Into::into)
    }
}

fn read(draft: &Draft, known: Option<u64>, light: bool) -> Value {
    let shared = draft.shared();
    let mut reply = shared.summary();
    reply["live"] = true.into();
    reply["title"] = draft.title().to_string().into();
    if let DraftContent::Url(url) = shared.content() {
        // Shown, not loaded: the phone gets pictures of it with `draft.frame`.
        reply["url"] = url.as_str().into();
    } else if known != Some(shared.generation()) {
        reply["page"] = draft.phone_page(light).into();
    }
    reply
}

fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// A note from the phone, held to what the desktop's page would send.
fn clean(note: DraftFeedback) -> DraftFeedback {
    let place = |value: f64| (value.clamp(0., 100.) * 10.).round() / 10.;
    DraftFeedback {
        note: clip(note.note.trim(), 600),
        tag: clip(&note.tag, 20),
        name: clip(&note.name, 60),
        x: place(note.x),
        y: place(note.y),
        at: note.at.and_then(DraftAnchor::checked),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monocode_core::visual_bridge::{DraftAction, DraftRequest};

    #[gpui::test]
    fn a_dev_server_draft_gives_the_phone_its_address_not_a_page(cx: &mut gpui::TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                cx.set_reduce_motion(true);
                let open = shell.visible_session().expect("a conversation is open");
                let (reply, _answer) = std::sync::mpsc::channel();
                shell.handle_draft_request(
                    DraftRequest {
                        session: open,
                        draft: None,
                        content: Some(DraftContent::Url("http://localhost:5173/".into())),
                        action: DraftAction::Snapshot,
                        width: None,
                        height: None,
                        light: None,
                        title: Some("App".into()),
                        id: Some("draft-1-server".into()),
                        reply,
                    },
                    window,
                    cx,
                );
                let params = json!({
                    "conversationId": shell.mobile_session_id(open),
                    "draft": "draft-1-server",
                    "action": "input",
                });
                let read = shell.mobile_draft_action(&params, window, cx).unwrap();
                assert_eq!(read["kind"], "url");
                assert_eq!(read["url"], "http://localhost:5173/");
                // The phone cannot load the desktop's localhost.
                assert!(read.get("page").is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_phone_changes_the_same_draft_the_desktop_card_shows(cx: &mut gpui::TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                cx.set_reduce_motion(true);
                shell.workspace.classic_layout = false;
                shell.layout_changed(cx);
                let open = shell.visible_session().expect("a conversation is open");
                let (reply, _answer) = std::sync::mpsc::channel();
                shell.handle_draft_request(
                    DraftRequest {
                        session: open,
                        draft: None,
                        content: Some(DraftContent::Html("<p>Hi</p>".into())),
                        action: DraftAction::Snapshot,
                        width: None,
                        height: None,
                        light: None,
                        title: Some("Pricing".into()),
                        id: Some("draft-1-phone".into()),
                        reply,
                    },
                    window,
                    cx,
                );
                let conversation = shell.mobile_session_id(open);
                let params = |more: Value| {
                    let mut params = json!({"conversationId": conversation, "draft": "draft-1-phone"});
                    params.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
                    params
                };

                // The phone gets the page in the same sandbox, for its own width.
                let first = shell
                    .mobile_draft_action(&params(json!({"action": "input", "actions": ["clicked \"Go\""]})), window, cx)
                    .unwrap();
                assert_eq!(first["live"], true);
                assert_eq!(first["title"], "Pricing");
                assert!(first["page"].as_str().unwrap().contains("ReactNativeWebView"));
                // Using the page there gives the person control here too.
                assert_eq!(first["control"], true);
                let draft = shell.draft_named(open, "draft-1-phone", cx).unwrap();
                assert!(draft.read(cx).shared().control());

                // A phone that has this version is not sent the page again.
                let generation = first["generation"].clone();
                let at = json!({"path": "0.1", "ax": 0.5, "ay": 0.25});
                let note = json!({"note": "Bigger", "tag": "button", "name": "Go", "x": 10, "y": 20, "at": at});
                let pinned = shell
                    .mobile_draft_action(
                        &params(json!({"action": "pin", "notes": [note], "generation": generation})),
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(pinned.get("page").is_none());
                assert_eq!(pinned["pending"][0]["note"], "Bigger");
                assert_eq!(pinned["pins"][0]["side"], "phone");
                // The pin keeps the element it was made on; the agent's note does not.
                assert_eq!(pinned["pins"][0]["at"], at);
                assert!(pinned["pending"][0].get("at").is_none());
                assert_eq!(draft.read(cx).shared().pending().len(), 1);

                // The page has no state yet, so there is none to change.
                assert!(shell
                    .mobile_draft_action(&params(json!({"action": "state", "values": {"a": 1}})), window, cx)
                    .is_err());
                assert!(shell
                    .mobile_draft_action(&params(json!({"action": "explode"})), window, cx)
                    .is_err());

                shell
                    .mobile_draft_action(&params(json!({"action": "send"})), window, cx)
                    .unwrap();
                let back = shell
                    .mobile_draft_action(&params(json!({"action": "handBack"})), window, cx)
                    .unwrap();
                assert_eq!(back["control"], false);
                assert_eq!(back["pending"], json!([]));
                let outbox = draft.update(cx, |draft, _| draft.take_outbox());
                assert_eq!(outbox.notes.len(), 1);
                assert_eq!(outbox.actions, ["clicked \"Go\""]);

                // A draft that is gone says so; a kept page outside the thread is refused.
                let gone = json!({"conversationId": conversation, "draft": "draft-0-0", "action": "send"});
                assert!(shell.mobile_draft_action(&gone, window, cx).is_err());
                let outside = json!({"conversationId": conversation, "draft": "kept:../x.html", "action": "send"});
                assert!(shell.mobile_draft_action(&outside, window, cx).is_err());
            })
            .unwrap();
    }

    #[test]
    fn a_phones_note_is_held_to_what_a_page_would_send() {
        let note = clean(DraftFeedback {
            note: format!("  {}  ", "n".repeat(700)),
            tag: "button".repeat(10),
            name: "Go".into(),
            x: 140.,
            y: 33.333,
            at: Some(DraftAnchor { path: "0.1;alert(1)".into(), ax: 0.5, ay: 0.5 }),
        });
        assert_eq!(note.note.len(), 600);
        assert_eq!(note.tag.len(), 20);
        assert_eq!((note.x, note.y), (100., 33.3));
        // A path the page could not have sent is dropped; the pin falls back
        // to its percentages.
        assert_eq!(note.at, None);
    }
}
