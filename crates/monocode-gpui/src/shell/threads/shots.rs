use super::*;
use gpui::{HeadlessAppContext, size};
use monocode_core::{
    agent::Settings,
    store::Store,
    tool::{ToolCall, ToolState},
    transcript::Row,
};

fn row(role: &str, text: impl Into<String>) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    }
}
fn tool(name: &str, input: Value, output: Value) -> Row {
    let mut call = ToolCall::new(name, &format!("monocode:{name}"));
    call.input = input.to_string();
    call.output = output.to_string();
    call.state = ToolState::Completed;
    row("tool", serde_json::to_string(&call).unwrap())
}

#[test]
#[ignore = "Requires Metal; writes child-thread UI screenshots using an in-memory fixture"]
fn thread_ui_shots() {
    let directory = std::env::var("THREAD_UI_SHOTS")
        .unwrap_or_else(|_| "/private/tmp/monocode-thread-shots".into());
    std::fs::create_dir_all(&directory).unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut lead = store.create("claude", "/thread-ui-fixture").unwrap();
    let mut child = store.create("codex", "/thread-ui-fixture").unwrap();
    lead.title = "Review live thread ownership".into();
    child.title = "Independent ownership review".into();
    for session in [&mut lead, &mut child] {
        store.title(session.id, &session.title).unwrap();
        session.has_messages = true;
    }
    let reply = "The child is owned by its lead. Cross-thread sends are rejected, and the composer remains locked after the lead finishes.";
    store.write_rows(lead.id,0,&[
        row("user","Start a Codex thread to review ownership, then debate its findings."),
        tool("thread_start",json!({"prompt":"Review child ownership","title":child.title,"harness":"codex"}),json!({"threadId":child.id,"harness":"codex","model":"gpt-6.1"})),
        tool("thread_wait",json!({"threadId":child.id}),json!({"threadId":child.id,"status":"idle","reply":reply})),
        tool("thread_send",json!({"threadId":child.id,"prompt":"Does the composer stay locked once I finish?"}),json!({"threadId":child.id,"accepted":true})),
        row("assistant","The ownership checks hold, and the child stays read-only after this turn."),
    ]).unwrap();
    store.write_rows(child.id,0,&[
        row("user","Review child ownership"),row("reasoning","Checking the bridge authorization and alternate send routes."),
        row("assistant",reply),row("user","Does the composer stay locked once I finish?"),row("assistant","Yes. The persisted parent marker keeps the composer locked after completion and reopening."),
    ]).unwrap();
    store.save_started_link(child.id, lead.id).unwrap();
    store.save_thread_parent(child.id, lead.id).unwrap();
    store
        .save_agent_settings(
            lead.id,
            &Settings {
                model: Some("claude-opus-5".into()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .save_agent_settings(
            child.id,
            &Settings {
                model: Some("gpt-6.1".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut workspace = Workspace::default();
    workspace.add_project(&lead.cwd);
    workspace.open(lead.id);
    let saved = serde_json::to_string(&workspace).unwrap();
    let lead_id = lead.id;
    let child_id = child.id;
    let store = Arc::new(Mutex::new(store));
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::popover::bind(cx);
    });
    let window = cx
        .open_window(size(px(1280.), px(820.)), move |_, cx| {
            cx.new(|cx| {
                Shell::new(
                    store,
                    vec![lead, child],
                    "/thread-ui-fixture".into(),
                    Some(saved),
                    cx,
                )
            })
        })
        .unwrap();
    window
        .update(&mut cx, |shell, _, cx| {
            shell
                .ensure_session(child_id, cx)
                .unwrap()
                .update(cx, |v, cx| v.load(cx));
            shell.thread_session_changed(child_id, cx);
            let transcript = shell.sessions[&lead_id].read(cx).transcript.clone();
            transcript.update(cx, |t, cx| t.toggle_thread(child_id, 1, cx));
        })
        .unwrap();
    let shot = |cx: &mut HeadlessAppContext, name: &str| {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.advance_clock(Duration::from_millis(400));
            cx.update_window(window.into(), |_, w, _| w.refresh())
                .unwrap();
        }
        window
            .update(cx, |shell, _, cx| {
                shell
                    .notification_state
                    .harness
                    .update(cx, |h, cx| h.clear(cx))
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, _| w.refresh())
            .unwrap();
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(std::path::Path::new(&directory).join(format!("{name}.png")))
            .unwrap();
    };
    shot(&mut cx, "lead");
    window
        .update(&mut cx, |shell, window, cx| {
            shell.open(child_id, window, cx)
        })
        .unwrap();
    shot(&mut cx, "child-finished");
}
