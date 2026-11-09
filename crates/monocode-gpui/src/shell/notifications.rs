//! Hosts the same notification stack as App.tsx, and routes native clicks.
use super::*;
use crate::notifications::{
    app_update::{self, AppUpdateEvent, InstalledUpdateNotice, WhatsNew, WhatsNewEvent},
    approvals::{self, ApprovalNotices, SessionInputs},
    delivery::{self, Permission, Target},
    errors,
    file_error::{FileActionError, FileActionErrorEvent},
    github_activity::{self, GithubActivity, GithubActivityEvent},
    harness_updates::{HarnessUpdates, HarnessUpdatesEvent},
    layout_notice::{LayoutNotice, LayoutNoticeEvent},
    linked_activity::{Cleanup, LinkedActivity, LinkedActivityEvent},
    platform, reminders,
    surface::{self, ToastSlot},
    undo_notice::{UndoNotice, UndoNoticeEvent},
    updater::{self, Event as UpdaterEvent, Updater},
};
use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

const APP_ERROR: &str = "app-error";
const MONO_ERROR: &str = "mono-error";

/// Approval cards come and go in numbers, so they keep their own exits.
#[derive(Default)]
struct ToastSlots {
    app_error: ToastSlot<errors::Frozen>,
    mono_error: ToastSlot<errors::Frozen>,
    file_error: ToastSlot<FileActionError>,
    undo: ToastSlot<UndoNotice>,
    reminders: ToastSlot<reminders::View>,
    harness: ToastSlot<HarnessUpdates>,
    layout: ToastSlot<LayoutNotice>,
    /// Only one session's GitHub card shows at a time, so they share a slot.
    linked: ToastSlot<LinkedActivity>,
}

pub(super) struct State {
    pub region: AnchorSlot,
    toasts: ToastSlots,
    pub harness: Entity<HarnessUpdates>,
    pub approvals: Entity<ApprovalNotices>,
    pub reminders: Entity<reminders::View>,
    pub layout: Entity<LayoutNotice>,
    pub installed: Entity<InstalledUpdateNotice>,
    pub updater: Entity<Updater>,
    pub file_error: Entity<FileActionError>,
    /// The card offering to undo the last folder archive, and what it undoes.
    pub undo: Entity<UndoNotice>,
    pub(super) undo_threads: Vec<i64>,
    undo_timer: Option<gpui::Task<()>>,
    pub reminder_error: Option<String>,
    pub github: Entity<GithubActivity>,
    linked: HashMap<i64, Entity<LinkedActivity>>,
    linked_cleanup: HashMap<i64, Cleanup>,
    /// When each card first showed. It slides in once, not on every return
    /// to its tab.
    linked_arrived: HashMap<i64, Instant>,
    /// The session whose card showed last.
    linked_shown: Option<i64>,
    linked_seen_ready: bool,
    linked_last_poll: Option<std::time::Instant>,
    linked_revisions: HashMap<String, i64>,
    linked_primed: bool,
    linked_started_at: u64,
    linked_seen_writes: Option<gpui::Task<()>>,
    pub(super) whats_new: Option<(Entity<WhatsNew>, Subscription, FocusHandle)>,
    permission: Permission,
    permission_probe: Option<Receiver<Permission>>,
    announcements: approvals::InputAnnouncements,
    responding: HashSet<i64>,
    sequence: u64,
    badge: usize,
    sounds_enabled: bool,
    sounds_enabled_at: u64,
    poll: Option<gpui::Task<()>>,
    /// How many notification mute and resume times had passed at the last poll.
    rule_times_passed: usize,
}

impl State {
    pub(super) fn new(workspace: &Workspace, store: Database, cx: &mut Context<Shell>) -> Self {
        Self {
            region: AnchorSlot::default(),
            toasts: ToastSlots::default(),
            harness: cx
                .new(|cx| HarnessUpdates::new(workspace.preferences.hidden_providers.clone(), cx)),
            approvals: cx.new(|_| ApprovalNotices::default()),
            reminders: cx.new(|_| reminders::View::default()),
            layout: cx.new(|_| {
                LayoutNotice::new(
                    !workspace.classic_layout,
                    workspace.new_layout_prompt_dismissed,
                )
            }),
            installed: cx.new(|_| InstalledUpdateNotice::new(None)),
            updater: updater::shared(store, cx),
            file_error: cx.new(|_| FileActionError::new()),
            undo: cx.new(|_| UndoNotice::new()),
            undo_threads: Vec::new(),
            undo_timer: None,
            reminder_error: None,
            github: cx.new(|_| GithubActivity::new(HashMap::new())),
            linked: HashMap::new(),
            linked_cleanup: HashMap::new(),
            linked_seen_ready: false,
            linked_last_poll: None,
            linked_revisions: HashMap::new(),
            linked_arrived: HashMap::new(),
            linked_shown: None,
            linked_primed: false,
            linked_started_at: monocode_core::time::now_ms(),
            linked_seen_writes: None,
            whats_new: None,
            permission: Permission::Unsupported,
            permission_probe: None,
            announcements: Default::default(),
            responding: HashSet::new(),
            sequence: 0,
            badge: 0,
            sounds_enabled: workspace.preferences.sounds,
            sounds_enabled_at: 0,
            poll: None,
            rule_times_passed: 0,
        }
    }
}

impl Shell {
    pub(super) fn app_update_footer(&self, cx: &App) -> AnyElement {
        let state = &self.notification_state;
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .when(state.installed.read(cx).is_visible(), |d| {
                d.child(div().mx_2().mt_2().child(state.installed.clone()))
            })
            .when(state.updater.read(cx).actionable(), |d| {
                d.child(
                    div()
                        .mx_2()
                        .mt_2()
                        .child(updater::row(state.updater.clone(), cx)),
                )
            })
            .into_any_element()
    }
    pub(super) fn refresh_notification_permission(&mut self) {
        self.notification_state.permission_probe = Some(platform::permission());
    }

    /// Call from SessionChanged, so a turn beginning and ending between render
    /// frames still produces exactly one completion. Render sync shares the set.
    pub(super) fn notification_session_changed(&mut self, id: i64, cx: &mut Context<Self>) {
        self.process_notification_inputs(cx);
        let Some(view) = self.sessions.get(&id) else {
            return;
        };
        let chat = view.read(cx);
        if chat.responding() {
            self.notification_state.responding.insert(id);
            return;
        }
        let ended = self.notification_state.responding.remove(&id) && !chat.busy;
        if !ended
            || chat.notification_was_cancelled()
            || chat.summary.archived
            || self
                .monos
                .as_ref()
                .is_some_and(|monos| monos.read(cx).is_mono_session(id))
        {
            // Ended with no finish to send, as when stopped.
            if ended {
                self.mobile_turn_ended(id);
            }
            return;
        }
        let summary = chat.summary.clone();
        let body = chat.notification_reply_body(cx);
        self.deliver_session_notice(
            &summary,
            "agentFinished",
            &body,
            Some(crate::sound::Cue::Finished),
            cx,
        );
    }

    pub(super) fn bind_notices(&mut self, cx: &mut Context<Self>) {
        self.bind_linked_activity(cx);
        self.observations.push(cx.subscribe(
            &self.notification_state.updater,
            |this, _, event: &UpdaterEvent, cx| {
                if let UpdaterEvent::Error(error) = event {
                    this.error = Some(error.clone());
                }
                this.invalidate_chrome(cx);
                if let Some(settings) = &this.settings {
                    settings.update(cx, |_, cx| cx.notify());
                }
                cx.notify();
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.harness,
            |this, _, event, cx| {
                if let HarnessUpdatesEvent::Updated(kind) = event {
                    for view in this.notification_chats(cx) {
                        if Kind::from_id(&view.read(cx).summary.provider) == *kind {
                            view.update(cx, |view, cx| {
                                view.refresh_models_after_harness_update(cx)
                            });
                        }
                    }
                    this.sync_settings(cx);
                }
                cx.notify();
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.approvals,
            |this, _, event, cx| match event {
                approvals::Action::OpenSession { session_id } => {
                    if let Ok(id) = session_id.parse() {
                        this.open_notice_target(Target::Session(id), cx);
                    }
                }
                approvals::Action::Decide {
                    session_id,
                    request_id,
                    decision,
                } => {
                    if let Ok(id) = session_id.parse()
                        && let Some(view) = this.notification_chat(id, cx)
                    {
                        view.update(cx, |view, cx| {
                            view.decide_notification_request(request_id, decision.clone(), cx);
                        });
                    }
                    cx.notify();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.layout,
            |this, _, event, cx| {
                if matches!(event, LayoutNoticeEvent::Try) && this.workspace.classic_layout {
                    // Dismisses the notice and picks the default frame.
                    this.toggle_layout(cx);
                } else {
                    this.workspace.new_layout_prompt_dismissed = true;
                    this.persist(cx);
                    cx.notify();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.reminders,
            |this, _, event, cx| match event {
                reminders::Action::Open { session_id, due_at } => this.open_notice_target(
                    Target::Reminder {
                        session_id: *session_id,
                        due_at: *due_at,
                    },
                    cx,
                ),
                reminders::Action::Dismiss { session_id, due_at } => {
                    this.clear_notification_reminder(*session_id, *due_at, cx)
                }
                reminders::Action::Snooze {
                    session_id,
                    expected_due_at,
                    due_at,
                } => this.snooze_notification_reminder(*session_id, *expected_due_at, *due_at, cx),
                reminders::Action::Retry => this.reload_notification_reminders(cx),
                reminders::Action::OpenSettings => {
                    cx.spawn(async move |this, cx| {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.open_settings(crate::settings::Section::General, window, cx)
                        });
                    })
                    .detach();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.installed,
            |this, _, event, cx| match event {
                AppUpdateEvent::Open(version) => this.open_whats_new(version.clone(), cx),
                AppUpdateEvent::Dismiss => {
                    this.notification_state
                        .installed
                        .update(cx, |notice, cx| notice.set_version(None, cx));
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.file_error,
            |this, _, _: &FileActionErrorEvent, cx| {
                this.notification_state
                    .file_error
                    .update(cx, |notice, cx| notice.set_message(None, cx));
                cx.notify();
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.undo,
            |this, _, event: &UndoNoticeEvent, cx| {
                if matches!(event, UndoNoticeEvent::Undo) {
                    this.undo_folder_archive(cx);
                }
                this.dismiss_undo_notice(cx);
            },
        ));
        if let Some(monos) = self.monos.clone() {
            self.observations.push(cx.subscribe(
                &monos,
                |this, _, _: &crate::monos::NotificationChanged, cx| {
                    this.process_notification_inputs(cx);
                    cx.notify()
                },
            ));
            self.observations.push(cx.subscribe(
                &monos,
                |this, _, event: &crate::monos::NotificationPosted, cx| {
                    this.notify_mobile_post(event.session, &event.body, cx)
                },
            ));
            self.observations.push(cx.subscribe(
                &monos,
                |this, _, event: &crate::monos::NotificationFinished, cx| {
                    if !event.was_cancelled && !event.habit && !event.summary.archived {
                        this.deliver_session_notice(
                            &event.summary,
                            "agentFinished",
                            &event.body,
                            Some(crate::sound::Cue::Finished),
                            cx,
                        );
                    }
                },
            ));
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store).and_then(|store| app_update::consume_installed_update(&store))
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Some(version)) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.notification_state
                        .installed
                        .update(cx, |notice, cx| notice.set_version(Some(version), cx));
                    this.invalidate_chrome(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn start_native_notifications(&mut self, cx: &mut Context<Self>) {
        platform::set_dock_badge(0);
        if platform::initialize() {
            self.notification_state.permission = Permission::Prompt;
            self.notification_state.permission_probe = Some(platform::permission());
        }
        self.notification_state.badge = 0;
        self.process_notification_inputs(cx);
        self.notification_state.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |this, cx| {
                        let permission = this
                            .notification_state
                            .permission_probe
                            .as_ref()
                            .and_then(|rx| rx.try_recv().ok());
                        if let Some(permission) = permission {
                            this.notification_state.permission = permission;
                            this.notification_state.permission_probe = None;
                            cx.notify();
                        }
                        for target in platform::drain_clicks() {
                            this.open_notice_target(target, cx);
                        }
                        this.poll_reminders(cx);
                        this.poll_linked_activity(cx);
                        this.poll_presence(cx);
                        // Scheduled mutes expire without a preference change;
                        // redraw when one does, not on every tick.
                        let passed = this
                            .workspace
                            .preferences
                            .notification_times_passed(monocode_core::time::now_ms());
                        if passed != this.notification_state.rule_times_passed {
                            this.notification_state.rule_times_passed = passed;
                            if !this.notification_state.approvals.read(cx).is_empty()
                                || this
                                    .notification_chats(cx)
                                    .iter()
                                    .any(|v| v.read(cx).needs_approval())
                            {
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn notification_chats(&self, cx: &App) -> Vec<Entity<Conversation>> {
        let mut views: Vec<_> = self.sessions.values().cloned().collect();
        if let Some(monos) = &self.monos {
            views.extend(monos.read(cx).all_notification_chats(cx));
        }
        let mut seen = HashSet::new();
        views.retain(|view| seen.insert(view.read(cx).summary.id));
        views.sort_by_key(|view| view.read(cx).summary.id);
        views
    }

    pub(super) fn notification_chat(&self, id: i64, cx: &App) -> Option<Entity<Conversation>> {
        self.sessions
            .get(&id)
            .cloned()
            .or_else(|| self.monos.as_ref()?.read(cx).notification_entity(id, cx))
    }

    /// Advance banners and the Dock badge while an occluded window skips render.
    fn process_notification_inputs(&mut self, cx: &mut Context<Self>) {
        let snapshots: Vec<_> = self
            .notification_chats(cx)
            .iter()
            .map(|view| {
                let chat = view.read(cx);
                SessionInputs {
                    session_id: chat.summary.id.to_string(),
                    title: chat.display_title().to_owned(),
                    provider: chat.summary.provider.clone(),
                    harness: Kind::from_id(&chat.summary.provider).label().to_owned(),
                    pending: chat.pending_notification_inputs(cx),
                    inbox_ask: false,
                    former_worker: self.former_worker(chat.summary.id),
                    conversation_focused: false,
                }
            })
            .collect();
        self.notification_state.approvals.update(cx, |notices, _| {
            notices.track_pending(&snapshots);
        });
        let badge = delivery::dock_badge_count(
            snapshots
                .iter()
                .filter_map(|s| Some((s.session_id.parse().ok()?, !s.pending.is_empty()))),
        );
        if badge != self.notification_state.badge {
            self.notification_state.badge = badge;
            platform::set_dock_badge(badge);
        }
        for (session, input) in self.notification_state.announcements.reconcile(&snapshots) {
            if let Ok(id) = session.session_id.parse()
                && let Some(view) = self.notification_chat(id, cx)
            {
                let summary = view.read(cx).summary.clone();
                let body = if input.kind == approvals::InputKind::Approval {
                    format!("Approve: {}", input.label)
                } else {
                    input.label
                };
                self.deliver_session_notice(
                    &summary,
                    "agentInput",
                    &body,
                    Some(crate::sound::Cue::Input),
                    cx,
                );
            }
        }
    }

    pub(super) fn notice_session_visible(&self, id: i64, cx: &App) -> bool {
        if self.app_view_open() || self.settings.is_some() || self.search_page() {
            return false;
        }
        if self.mono_active(cx) {
            return self
                .monos
                .as_ref()
                .and_then(|m| m.read(cx).active_notification_session(cx))
                == Some(id);
        }
        self.workspace.active_tab().is_some_and(|tab| {
            self.workspace
                .layout(tab)
                .leaf_ids()
                .into_iter()
                .any(|pane| {
                    self.workspace.session_for_pane(pane) == id
                        && !self.workspace.is_detached_pane(pane)
                })
        })
    }

    pub(super) fn sync_notices(&mut self, window: &Window, cx: &mut Context<Self>) {
        let now = monocode_core::time::now_ms();
        // This runs on every Shell frame; borrow the preferences below instead
        // of cloning their JSON maps each time.
        let sounds = self.workspace.preferences.sounds;
        if self.notification_state.sounds_enabled != sounds {
            self.notification_state.sounds_enabled = sounds;
            if sounds {
                self.notification_state.sounds_enabled_at = now;
            }
        }
        let views = self.notification_chats(cx);
        let mut snapshots = Vec::new();
        let mut directories = HashMap::new();
        let mut completed = Vec::new();
        let mut quiet = Vec::new();
        let mut live_ids = HashSet::new();
        for view in views {
            let chat = view.read(cx);
            let id = chat.summary.id;
            live_ids.insert(id);
            if chat.responding() {
                self.notification_state.responding.insert(id);
            } else if self.notification_state.responding.remove(&id) && !chat.busy {
                if !chat.notification_was_cancelled()
                    && !chat.summary.archived
                    && self.sessions.contains_key(&id)
                    && !self
                        .monos
                        .as_ref()
                        .is_some_and(|monos| monos.read(cx).is_mono_session(id))
                {
                    completed.push((chat.summary.clone(), chat.notification_reply_body(cx)));
                } else {
                    // Ended with no finish to send, as when stopped.
                    quiet.push(id);
                }
            }
            directories.insert(id.to_string(), chat.summary.cwd.clone());
            let mono_visible = self.mono_active(cx) && self.notice_session_visible(id, cx);
            snapshots.push(SessionInputs {
                session_id: id.to_string(),
                title: chat.display_title().to_owned(),
                provider: chat.summary.provider.clone(),
                harness: Kind::from_id(&chat.summary.provider).label().to_owned(),
                pending: chat.pending_notification_inputs(cx),
                inbox_ask: false,
                former_worker: self.former_worker(id),
                conversation_focused: self.window_active
                    && (self.workspace.active == Some(id) || mono_visible)
                    && self.notice_session_visible(id, cx)
                    && chat.composer_focused(window, cx),
            });
        }
        self.notification_state
            .responding
            .retain(|id| live_ids.contains(id));
        for id in quiet {
            self.mobile_turn_ended(id);
        }
        let badge = delivery::dock_badge_count(
            snapshots
                .iter()
                .filter_map(|s| Some((s.session_id.parse().ok()?, !s.pending.is_empty()))),
        );
        if self.notification_state.badge != badge {
            self.notification_state.badge = badge;
            platform::set_dock_badge(badge);
        }
        let announcements = self.notification_state.announcements.reconcile(&snapshots);
        for (session, input) in announcements {
            if let Ok(id) = session.session_id.parse()
                && let Some(view) = self.notification_chat(id, cx)
            {
                let summary = view.read(cx).summary.clone();
                self.deliver_session_notice(
                    &summary,
                    "agentInput",
                    &input.label,
                    Some(crate::sound::Cue::Input),
                    cx,
                );
            }
        }
        for (summary, body) in completed {
            self.deliver_session_notice(
                &summary,
                "agentFinished",
                &body,
                Some(crate::sound::Cue::Finished),
                cx,
            );
        }
        let prefs = &self.workspace.preferences;
        self.notification_state.approvals.update(cx, |notices, cx| {
            notices.sync(
                &snapshots,
                |session, at| {
                    let occurred = at
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        .min(u64::MAX as u128) as u64;
                    directories.get(&session.session_id).is_some_and(|cwd| {
                        prefs.allows_project_notification(cwd, "agentInput", Some(occurred), now)
                    })
                },
                cx,
            )
        });
        let due = self
            .reminders
            .iter()
            .filter(|r| r.due_at <= now)
            .filter_map(|r| {
                let summary = self.session_summaries.get(&r.session_id)?;
                prefs
                    .allows_project_notification(&summary.cwd, "reminders", Some(r.due_at), now)
                    .then(|| reminders::Snapshot {
                        session_id: r.session_id,
                        due_at: r.due_at,
                        title: summary.title.clone(),
                        project: project_name(&summary.cwd),
                        time_label: local_reminder_label(r.due_at),
                    })
            })
            .collect();
        let error = self.notification_state.reminder_error.clone();
        self.notification_state.reminders.update(cx, |notices, cx| {
            notices.sync(due, error, prefs.notifications, cx)
        });
        self.notification_state.layout.update(cx, |notice, cx| {
            notice.set_snapshot(
                !self.workspace.classic_layout,
                self.workspace.new_layout_prompt_dismissed,
                cx,
            )
        });
    }

    pub(super) fn deliver_session_notice(
        &mut self,
        summary: &Session,
        category: &str,
        body: &str,
        cue: Option<crate::sound::Cue>,
        cx: &mut Context<Self>,
    ) {
        self.notify_mobile(summary, category, body, cx);
        self.dispatch_notice(
            summary,
            category,
            Target::Session(summary.id),
            body,
            cue,
            cx,
        );
    }

    pub(super) fn deliver_reminder_notice(&mut self, id: i64, due_at: u64, cx: &mut Context<Self>) {
        if let Some(summary) = self.session_summaries.get(&id).cloned() {
            self.dispatch_notice(
                &summary,
                "reminders",
                Target::Reminder {
                    session_id: id,
                    due_at,
                },
                &summary.title,
                None,
                cx,
            );
        }
    }

    fn dispatch_notice(
        &mut self,
        summary: &Session,
        category: &str,
        target: Target,
        body: &str,
        cue: Option<crate::sound::Cue>,
        cx: &mut Context<Self>,
    ) {
        let now = monocode_core::time::now_ms();
        let occurred = match target {
            Target::Session(_) => now,
            Target::Reminder { due_at, .. } => due_at,
        };
        let prefs = &self.workspace.preferences;
        if !prefs.allows_project_notification(&summary.cwd, category, Some(occurred), now) {
            return;
        }
        let notify = delivery::should_notify(
            prefs.notifications,
            self.notification_state.permission,
            self.window_active,
            category != "reminders" && self.notice_session_visible(summary.id, cx),
        );
        if !notify {
            if prefs.sounds
                && occurred >= self.notification_state.sounds_enabled_at
                && let Some(cue) = cue
            {
                crate::sound::play(cue);
            }
            return;
        }
        self.notification_state.sequence = self.notification_state.sequence.wrapping_add(1);
        let body = if body.trim().is_empty() && category == "agentFinished" {
            format!("{} finished", Kind::from_id(&summary.provider).label())
        } else {
            body.to_owned()
        };
        let receiver = platform::show(platform::Notification {
            tag: target.tag(self.notification_state.sequence),
            subtitle: if category == "reminders" {
                "Session reminder".into()
            } else {
                summary.title.clone()
            },
            body: delivery::clip_body(&body),
            sound: prefs.sounds,
        });
        let directory = summary.cwd.clone();
        let category = category.to_owned();
        let work = cx.background_executor().spawn(async move {
            receiver
                .recv_timeout(Duration::from_secs(5))
                .ok()
                .is_some_and(|result| result.is_ok())
        });
        cx.spawn(async move |this, cx| {
            let accepted = work.await;
            let _ = this.update(cx, |this, _| {
                let now = monocode_core::time::now_ms();
                let prefs = &this.workspace.preferences;
                if !accepted
                    && prefs.sounds
                    && occurred >= this.notification_state.sounds_enabled_at
                    && prefs.allows_project_notification(&directory, &category, Some(occurred), now)
                    && let Some(cue) = cue
                {
                    crate::sound::play(cue);
                }
            });
        })
        .detach();
    }

    fn open_notice_target(&mut self, target: Target, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                let id = match target {
                    Target::Session(id) => id,
                    Target::Reminder { session_id, due_at } => {
                        this.clear_notification_reminder(session_id, due_at, cx);
                        session_id
                    }
                };
                this.close_settings(window, cx);
                this.leave_app_views(cx);
                this.leave_app_search(cx);
                if let Some(monos) = this.monos.clone()
                    && monos.update(cx, |m, cx| m.open_notification_session(id, window, cx))
                {
                    window.activate_window();
                    return;
                }
                // Habit runs share their live Conversation with the Mono
                // manager. Adopt that entity so controls answer the running
                // provider, rather than constructing a disconnected history.
                if !this.sessions.contains_key(&id)
                    && let Some(view) = this.notification_chat(id, cx)
                {
                    let summary = view.read(cx).summary.clone();
                    this.session_summaries.insert(id, summary.clone());
                    this.session_marks
                        .insert(id, cx.new(|_| SessionMark::new(summary.provider.clone())));
                    this.tab_marks
                        .insert(id, cx.new(|_| SessionMark::tab(summary.provider.clone())));
                    this.observations.push(cx.subscribe(
                        &view,
                        move |this, _, _: &SessionChanged, cx| {
                            this.refresh_session_mark(id, cx);
                            this.notification_session_changed(id, cx);
                            this.invalidate_chrome(cx);
                            cx.notify();
                        },
                    ));
                    this.sessions.insert(id, view);
                    if !this.order.contains(&id) {
                        this.order.push(id);
                    }
                }
                if this.session_summaries.contains_key(&id) {
                    // `open` normally dismisses today's due reminder. A stale
                    // OS target must preserve a newer occurrence instead.
                    let newer = match target {
                        Target::Reminder { due_at, .. } => this
                            .reminders
                            .iter()
                            .find(|r| r.session_id == id && r.due_at != due_at)
                            .cloned(),
                        _ => None,
                    };
                    if newer.is_some() {
                        this.reminders.retain(|r| r.session_id != id);
                    }
                    this.close_settings(window, cx);
                    this.open(id, window, cx);
                    if let Some(newer) = newer {
                        this.reminders.push(newer);
                    }
                    window.activate_window();
                }
            });
        })
        .detach();
    }

    pub(super) fn clear_notification_reminder(
        &mut self,
        id: i64,
        expected: u64,
        cx: &mut Context<Self>,
    ) {
        let removed = self
            .reminders
            .iter()
            .find(|r| r.session_id == id && r.due_at == expected)
            .cloned();
        self.reminders
            .retain(|r| !(r.session_id == id && r.due_at == expected));
        platform::dismiss(
            &Target::Reminder {
                session_id: id,
                due_at: expected,
            }
            .tag(0),
        );
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move {
                locked(&store).and_then(|s| s.clear_reminders(&[id], Some(expected)))
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Err(error) => {
                        if let Some(removed) = removed
                            && !this.reminders.iter().any(|r| r.session_id == id)
                        {
                            this.reminders.push(removed);
                        }
                        this.notification_state.reminder_error = Some(error);
                    }
                    Ok(()) => this.notification_state.reminder_error = None,
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        }));
        self.invalidate_chrome(cx);
        cx.notify();
    }

    fn snooze_notification_reminder(
        &mut self,
        id: i64,
        expected: u64,
        due_at: u64,
        cx: &mut Context<Self>,
    ) {
        if !self
            .reminders
            .iter()
            .any(|r| r.session_id == id && r.due_at == expected)
        {
            return;
        }
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move {
                locked(&store).and_then(|s| s.snooze_reminder_if(id, expected, due_at))
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    platform::dismiss(
                        &Target::Reminder {
                            session_id: id,
                            due_at: expected,
                        }
                        .tag(0),
                    );
                    this.notification_state.reminder_error = None;
                    this.reload_notification_reminders(cx);
                }
                Err(error) => {
                    this.notification_state.reminder_error = Some(error);
                    cx.notify();
                }
            });
        }));
    }

    fn reload_notification_reminders(&mut self, cx: &mut Context<Self>) {
        let expected_snapshot = self.reminders.clone();
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx
                .background_executor()
                .spawn(async move { locked(&store).and_then(|s| s.reminders()) });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(reminders) => {
                        // Preserve a newer optimistic schedule/cancel while its
                        // ordered write waits behind this reload operation.
                        if this.reminders == expected_snapshot {
                            this.reminders = reminders;
                        }
                        this.notification_state.reminder_error = None;
                        this.poll_reminders(cx);
                    }
                    Err(error) => this.notification_state.reminder_error = Some(error),
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        }));
    }

    pub(super) fn open_whats_new(&mut self, version: String, cx: &mut Context<Self>) {
        let view = cx.new(|_| WhatsNew::new(version.clone()));
        let weak = view.downgrade();
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store).and_then(|store| store.setting("monocode.nativeReleaseNotes"))
        });
        cx.spawn(async move |_, cx| {
            if let Ok(Some(raw)) = work.await
                && let Ok(notes) = serde_json::from_str::<serde_json::Value>(&raw)
                && notes["version"].as_str() == Some(version.as_str())
                && let Some(body) = notes["body"].as_str()
            {
                let _ = weak.update(cx, |view, cx| view.set_release_notes(body, cx));
            }
        })
        .detach();
        let events = cx.subscribe(&view, |this, _, _: &WhatsNewEvent, cx| {
            this.notification_state.whats_new = None;
            cx.notify();
        });
        self.notification_state.whats_new = Some((view, events, cx.focus_handle()));
        cx.notify();
    }

    /// Offers to undo an archive for a few seconds. A newer offer replaces it.
    pub(super) fn offer_undo(
        &mut self,
        message: String,
        threads: Vec<i64>,
        cx: &mut Context<Self>,
    ) {
        const WINDOW: Duration = Duration::from_secs(8);
        let state = &mut self.notification_state;
        state.undo_threads = threads;
        state
            .undo
            .update(cx, |notice, cx| notice.set_message(Some(message), cx));
        state.undo_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WINDOW).await;
            let _ = this.update(cx, |this, cx| this.dismiss_undo_notice(cx));
        }));
        cx.notify();
    }

    pub(super) fn dismiss_undo_notice(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.notification_state;
        state.undo_timer = None;
        state.undo_threads.clear();
        state
            .undo
            .update(cx, |notice, cx| notice.set_message(None, cx));
        cx.notify();
    }

    /// Copies are taken every render because these toasts can clear from
    /// anywhere, and by the next render their content is already gone.
    fn sync_toasts(&mut self, cx: &mut Context<Self>) {
        let frozen = |id, message| errors::Frozen { id, message };
        let app_error = self.error.clone().map(|message| frozen(APP_ERROR, message));
        let mono_error = self
            .monos
            .as_ref()
            .and_then(|monos| monos.read(cx).error.clone())
            .map(|message| frozen(MONO_ERROR, message));
        let state = &self.notification_state;
        let file_error = state.file_error.read(cx);
        let file_error = file_error.is_visible().then(|| file_error.clone());
        let undo = state.undo.read(cx);
        let undo = undo.is_visible().then(|| undo.clone());
        let reminders = state.reminders.read(cx);
        let reminders = (!reminders.is_empty()).then(|| reminders.frozen());
        let harness = state.harness.read(cx);
        let harness = harness.is_visible().then(|| harness.clone());
        let layout = state.layout.read(cx);
        let layout = layout.is_visible().then(|| layout.clone());

        let toasts = &mut self.notification_state.toasts;
        toasts.app_error.sync(
            app_error,
            |s| &mut s.notification_state.toasts.app_error,
            cx,
        );
        toasts.mono_error.sync(
            mono_error,
            |s| &mut s.notification_state.toasts.mono_error,
            cx,
        );
        toasts.file_error.sync(
            file_error,
            |s| &mut s.notification_state.toasts.file_error,
            cx,
        );
        toasts
            .undo
            .sync(undo, |s| &mut s.notification_state.toasts.undo, cx);
        toasts.reminders.sync(
            reminders,
            |s| &mut s.notification_state.toasts.reminders,
            cx,
        );
        toasts
            .harness
            .sync(harness, |s| &mut s.notification_state.toasts.harness, cx);
        toasts
            .layout
            .sync(layout, |s| &mut s.notification_state.toasts.layout, cx);
    }

    pub(super) fn notices_overlay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_toasts(cx);
        let state = &self.notification_state;
        let toasts = &state.toasts;
        let mono_error = self.monos.as_ref().and_then(|monos| {
            monos
                .read(cx)
                .error
                .clone()
                .map(|message| (monos.clone(), message))
        });
        let stack = div()
            .id("notification-stack")
            .absolute()
            .top(px(12.))
            .right(px(12.))
            .w(px(360.))
            .max_w(px((self.viewport_width - 24.).max(0.)))
            .flex()
            .flex_col()
            .items_end()
            .gap(px(surface::TOAST_GAP))
            .child(state.region.probe())
            .when_some(self.error.clone(), |stack, message| {
                stack.child(
                    toasts.app_error.enter(
                        "app-error-enter",
                        div().w_full().child(errors::card(
                            APP_ERROR,
                            message,
                            icon_button("dismiss-error", "Dismiss error", "close")
                                .debug_selector(|| "dismiss-error".into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                })),
                        )),
                    ),
                )
            })
            .children(toasts.app_error.exit("app-error-exit"))
            .when_some(mono_error, |stack, (monos, message)| {
                stack.child(
                    toasts.mono_error.enter(
                        "mono-error-enter",
                        div().w_full().child(errors::card(
                            MONO_ERROR,
                            message,
                            icon_button("mono-error-close", "Dismiss Mono error", "close")
                                .debug_selector(|| "mono-error-close".into())
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    monos.update(cx, |monos, cx| monos.dismiss_error(cx));
                                })),
                        )),
                    ),
                )
            })
            .children(toasts.mono_error.exit("mono-error-exit"))
            .when(state.file_error.read(cx).is_visible(), |stack| {
                stack.child(toasts.file_error.enter(
                    "file-error-enter",
                    div().w_full().child(state.file_error.clone()),
                ))
            })
            .children(toasts.file_error.exit("file-error-exit"))
            .when(state.undo.read(cx).is_visible(), |stack| {
                stack.child(toasts.undo.enter(
                    "undo-notice-enter",
                    div().w_full().child(state.undo.clone()),
                ))
            })
            .children(toasts.undo.exit("undo-notice-exit"))
            .when(!state.reminders.read(cx).is_empty(), |d| {
                d.child(
                    toasts.reminders.enter(
                        "reminders-enter",
                        div()
                            .w(px(320.))
                            .max_w_full()
                            .child(state.reminders.clone()),
                    ),
                )
            })
            .children(toasts.reminders.exit("reminders-exit"))
            .when(state.harness.read(cx).is_visible(), |d| {
                d.child(toasts.harness.enter(
                    "harness-updates-enter",
                    div().w(px(340.)).max_w_full().child(state.harness.clone()),
                ))
            })
            .children(toasts.harness.exit("harness-updates-exit"))
            .when(state.layout.read(cx).is_visible(), |d| {
                d.child(toasts.layout.enter(
                    "layout-notice-enter",
                    div().w(px(340.)).max_w_full().child(state.layout.clone()),
                ))
            })
            .children(toasts.layout.exit("layout-notice-exit"))
            .when(!state.approvals.read(cx).is_empty(), |d| {
                d.child(
                    div()
                        .w(px(360.))
                        .max_w_full()
                        .child(state.approvals.clone()),
                )
            });
        // Toasts remain actionable above dialogs, including Mono settings.
        gpui::deferred(stack).with_priority(110).into_any_element()
    }

    pub(super) fn notice_modals(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements = Vec::new();
        if let Some((view, _, focus)) = &self.notification_state.whats_new {
            let view = view.clone();
            let focus = focus.clone();
            if !focus.contains_focused(window, cx) {
                window.focus(&focus, cx);
            }
            elements.push(
                div()
                    .id("whats-new-overlay")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::modal::scrim("whats-new-scrim"))
                    .track_focus(&focus)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            this.notification_state.whats_new = None;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.notification_state.whats_new = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(view),
                    )
                    .into_any_element(),
            );
        }
        elements
    }
}

impl Shell {
    fn bind_linked_activity(&mut self, cx: &mut Context<Self>) {
        self.observations.push(cx.subscribe(
            &self.notification_state.github,
            |this, _, event, cx| {
                match event {
                    GithubActivityEvent::Acknowledged(seen) => {
                        let seen = seen.clone();
                        let store = this.store.clone();
                        let previous = this.notification_state.linked_seen_writes.take();
                        this.notification_state.linked_seen_writes =
                            Some(cx.spawn(async move |_, cx| {
                                if let Some(previous) = previous {
                                    previous.await;
                                }
                                cx.background_executor()
                                    .spawn(async move {
                                        let _ = locked(&store)
                                            .and_then(|s| github_activity::save_seen(&s, &seen));
                                    })
                                    .await;
                            }));
                    }
                    GithubActivityEvent::Changed => {
                        this.observe_linked_activity(cx);
                        this.sync_linked_activity(cx);
                    }
                }
                cx.notify();
            },
        ));
        let store = self.store.clone();
        let metadata = self.workspace.desktop_preferences_metadata.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)
                .and_then(|s| github_activity::load_seen_with_metadata(&s, metadata.as_ref()))
        });
        cx.spawn(async move |this, cx| {
            let seen = work.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.notification_state
                    .github
                    .update(cx, |feed, _| feed.restore_seen(seen));
                this.notification_state.linked_seen_ready = true;
                this.poll_linked_activity(cx);
            });
        })
        .detach();
    }

    fn poll_linked_activity(&mut self, cx: &mut Context<Self>) {
        // On a hotspot keep-awake joined, the data is better left to agents.
        if self.notification_state.linked_seen_ready
            && !crate::keep_awake::on_hotspot()
            && self
                .notification_state
                .linked_last_poll
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
            && self.notification_state.github.read(cx).ready_to_poll()
        {
            self.notification_state.linked_last_poll = Some(std::time::Instant::now());
            let mut sessions = self.session_summaries.clone();
            for view in self.notification_chats(cx) {
                let summary = view.read(cx).summary.clone();
                sessions.insert(summary.id, summary);
            }
            self.notification_state.github.update(cx, |feed, cx| {
                feed.poll(sessions.into_values().collect(), cx)
            });
        }
        let pending = self.notification_state.linked_cleanup.clone();
        for (id, action) in pending {
            if self.archiving.contains(&id) || self.deleting.contains(&id) {
                continue;
            }
            let success = match action {
                Cleanup::Archive => self.session_summaries.get(&id).is_some_and(|s| s.archived),
                Cleanup::Delete => !self.session_summaries.contains_key(&id),
            };
            self.notification_state.linked_cleanup.remove(&id);
            if let Some(notice) = self.notification_state.linked.get(&id).cloned() {
                notice.update(cx, |notice, cx| notice.finish_cleanup(success, cx));
            }
            if success {
                self.notification_state
                    .github
                    .update(cx, |feed, cx| feed.acknowledge(id, cx));
            }
        }
    }

    fn linked_activity_allowed(&self, card: &github_activity::Card) -> bool {
        let project = format!(
            "repository:github.com/{}",
            card.item.repo.trim().to_lowercase()
        );
        let category = if card.item.kind == "pr" {
            "pullRequests"
        } else {
            "issues"
        };
        let occurred = u64::try_from(card.updated_at).ok();
        occurred.is_some()
            && self.workspace.preferences.allows_notification_project(
                &project,
                category,
                occurred,
                monocode_core::time::now_ms(),
            )
    }

    fn observe_linked_activity(&mut self, cx: &mut Context<Self>) {
        let cards: Vec<_> = self
            .notification_state
            .github
            .read(cx)
            .cards
            .values()
            .cloned()
            .collect();
        self.notification_state
            .linked_arrived
            .retain(|id, _| cards.iter().any(|card| card.session == *id));
        for card in cards {
            let key = format!(
                "{}:{}:{}:{}",
                card.session,
                card.item.repo.to_lowercase(),
                card.item.kind,
                card.item.number
            );
            let previous = self.notification_state.linked_revisions.get(&key).copied();
            self.notification_state
                .linked_revisions
                .insert(key, previous.unwrap_or(0).max(card.updated_at));
            // The first successful feed is a baseline, never a new-activity
            // announcement. Acknowledgment and local-turn cutoffs are applied
            // by the feed before a card enters this collection.
            let newly_changed = self.notification_state.linked_primed
                && previous.is_none_or(|at| card.updated_at > at);
            let occurred = u64::try_from(card.updated_at).unwrap_or(0);
            if newly_changed
                && !card.details_error
                && self.linked_activity_allowed(&card)
                && self.workspace.preferences.sounds
                && occurred >= self.notification_state.sounds_enabled_at
                && occurred >= self.notification_state.linked_started_at
            {
                // React uses a deduplicated chime, without an OS/banner here.
                crate::sound::play(crate::sound::Cue::LinkedActivity);
            }
        }
        self.notification_state.linked_primed = true;
    }

    fn sync_linked_activity(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<_> = self.notification_state.linked.keys().copied().collect();
        for id in ids {
            let card = self
                .notification_state
                .github
                .read(cx)
                .cards
                .get(&id)
                .filter(|card| self.linked_activity_allowed(card))
                .cloned();
            let can_cleanup = self.session_summaries.get(&id).is_some_and(|s| !s.archived);
            if let Some(notice) = self.notification_state.linked.get(&id) {
                notice.update(cx, |notice, cx| {
                    notice.set_snapshot(card, can_cleanup, can_cleanup, cx)
                });
            }
        }
    }

    /// The pane host positions the returned card at top/right 12px, clipped to
    /// the exact session surface, so split chats and Mono sessions stay scoped.
    pub(super) fn linked_notice(&mut self, id: i64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(notice) = self.linked_view(id, cx) else {
            let state = &self.notification_state;
            return (state.linked_shown == Some(id))
                .then(|| state.toasts.linked.exit("linked-activity-exit"))
                .flatten()
                .map(IntoElement::into_any_element);
        };
        let state = &mut self.notification_state;
        state.toasts.linked.stay();
        state.linked_shown = Some(id);
        let arrived = *state.linked_arrived.entry(id).or_insert_with(Instant::now);
        let toast = div().w_full().child(notice);
        let slot = &state.toasts.linked;
        Some(if arrived.elapsed() < surface::TOAST_ENTER {
            slot.enter(("linked-activity-enter", id as u64), toast)
                .into_any_element()
        } else {
            slot.settled(toast).into_any_element()
        })
    }

    /// Runs from the card's own events, because a card also disappears when
    /// its tab closes or changes, and that must not look like a dismissal.
    fn linked_leave(&mut self, id: i64, cx: &mut Context<Self>) {
        let state = &self.notification_state;
        // The close button acknowledges and dismisses, so this runs twice.
        if state.linked_shown != Some(id) || !state.github.read(cx).cards.contains_key(&id) {
            return;
        }
        let Some(copy) = state.linked.get(&id).map(|notice| notice.read(cx).clone()) else {
            return;
        };
        self.notification_state.toasts.linked.leave(
            copy,
            |shell| &mut shell.notification_state.toasts.linked,
            cx,
        );
    }

    fn linked_view(&mut self, id: i64, cx: &mut Context<Self>) -> Option<Entity<LinkedActivity>> {
        let card = self
            .notification_state
            .github
            .read(cx)
            .cards
            .get(&id)
            .filter(|card| self.linked_activity_allowed(card))
            .cloned()?;
        let can_cleanup = self.session_summaries.get(&id).is_some_and(|s| !s.archived);
        if let Some(notice) = self.notification_state.linked.get(&id) {
            notice.update(cx, |notice, cx| {
                notice.set_snapshot(Some(card), can_cleanup, can_cleanup, cx)
            });
            return Some(notice.clone());
        }
        let notice = cx.new(|cx| {
            let mut notice = LinkedActivity::new();
            notice.set_snapshot(Some(card), can_cleanup, can_cleanup, cx);
            notice
        });
        self.observations
            .push(cx.subscribe(&notice, |this, _, event, cx| {
                this.handle_linked_activity(event.clone(), cx)
            }));
        self.notification_state.linked.insert(id, notice.clone());
        Some(notice)
    }

    /// Archives threads the Inbox offered, as the card's button would. An id
    /// the shell does not list is skipped, and so is one already archived,
    /// because the archive toggle would restore it.
    pub(crate) fn archive_linked_threads(&mut self, ids: &[i64], cx: &mut Context<Self>) {
        for &id in ids {
            if self.session_summaries.get(&id).is_some_and(|s| !s.archived) {
                self.handle_linked_activity(LinkedActivityEvent::Archive(id), cx);
            }
        }
    }

    fn open_work_link_later(&mut self, url: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| this.open_work_link(&url, window, cx));
        })
        .detach();
    }

    fn handle_linked_activity(&mut self, event: LinkedActivityEvent, cx: &mut Context<Self>) {
        match event {
            LinkedActivityEvent::Acknowledge(id) | LinkedActivityEvent::Dismiss(id) => {
                self.linked_leave(id, cx);
                self.notification_state
                    .github
                    .update(cx, |feed, cx| feed.acknowledge(id, cx));
            }
            LinkedActivityEvent::OpenUrl(url) => self.open_work_link_later(url, cx),
            LinkedActivityEvent::OpenDiscussion(id) => {
                // A native threaded discussion pane is not implemented yet.
                if let Some(item) = self
                    .session_summaries
                    .get(&id)
                    .and_then(|s| s.work_item.as_ref())
                {
                    let url = item.url.clone();
                    self.open_work_link_later(url, cx);
                }
            }
            LinkedActivityEvent::AddToChat(id, prompt) => {
                cx.spawn(async move |this, cx| {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.close_settings(window, cx);
                        let opened_mono = this.monos.clone().is_some_and(|monos| {
                            monos.update(cx, |m, cx| m.open_notification_session(id, window, cx))
                        });
                        if !opened_mono {
                            this.open(id, window, cx);
                        }
                        if let Some(view) = this.notification_chat(id, cx) {
                            let input = view.read(cx).input.clone();
                            if !input.read(cx).is_composing() {
                                input.update(cx, |input, cx| {
                                    let draft = monocode_core::composer::append_block(
                                        &input.content,
                                        &prompt,
                                    );
                                    input.set_text(draft, cx);
                                });
                                window.focus(&input.focus_handle(cx), cx);
                            }
                        }
                        window.activate_window();
                        cx.notify();
                    });
                })
                .detach();
            }
            LinkedActivityEvent::Archive(id) => {
                self.toggle_archive(id, cx);
                if self.archiving.contains(&id) {
                    self.notification_state
                        .linked_cleanup
                        .insert(id, Cleanup::Archive);
                } else if let Some(notice) = self.notification_state.linked.get(&id) {
                    notice.update(cx, |notice, cx| notice.finish_cleanup(false, cx));
                }
            }
            LinkedActivityEvent::Delete(id) => {
                cx.spawn(async move |this, cx| {
                    let prompt = this.update_in(cx, |_, window, cx| {
                        window.prompt(
                            gpui::PromptLevel::Warning,
                            "Delete this conversation permanently?",
                            Some("Its messages and saved settings will be removed."),
                            &["Delete", "Cancel"],
                            cx,
                        )
                    });
                    let confirmed = match prompt {
                        Ok(prompt) => matches!(prompt.await, Ok(0)),
                        Err(_) => false,
                    };
                    let _ = this.update_in(cx, |this, window, cx| {
                        if confirmed {
                            this.delete_session(id, window, cx);
                        }
                        if this.deleting.contains(&id) {
                            this.notification_state
                                .linked_cleanup
                                .insert(id, Cleanup::Delete);
                        } else if let Some(notice) = this.notification_state.linked.get(&id) {
                            notice.update(cx, |notice, cx| notice.finish_cleanup(false, cx));
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn error_cards_share_top_right_stack_and_dismiss_independently(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let body = visual.debug_bounds("main-pane-frame").unwrap();
        window
            .update(visual, |shell, _, cx| {
                shell.error = Some("Could not save workspace".into());
                shell
                    .notification_state
                    .file_error
                    .update(cx, |notice, cx| {
                        notice.set_message(Some("Could not rename file".into()), cx);
                    });
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        let app = visual.debug_bounds("app-error").unwrap();
        let file = visual.debug_bounds("file-action-error").unwrap();
        let viewport = window.update(visual, |_, w, _| w.viewport_size()).unwrap();
        assert_eq!(app.top(), px(12.));
        assert_eq!(app.right(), viewport.width - px(12.));
        assert_eq!(app.size.width, px(360.));
        assert_eq!(file.size.width, app.size.width);
        assert_eq!(file.right(), app.right());
        assert_eq!(file.top(), app.bottom() + px(8.));
        assert_eq!(visual.debug_bounds("main-pane-frame").unwrap(), body);

        let close = visual.debug_bounds("dismiss-error").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("app-error").is_none());
        assert_eq!(
            visual.debug_bounds("file-action-error").unwrap().top(),
            px(12.)
        );

        visual.simulate_resize(gpui::size(px(300.), px(800.)));
        visual.run_until_parked();
        visual.refresh().unwrap();
        let file = visual.debug_bounds("file-action-error").unwrap();
        assert_eq!(file.left(), px(12.));
        assert_eq!(file.right(), px(288.));
        let close = visual.debug_bounds("dismiss-file-action-error").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("file-action-error").is_none());
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.error.is_none());
                assert!(!shell.notification_state.file_error.read(cx).is_visible());
            })
            .unwrap();
    }

    fn settle(visual: &mut VisualTestContext) {
        visual.run_until_parked();
        visual.refresh().unwrap();
    }

    #[gpui::test]
    fn closed_toast_stays_until_its_exit_ends(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let set_error = |visual: &mut VisualTestContext, error: Option<&str>| {
            window
                .update(visual, |shell, _, cx| {
                    shell.error = error.map(Into::into);
                    cx.notify();
                })
                .unwrap();
            settle(visual);
        };
        set_error(visual, Some("Could not save workspace"));
        set_error(visual, None);
        assert!(visual.debug_bounds("app-error").is_some());

        visual.executor().advance_clock(surface::TOAST_EXIT);
        settle(visual);
        assert!(visual.debug_bounds("app-error").is_none());
    }

    #[gpui::test]
    fn toast_view_stays_until_its_exit_ends(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |shell, _, cx| {
                shell.notification_state.undo.update(cx, |notice, cx| {
                    notice.set_message(Some("Archived 2 threads".into()), cx)
                });
            })
            .unwrap();
        settle(visual);
        window
            .update(visual, |shell, _, cx| shell.dismiss_undo_notice(cx))
            .unwrap();
        settle(visual);
        assert!(visual.debug_bounds("undo-notice").is_some());

        visual.executor().advance_clock(surface::TOAST_EXIT);
        settle(visual);
        assert!(visual.debug_bounds("undo-notice").is_none());
    }

    #[gpui::test]
    fn toast_returning_during_its_exit_stays(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let set_error = |visual: &mut VisualTestContext, error: Option<&str>| {
            window
                .update(visual, |shell, _, cx| {
                    shell.error = error.map(Into::into);
                    cx.notify();
                })
                .unwrap();
            settle(visual);
        };
        set_error(visual, Some("First"));
        set_error(visual, None);
        set_error(visual, Some("Second"));

        visual.executor().advance_clock(surface::TOAST_EXIT);
        settle(visual);
        assert!(visual.debug_bounds("app-error").is_some());
        window
            .update(visual, |shell, _, cx| {
                assert!(
                    shell
                        .notification_state
                        .toasts
                        .app_error
                        .exit("exit")
                        .is_none()
                );
                cx.notify();
            })
            .unwrap();
    }

    #[gpui::test]
    fn narrow_toast_keeps_its_right_edge_as_it_leaves(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let set_layout = |visual: &mut VisualTestContext, dismissed| {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.classic_layout = true;
                    shell.workspace.new_layout_prompt_dismissed = dismissed;
                    cx.notify();
                })
                .unwrap();
            settle(visual);
            settle(visual);
        };
        set_layout(visual, false);
        let shown = visual.debug_bounds("new-layout-notice").unwrap();
        assert_eq!(shown.size.width, px(340.));
        set_layout(visual, true);
        let leaving = visual.debug_bounds("new-layout-notice").unwrap();
        assert_eq!(leaving.size.width, px(340.));
        // It only ever moves right of its resting place while leaving.
        let viewport = window.update(visual, |_, w, _| w.viewport_size()).unwrap();
        let rest = viewport.width - px(12. + 340.);
        assert!(leaving.left() >= rest);
    }

    #[gpui::test]
    fn reduced_motion_closes_toasts_at_once(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |shell, _, cx| {
                shell.error = Some("Could not save workspace".into());
                cx.notify();
            })
            .unwrap();
        settle(visual);
        window
            .update(visual, |shell, _, cx| {
                shell.error = None;
                cx.notify();
            })
            .unwrap();
        settle(visual);
        assert!(visual.debug_bounds("app-error").is_none());
    }

    fn linked_card(session: i64) -> github_activity::Card {
        github_activity::Card {
            session,
            item: monocode_core::session_groups::WorkItem {
                kind: "pr".into(),
                repo: "acme/app".into(),
                number: 7,
                url: "https://github.com/acme/app/pull/7".into(),
            },
            title: "Fix the thing".into(),
            state: "open".into(),
            updated_at: monocode_core::time::now_ms() as i64,
            entries: Vec::new(),
            details_error: false,
            truncated: false,
        }
    }

    /// Shows a GitHub card on the active session and returns that session.
    fn with_linked_card(window: gpui::WindowHandle<Shell>, visual: &mut VisualTestContext) -> i64 {
        let id = window
            .update(visual, |shell, _, cx| {
                let id = shell.workspace.active.unwrap();
                shell.notification_state.github.update(cx, |feed, cx| {
                    feed.cards.insert(id, linked_card(id));
                    cx.notify();
                });
                id
            })
            .unwrap();
        settle(visual);
        settle(visual);
        assert!(visual.debug_bounds("linked-activity").is_some());
        id
    }

    #[gpui::test]
    fn closing_a_github_card_plays_its_exit(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let id = with_linked_card(window, visual);
        window
            .update(visual, |shell, _, cx| {
                // What the close button sends.
                shell.handle_linked_activity(LinkedActivityEvent::Acknowledge(id), cx);
                shell.handle_linked_activity(LinkedActivityEvent::Dismiss(id), cx);
                assert!(shell.notification_state.github.read(cx).cards.is_empty());
            })
            .unwrap();
        settle(visual);
        assert!(visual.debug_bounds("linked-activity").is_some());

        visual.executor().advance_clock(surface::TOAST_EXIT);
        settle(visual);
        assert!(visual.debug_bounds("linked-activity").is_none());
    }

    #[gpui::test]
    fn leaving_a_github_card_for_another_tab_plays_no_exit(cx: &mut TestAppContext) {
        let (window, a, b) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let id = with_linked_card(window, visual);
        let other = if id == a { b } else { a };
        window
            .update(visual, |shell, window, cx| shell.open(other, window, cx))
            .unwrap();
        settle(visual);
        settle(visual);
        assert!(visual.debug_bounds("linked-activity").is_none());
    }

    #[gpui::test]
    fn returning_to_a_tab_does_not_replay_a_github_cards_entry(cx: &mut TestAppContext) {
        let (window, a, b) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let id = with_linked_card(window, visual);
        let other = if id == a { b } else { a };
        window
            .update(visual, |shell, _, cx| {
                let long_ago = Instant::now() - Duration::from_secs(60);
                let arrived = &mut shell.notification_state.linked_arrived;
                arrived.insert(id, long_ago);
                cx.notify();
            })
            .unwrap();
        settle(visual);
        settle(visual);
        let rest = visual.debug_bounds("linked-activity").unwrap();

        for session in [other, id] {
            window
                .update(visual, |shell, window, cx| shell.open(session, window, cx))
                .unwrap();
            settle(visual);
            settle(visual);
        }
        assert_eq!(visual.debug_bounds("linked-activity").unwrap(), rest);
    }

    #[gpui::test]
    fn dismissing_installed_update_does_not_open_whats_new(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |shell, _, cx| {
                shell.notification_state.installed.update(cx, |notice, cx| {
                    notice.set_version(Some("1.2.3".into()), cx)
                });
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        let close = visual.debug_bounds("dismiss-installed-update").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(!shell.notification_state.installed.read(cx).is_visible());
                assert!(shell.notification_state.whats_new.is_none());
            })
            .unwrap();
    }
}

#[cfg(test)]
mod whats_new_tests {
    use super::*;
    use gpui::{
        ListOffset, Pixels, Point, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
        VisualTestContext, point,
    };

    #[gpui::test]
    fn wheel_over_whats_new_leaves_the_session_behind_it_in_place(cx: &mut TestAppContext) {
        let (window, _, session) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let transcript = window
            .update(visual, |shell, _, cx| {
                shell.sessions[&session].update(cx, |session, cx| {
                    session.summary.has_messages = true;
                    cx.notify();
                    session.transcript.clone()
                })
            })
            .unwrap();
        transcript.update(visual, |view, cx| {
            let rows = (0..200)
                .map(|ix| monocode_core::transcript::Row {
                    role: if ix % 2 == 0 { "user" } else { "assistant" }.into(),
                    text: "A message in the history".into(),
                    first: true,
                    sent_at: None,
                })
                .collect();
            view.patch(0, rows, cx);
            view.list = view.list.clone().measure_all();
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        transcript.update(visual, |view, _| {
            view.list.pause_following_tail();
            view.list.scroll_to(ListOffset {
                item_ix: 100,
                offset_in_item: px(0.),
            });
        });
        visual.refresh().unwrap();

        let offset = |visual: &mut VisualTestContext| {
            transcript.read_with(visual, |view, _| {
                view.list.scroll_px_offset_for_scrollbar().y
            })
        };
        let wheel = |visual: &mut VisualTestContext, position: Point<Pixels>| {
            visual.simulate_event(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(40.))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Moved,
            });
            visual.run_until_parked();
            visual.refresh().unwrap();
        };

        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        let start = offset(visual);
        wheel(visual, pane.center());
        let open_position = offset(visual);
        assert_ne!(
            open_position, start,
            "the wheel reaches an unblocked session"
        );

        window
            .update(visual, |shell, _, cx| {
                shell.open_whats_new(crate::notifications::updater::version().into(), cx);
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        // The pane center falls on the card; its corner falls on the scrim.
        wheel(visual, pane.center());
        wheel(visual, pane.origin + point(px(8.), px(8.)));
        assert_eq!(offset(visual), open_position);
    }
}
