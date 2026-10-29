use crate::settings::{
    SettingsView,
    widgets::{note, secondary_button},
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use monocode_core::permissions::{Grant, Store, Subject};

impl SettingsView {
    pub(super) fn permissions_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut page = vec![
            div()
                .relative()
                .child(self.reveal_probe("saved-permissions"))
                .child(self.project_picker(cx))
                .into_any_element(),
            note(
                "Requests saved with Always allow in this project apply across its worktrees, chats and restarts. Plan and read-only turns still ask.",
            ),
        ];
        if let Some(error) = &self.grants_error {
            page.push(
                div()
                    .id("saved-permissions-error")
                    .mt_4()
                    .role(gpui::Role::Alert)
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        let project =
            monocode_core::permissions::repository(std::path::Path::new(&self.selected_project));
        let mut grants: Vec<&Grant> = self
            .grants
            .iter()
            .filter(|g| g.key.project == project)
            .collect();
        grants.sort_by(|a, b| {
            (&a.key.provider, &a.key.account, &a.label).cmp(&(
                &b.key.provider,
                &b.key.account,
                &b.label,
            ))
        });
        if grants.is_empty() {
            page.push(note("No saved permissions for this project."));
        }
        let mut list = div().mt_4();
        for (index, grant) in grants.into_iter().enumerate() {
            let key = grant.key.clone();
            let provider = monocode_core::provider::Kind::parse(&key.provider)
                .map_or(key.provider.as_str(), |kind| kind.label());
            list = list.child(
                self.row(
                    None,
                    index == 0,
                    grant.label.clone(),
                    Some(
                        format!(
                            "{} · {provider} · Account {}",
                            kind(&key.subject),
                            key.account
                        )
                        .into(),
                    ),
                    vec![
                        secondary_button(
                            SharedString::from(format!("remove-permission-{index}")),
                            "Remove",
                            false,
                            self.grants_busy,
                        )
                        .debug_selector(move || format!("remove-permission-{index}"))
                        .child("Remove")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.grants_busy {
                                return;
                            }
                            let store = Store::default();
                            let key = key.clone();
                            this.load_grants(
                                move || {
                                    store.remove(&key)?;
                                    store.list()
                                },
                                cx,
                            );
                        }))
                        .into_any_element(),
                    ],
                ),
            );
        }
        page.push(list.into_any_element());
        page
    }

    pub(crate) fn refresh_grants(&mut self, cx: &mut Context<Self>) {
        self.load_grants(|| Store::default().list(), cx);
    }

    fn load_grants(
        &mut self,
        work: impl FnOnce() -> Result<Vec<Grant>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.grants_busy = true;
        let work = cx.background_executor().spawn(async move { work() });
        self.grants_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.grants_busy = false;
                match result {
                    Ok(grants) => {
                        this.grants = grants;
                        this.grants_error = None;
                    }
                    Err(error) => this.grants_error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn kind(subject: &Subject) -> &'static str {
    match subject {
        Subject::McpConfirmation { .. } => "MCP confirmation",
        Subject::McpTool { .. } => "MCP tool",
        Subject::Command { .. } => "Command",
        Subject::Edit { .. } => "File edits",
        Subject::Fetch { .. } => "Web fetch",
        Subject::Request { .. } => "Exact request",
    }
}
