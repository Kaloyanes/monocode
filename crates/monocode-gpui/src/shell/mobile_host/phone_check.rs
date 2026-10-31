//! TEMPORARY, debug builds only: a live check of drafts on a paired phone.
//! With `MONOCODE_PHONE_CHECK=<dir>` the app turns on phone access, opens a
//! thread in `<dir>/project` with the draft `<dir>/page.html`, writes the
//! pairing link to `<dir>/invite.txt`, and keeps `<dir>/draft.json` up to date
//! with the draft's shared record, so a simulator can pair and be driven
//! against it. Run it with `MONOCODE_NATIVE_DATA_DIR` set to a scratch folder.
use super::*;
use monocode_core::visual_bridge::{DraftAction, DraftContent, DraftRequest};

const DRAFT: &str = "draft-phone-check";

impl Shell {
    pub(super) fn phone_check(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = std::env::var_os("MONOCODE_PHONE_CHECK").map(std::path::PathBuf::from)
        else {
            return;
        };
        let Some(host) = self.mobile.host.clone() else {
            return;
        };
        let out = dir.clone();
        let note = move |name: &str, text: String| {
            let _ = std::fs::write(out.join(name), text);
        };
        if let Err(error) = host.configure(true, false, "") {
            note("error.txt", format!("configure: {error}"));
            return;
        }
        let project = dir.join("project");
        let _ = std::fs::create_dir_all(&project);
        let project = project.to_string_lossy().into_owned();
        let summary = match locked(&self.store).and_then(|store| store.create("claude", &project)) {
            Ok(summary) => summary,
            Err(error) => return note("error.txt", format!("create: {error}")),
        };
        let session = summary.id;
        self.workspace.add_project(&project);
        self.workspace.project = project;
        self.add(summary, cx);
        let page = std::fs::read_to_string(dir.join("page.html")).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let (reply, answer) = std::sync::mpsc::channel();
            let opened = this.update_in(cx, |this, window, cx| {
                this.handle_draft_request(
                    DraftRequest {
                        session,
                        draft: None,
                        content: Some(DraftContent::Html(page)),
                        action: DraftAction::Snapshot,
                        width: None,
                        height: None,
                        light: None,
                        title: Some("Pricing card".into()),
                        id: Some(DRAFT.into()),
                        reply,
                    },
                    window,
                    cx,
                );
            });
            if opened.is_err() {
                return;
            }
            match host.invite() {
                Ok(invite) => note("invite.txt", invite.url),
                Err(error) => note("error.txt", format!("invite: {error}")),
            }
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let Ok(record) = this.update(cx, |this, cx| {
                    this.draft_named(session, DRAFT, cx).map(|draft| {
                        let mut record = draft.read(cx).shared().summary();
                        record["answered"] = answer.try_recv().is_ok().into();
                        record
                    })
                }) else {
                    return;
                };
                if let Some(record) = record {
                    note("draft.json", record.to_string());
                }
            }
        })
        .detach();
    }
}
