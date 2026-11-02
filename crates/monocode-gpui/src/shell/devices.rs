//! The Device tab from the shell's side: opening it, and the `devices.*`
//! actions agents reach through the MonoCode MCP server.
use super::*;
use crate::right_panel::RightTab;
use monocode_core::{
    device::{self, Device, Platform, hub, toolchain},
    mono::bridge::Call,
};
use serde_json::{Value, json};

/// Screenshots kept for agents to read back; older ones are deleted.
const KEPT_SCREENSHOTS: usize = 20;

impl Shell {
    /// Shows the Device tab, opening the right panel for it. With devices
    /// off, opens Settings › Devices instead.
    pub(super) fn open_device_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.devices_in_panel() {
            self.open_settings(crate::settings::Section::Devices, window, cx);
            return;
        }
        self.ensure_right_panel(window, cx);
        let Some(panel) = self.right_panel.clone() else {
            return;
        };
        panel.update(cx, |panel, cx| panel.select(RightTab::Device, cx));
        self.workspace.right_panel_width = self.workspace.right_panel_width.max(380.);
        if self.workspace.right_panel_open {
            self.sync_right_panel(cx);
        } else {
            self.set_right_panel_open(true, window, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }

    pub(super) fn toggle_device_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let showing = self.workspace.right_panel_open
            && self
                .right_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).tab() == RightTab::Device);
        if showing {
            self.set_right_panel_open(false, window, cx);
        } else {
            self.open_device_panel(window, cx);
        }
    }

    /// The Device tab's screenshots and recordings go into the message being
    /// written in the conversation on screen.
    pub(super) fn attach_to_visible_message(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chat = self
            .visible_session()
            .and_then(|id| self.sessions.get(&id).cloned());
        match chat {
            Some(chat) => chat.update(cx, |chat, cx| chat.add_attachments(paths, cx)),
            None => self.notification_state.file_error.update(cx, |notice, cx| {
                notice.set_message(
                    Some("Open a thread to attach the screenshot to.".into()),
                    cx,
                )
            }),
        }
        cx.notify();
    }

    fn authorize_device_call(&self, call: &Call, cx: &App) -> Result<(), String> {
        device::validate(&call.action, &call.input)?;
        if !cfg!(target_os = "macos") {
            return Err("Devices need the macOS app.".into());
        }
        if !device::agents_enabled() {
            return Err(
                "Agent access to devices is off. The user can turn it on in Settings › Devices."
                    .into(),
            );
        }
        let caller = self
            .sessions
            .get(&call.session)
            .map(|view| view.read(cx))
            .ok_or("This thread is not open in MonoCode.")?;
        if caller.summary.cwd.starts_with("remote://") {
            return Err("Devices are not available on remote projects.".into());
        }
        let read_only = caller.app_read_only
            || caller.settings.plan
            || caller.settings.access == monocode_core::agent::AccessMode::ReadOnly;
        if read_only && !matches!(call.action.as_str(), "devices.list" | "devices.screenshot") {
            return Err(
                "This thread's access policy allows only device_list and device_screenshot.".into(),
            );
        }
        Ok(())
    }

    pub(super) fn handle_device_call(
        &mut self,
        call: Call,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.authorize_device_call(&call, cx) {
            let _ = call.reply.send(Err(error));
            return;
        }
        let query = call.input["device"].as_str().map(str::to_owned);
        let platform = match call.input["platform"].as_str() {
            Some("ios") => Some(Platform::Ios),
            Some("android") => Some(Platform::Android),
            _ => None,
        };
        // The device in the panel, for a screenshot that names none.
        let shown = self
            .right_panel
            .as_ref()
            .and_then(|panel| panel.read(cx).device.read(cx).selected().cloned());
        let session = call.session;
        let action = call.action.clone();
        let setting = call.input["setting"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let value = call.input["value"].as_str().unwrap_or_default().to_owned();
        let work = cx.background_executor().spawn(async move {
            let find = |query: &str| -> Result<Device, String> {
                let list = hub::list()?;
                device::resolve(&list, query, platform)
                    .cloned()
                    .ok_or_else(|| {
                        let names: Vec<String> = list
                            .devices
                            .iter()
                            .filter(|d| d.supported)
                            .map(|d| format!("{} ({})", d.name, d.id))
                            .collect();
                        format!("No device matches {query:?}. Devices: {}", names.join(", "))
                    })
            };
            match action.as_str() {
                "devices.list" => {
                    let list = hub::list()?;
                    Ok((json!(list), None))
                }
                "devices.open" => {
                    let mut device = find(query.as_deref().unwrap_or_default())?;
                    if !device.booted {
                        device.id = hub::boot(&device)?;
                        device.booted = true;
                    }
                    toolchain::install(toolchain::Tool::AgentDevice)?;
                    let launcher = toolchain::agent_launcher()?;
                    let name = device::session_name(session, &device.id);
                    Ok((device::quick_start(&device, &launcher, &name), Some(device)))
                }
                "devices.screenshot" => {
                    let device = match (query.as_deref(), shown) {
                        (Some(query), _) => find(query)?,
                        (None, Some(device)) => device,
                        (None, None) => {
                            return Err(
                                "No device is open. Call device_open first, or name one.".into()
                            );
                        }
                    };
                    if !device.booted {
                        return Err(format!(
                            "{} is not running. Call device_open first.",
                            device.name
                        ));
                    }
                    let png = hub::screenshot(device.platform, &device.id)?;
                    let path = save_screenshot(session, &png)?;
                    Ok((json!({"device": device, "path": path}), None))
                }
                "devices.set" => {
                    let device = match (query.as_deref(), shown) {
                        (Some(query), _) => find(query)?,
                        (None, Some(device)) => device,
                        (None, None) => {
                            return Err(
                                "No device is open. Call device_open first, or name one.".into()
                            );
                        }
                    };
                    if device.platform != Platform::Ios {
                        return Err("device_set changes iOS Simulators only.".into());
                    }
                    if !device.booted {
                        return Err(format!(
                            "{} is not running. Call device_open first.",
                            device.name
                        ));
                    }
                    let applied = match agent_change(&setting, &value)? {
                        AgentChange::Setting(setting, value) => {
                            crate::device::sim::set(&device.id, setting, value)?;
                            value.to_owned()
                        }
                        AgentChange::Place(at) => {
                            crate::device::sim::set_location(&*hub::ensure()?, &device.id, at)?;
                            at.map(|(lat, lng)| format!("{lat},{lng}"))
                                .unwrap_or_else(|| "clear".into())
                        }
                    };
                    Ok((
                        json!({"device": device.id, "setting": setting, "value": applied}),
                        None,
                    ))
                }
                "devices.close" => {
                    let device = find(query.as_deref().unwrap_or_default())?;
                    if device.booted {
                        hub::shutdown(&device)?;
                    }
                    Ok((json!({"device": device.id, "shutDown": true}), None))
                }
                _ => Err("Unknown device action".into()),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result: Result<(Value, Option<Device>), String> = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Ok((_, Some(device))) = &result {
                    this.open_device_panel(window, cx);
                    if let Some(panel) = this.right_panel.clone() {
                        let device = device.clone();
                        panel.update(cx, |panel, cx| {
                            panel.device.update(cx, |d, cx| d.show(device, cx))
                        });
                    }
                }
                if call.action == "devices.set"
                    && let Ok((value, _)) = &result
                    && let Some(panel) = this.right_panel.clone()
                    && let Ok(change) = agent_change(
                        value["setting"].as_str().unwrap_or_default(),
                        value["value"].as_str().unwrap_or_default(),
                    )
                {
                    let id = value["device"].as_str().unwrap_or_default().to_owned();
                    panel.update(cx, |panel, cx| {
                        panel.device.update(cx, |d, cx| {
                            if d.selected().is_some_and(|shown| shown.id == id) {
                                match change {
                                    AgentChange::Setting(setting, value) => {
                                        d.agent_set(Some((setting, value)), None, cx)
                                    }
                                    AgentChange::Place(at) => d.agent_set(
                                        None,
                                        Some(at.map(|(lat, lng)| {
                                            (format!("{lat:.3}, {lng:.3}").into(), lat, lng)
                                        })),
                                        cx,
                                    ),
                                }
                            }
                        })
                    });
                }
                if call.action == "devices.close"
                    && result.is_ok()
                    && let Some(panel) = this.right_panel.clone()
                {
                    panel.update(cx, |panel, cx| {
                        panel.device.update(cx, |d, cx| d.refresh(cx))
                    });
                }
                let _ = call.reply.send(result.map(|(value, _)| value));
            });
        })
        .detach();
    }
}

/// What an agent's `device_set` asks for.
enum AgentChange {
    Setting(crate::device::sim::Setting, &'static str),
    Place(Option<(f64, f64)>),
}

fn agent_change(setting: &str, value: &str) -> Result<AgentChange, String> {
    use crate::device::sim;
    if setting == "location" {
        return if matches!(value.trim(), "clear" | "off" | "none") {
            Ok(AgentChange::Place(None))
        } else {
            sim::parse_coordinates(value)
                .map(|at| AgentChange::Place(Some(at)))
                .ok_or_else(|| "location takes \"latitude,longitude\" or clear".to_string())
        };
    }
    let setting = sim::Setting::from_key(setting).ok_or("Unknown setting")?;
    Ok(AgentChange::Setting(setting, setting.accept(value)?))
}

fn save_screenshot(session: i64, png: &[u8]) -> Result<String, String> {
    let dir = toolchain::root().join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{session}-{}.png", monocode_core::time::now_ms()));
    std::fs::write(&path, png).map_err(|e| e.to_string())?;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect();
        if files.len() > KEPT_SCREENSHOTS {
            files.sort();
            for (_, old) in &files[..files.len() - KEPT_SCREENSHOTS] {
                let _ = std::fs::remove_file(old);
            }
        }
    }
    Ok(path.to_string_lossy().into_owned())
}
