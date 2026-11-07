//! The Add MCP server dialog: paste anything, see what was understood, and
//! pick every provider and scope it should be added to.
use super::management::{Editor, form_field};
use super::*;
use gpui::{AnyElement, Role, SharedString};
use monocode_core::mcp::{self, ParsedServer};
use std::collections::HashMap;

pub(super) const PROVIDERS: [(&str, &str); 5] = [
    ("claude", "Claude Code"),
    ("claude_desktop", "Claude Desktop"),
    ("codex", "Codex"),
    ("cursor", "Cursor"),
    ("opencode", "OpenCode"),
];

pub(super) fn scopes(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["local", "project", "user"],
        "codex" | "claude_desktop" => &["user"],
        _ => &["project", "user"],
    }
}

fn default_scope(provider: &str) -> &'static str {
    match provider {
        "claude" => "local",
        "codex" | "claude_desktop" => "user",
        _ => "project",
    }
}

fn scope_label(scope: &str) -> &'static str {
    match scope {
        "local" => "Local",
        "project" => "Project",
        _ => "User",
    }
}

fn scope_id(provider: &str) -> &'static str {
    match provider {
        "claude" => "mcp-scope-claude",
        "claude_desktop" => "mcp-scope-claude-desktop",
        "codex" => "mcp-scope-codex",
        "cursor" => "mcp-scope-cursor",
        _ => "mcp-scope-opencode",
    }
}

fn provider_label(provider: &str) -> &'static str {
    PROVIDERS
        .iter()
        .find(|(id, _)| *id == provider)
        .map_or("", |(_, label)| label)
}

/// Which providers are ticked, the scope each one writes to, and the name the
/// form filled in from the pasted text.
pub(super) struct Choice {
    selected: Vec<&'static str>,
    scopes: HashMap<&'static str, &'static str>,
    autofilled: String,
}

impl Choice {
    pub(super) fn new(first: &str) -> Self {
        Self {
            selected: PROVIDERS
                .iter()
                .find(|(id, _)| *id == first)
                .map(|(id, _)| *id)
                .into_iter()
                .collect(),
            scopes: PROVIDERS
                .iter()
                .map(|(id, _)| (*id, default_scope(id)))
                .collect(),
            autofilled: String::new(),
        }
    }
    fn scope(&self, provider: &str) -> &'static str {
        self.scopes
            .get(provider)
            .copied()
            .unwrap_or_else(|| default_scope(provider))
    }
}

fn parsed(editor: &Editor, cx: &App) -> Result<Option<ParsedServer>, String> {
    mcp::parse_input(&editor.body.read(cx).content)
}

/// The ticked providers that can take this server under this name, in list order.
pub(super) fn targets(editor: &Editor, cx: &App) -> Vec<&'static str> {
    let Ok(Some(server)) = parsed(editor, cx) else {
        return Vec::new();
    };
    let name = editor.name.read(cx).content.to_string();
    PROVIDERS
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| editor.mcp.selected.contains(id))
        .filter(|id| {
            mcp::name_issue(id, &name).is_none()
                && mcp::provider_issue(id, &server.server).is_none()
        })
        .collect()
}

/// Footer text: how many ticked providers will be left out.
pub(super) fn skipped(editor: &Editor, cx: &App) -> usize {
    editor.mcp.selected.len() - targets(editor, cx).len()
}

pub(super) fn submit_label(editor: &Editor, cx: &App) -> String {
    if editor.busy {
        return "Adding…".into();
    }
    match targets(editor, cx).as_slice() {
        [] => "Add server".into(),
        [one] => format!("Add to {}", provider_label(one)),
        many => format!("Add to {} providers", many.len()),
    }
}

fn mask(value: &str) -> String {
    if value.chars().count() < 5 {
        "••••".into()
    } else {
        format!("{}••••••", value.chars().take(3).collect::<String>())
    }
}

fn command_line(server: &serde_json::Value) -> String {
    if let Some(url) = server.get("url").and_then(|v| v.as_str()) {
        return url.to_owned();
    }
    let mut words = Vec::new();
    match server.get("command") {
        Some(serde_json::Value::String(command)) => words.push(command.clone()),
        Some(serde_json::Value::Array(parts)) => {
            words.extend(parts.iter().filter_map(|p| p.as_str().map(str::to_owned)))
        }
        _ => {}
    }
    if let Some(args) = server.get("args").and_then(|v| v.as_array()) {
        words.extend(args.iter().filter_map(|a| a.as_str().map(str::to_owned)));
    }
    words.join(" ")
}

fn secrets(server: &serde_json::Value) -> Vec<(String, String)> {
    ["env", "headers"]
        .iter()
        .filter_map(|key| server.get(*key).and_then(|v| v.as_object()))
        .flatten()
        .map(|(k, v)| (k.clone(), mask(v.as_str().unwrap_or_default())))
        .collect()
}

fn has_secrets(server: &serde_json::Value) -> bool {
    !secrets(server).is_empty()
}

impl SettingsView {
    /// Fills the name from what was pasted until the user types their own.
    pub(super) fn mcp_text_changed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.management else {
            return;
        };
        let Ok(Some(server)) = parsed(editor, cx) else {
            return;
        };
        let current = editor.name.read(cx).content.to_string();
        if current.is_empty() || current == editor.mcp.autofilled {
            editor.mcp.autofilled = server.name.clone();
            editor
                .name
                .update(cx, |input, cx| input.set_text(server.name, cx));
        }
    }

    fn toggle_mcp_provider(&mut self, provider: &'static str, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.management {
            if editor.busy {
                return;
            }
            editor.error = None;
            if let Some(at) = editor.mcp.selected.iter().position(|p| *p == provider) {
                editor.mcp.selected.remove(at);
            } else {
                editor.mcp.selected.push(provider);
            }
        }
        cx.notify();
    }

    pub(super) fn submit_mcp(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.management else {
            return;
        };
        if editor.busy {
            return;
        }
        let Ok(Some(parsed)) = parsed(editor, cx) else {
            return;
        };
        let jobs = targets(editor, cx)
            .into_iter()
            .map(|provider| (provider, editor.mcp.scope(provider)))
            .collect::<Vec<_>>();
        if jobs.is_empty() {
            return;
        }
        let name = editor.name.read(cx).content.trim().to_owned();
        editor.busy = true;
        editor.error = None;
        let cwd = self.selected_project.clone();
        let generation = self.management_generation;
        let work = cx.background_executor().spawn(async move {
            jobs.into_iter()
                .map(|(provider, scope)| {
                    let result = mcp::mcp_add_server(
                        cwd.clone(),
                        provider.into(),
                        scope.into(),
                        name.clone(),
                        parsed.server.clone(),
                    );
                    (provider, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let results = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.management_generation == generation {
                    this.mcp_finished(results, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Closes the dialog when every provider took the server. Otherwise it
    /// stays open with only the providers that failed still ticked.
    fn mcp_finished(
        &mut self,
        results: Vec<(&'static str, Result<(), String>)>,
        cx: &mut Context<Self>,
    ) {
        let failed = results
            .iter()
            .filter_map(|(provider, result)| {
                result
                    .as_ref()
                    .err()
                    .map(|e| format!("{}: {e}", provider_label(provider)))
            })
            .collect::<Vec<_>>();
        if failed.is_empty() {
            self.management_finished(Ok(()), cx);
            return;
        }
        if let Some(editor) = &mut self.management {
            editor.mcp.selected.retain(|provider| {
                results
                    .iter()
                    .any(|(p, result)| p == provider && result.is_err())
            });
        }
        if failed.len() < results.len() {
            self.refresh(cx);
        }
        self.management_finished(Err(failed.join("\n")), cx);
    }

    pub(super) fn mcp_rows(
        &mut self,
        name: Entity<Input>,
        body: Entity<Input>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![form_field("Server config", body, 100.)];
        let Some(editor) = &self.management else {
            return rows;
        };
        let server = match parsed(editor, cx) {
            Err(error) => {
                rows.push(
                    div()
                        .id("mcp-config-error")
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .text_color(crate::modal::danger(1.))
                        .child(error)
                        .into_any_element(),
                );
                return rows;
            }
            Ok(None) => return rows,
            Ok(Some(server)) => server,
        };
        let name_text = editor.name.read(cx).content.to_string();
        let selected = editor.mcp.selected.clone();
        let chosen = PROVIDERS
            .iter()
            .map(|(id, _)| (*id, editor.mcp.scope(id)))
            .collect::<Vec<_>>();
        let cwd = self.selected_project.clone();
        let remote = server.server.get("url").is_some();
        let secrets = secrets(&server.server);
        let mono = crate::fonts::mono();

        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.03))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(5.))
                            .bg(if remote {
                                gpui::rgba(0x38bdf829)
                            } else {
                                content(0.1).into()
                            })
                            .text_size(px(11.))
                            .text_color(if remote {
                                gpui::rgb(0x38bdf8).into()
                            } else {
                                content(0.6)
                            })
                            .child(if remote { "HTTP" } else { "stdio" }),
                    )
                    .child(div().flex_1().child(form_field("", name, 32.)))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(format!("from {}", server.source)),
                    ),
            );
        if name_text.trim().is_empty() {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(crate::modal::danger(1.))
                    .child("Give it a name."),
            );
        }
        card = card.child(
            div()
                .font_family(mono)
                .text_size(px(12.))
                .text_color(content(0.85))
                .child(command_line(&server.server)),
        );
        for (key, value) in &secrets {
            card = card.child(
                div()
                    .font_family(mono)
                    .text_size(px(11.))
                    .text_color(content(0.55))
                    .child(format!("{key} = {value}")),
            );
        }
        rows.push(card.into_any_element());

        rows.push(
            div()
                .flex()
                .justify_between()
                .items_baseline()
                .text_size(px(12.))
                .text_color(content(0.65))
                .child("Add to")
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.45))
                        .child(format!("{} selected", selected.len())),
                )
                .into_any_element(),
        );
        let mut list = div()
            .flex()
            .flex_col()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.03))
            .overflow_hidden();
        for (ix, (provider, label)) in PROVIDERS.iter().copied().enumerate() {
            let on = selected.contains(&provider);
            let scope = chosen
                .iter()
                .find(|(id, _)| *id == provider)
                .map_or("user", |(_, scope)| scope);
            let issue = mcp::name_issue(provider, &name_text)
                .filter(|_| !name_text.trim().is_empty())
                .or_else(|| mcp::provider_issue(provider, &server.server));
            let blocked = on && (issue.is_some() || name_text.trim().is_empty());
            let mut row = div()
                .flex()
                .flex_col()
                .when(ix > 0, |d| d.border_t_1().border_color(content(0.05)))
                .child(
                    div()
                        .id(SharedString::from(format!("mcp-target-{provider}")))
                        .debug_selector(move || format!("mcp-target-{provider}"))
                        .role(Role::CheckBox)
                        .aria_label(label)
                        .aria_toggled(if on {
                            gpui::Toggled::True
                        } else {
                            gpui::Toggled::False
                        })
                        .flex()
                        .items_center()
                        .gap_3()
                        .h(px(40.))
                        .px_3()
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.03)))
                        .child(
                            div()
                                .size(px(16.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .border_1()
                                .border_color(if on { accent_color() } else { content(0.2) })
                                .when(on, |d| {
                                    d.bg(accent_color())
                                        .child(icon("check", 12.).text_color(gpui::rgb(0x06121f)))
                                }),
                        )
                        .child(provider_icon(
                            if provider == "claude_desktop" {
                                "claude"
                            } else {
                                provider
                            },
                            16.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(13.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(label),
                        )
                        .when(on, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(if blocked {
                                        crate::modal::danger(1.)
                                    } else {
                                        gpui::rgb(0x34d399).into()
                                    })
                                    .child(if blocked { "Skipped" } else { "Ready" }),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_mcp_provider(provider, cx)
                        })),
                );
            if on {
                let options = scopes(provider);
                let path = mcp::destination(provider, scope, &cwd);
                let mut detail = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl(px(54.))
                    .pr_3()
                    .pb_3()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .child(if options.len() > 1 {
                                self.segmented(
                                    scope_id(provider),
                                    "Scope",
                                    scope,
                                    &options
                                        .iter()
                                        .map(|s| (*s, scope_label(s)))
                                        .collect::<Vec<_>>(),
                                    cx,
                                    move |this, value, cx| {
                                        if let Some(e) = &mut this.management {
                                            e.mcp.scopes.insert(provider, value);
                                        }
                                        cx.notify();
                                    },
                                )
                            } else {
                                div()
                                    .px(px(10.))
                                    .py_1()
                                    .rounded(px(5.))
                                    .bg(content(0.06))
                                    .text_size(px(11.))
                                    .text_color(content(0.6))
                                    .child(format!("{} only", scope_label(options[0])))
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(120.))
                                    .truncate()
                                    .font_family(mono)
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .child(path),
                            ),
                    );
                if let Some(issue) = issue {
                    detail = detail.child(
                        div()
                            .text_size(px(11.))
                            .text_color(crate::modal::danger(1.))
                            .child(format!("{issue} Skipped.")),
                    );
                }
                if scope == "project" && provider != "codex" && has_secrets(&server.server) {
                    detail = detail.child(
                        div()
                            .text_size(px(11.))
                            .text_color(gpui::rgb(0xf59e0b))
                            .child("Project scope is usually committed to git. Keep secrets in User scope."),
                    );
                }
                row = row.child(detail);
            }
            list = list.child(row);
        }
        rows.push(list.into_any_element());
        rows
    }
}
