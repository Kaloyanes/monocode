//! Inbox: the original provider rail and work-item detail surface, backed by
//! the native adapters and the desktop app's existing credential files.
mod avatar;
mod diff;
mod document;
mod review;
mod ui;
pub(crate) use avatar::image as avatar_image;
pub(crate) use ui::{provider_mark, relative_time};

use crate::{
    conversation::Conversation,
    diff_view::{DiffEvent, DiffView},
    input::{self, Input},
    popover::AnchorSlot,
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    ListAlignment, ListState, Pixels, Subscription, Window, px,
};
use monocode_core::{
    inbox::{self as model, Checks, Details, Diff, Item, Service, Snapshot, Thread, ViewState},
    store::Session,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// Ask the shell to show a work-item link in the Inbox.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = inbox_link, no_json)]
pub(crate) struct OpenWorkLink(pub String);

/// A click on a link to a work item: the Inbox shows it when this window has
/// one, otherwise (or with Cmd) the browser does.
pub(crate) fn follow_link(url: &str, event: &ClickEvent, window: &mut Window, cx: &mut App) {
    if event.modifiers().secondary() || !open_in_inbox(url, window, cx) {
        cx.open_url(url);
    }
}

/// Hand `url` to this window's shell. False when the window has no inbox to
/// show it in, such as a window that is not the main one.
pub(crate) fn open_in_inbox(url: &str, window: &mut Window, cx: &mut App) -> bool {
    let action = OpenWorkLink(url.into());
    if !window.is_action_available(&action, cx) {
        return false;
    }
    window.dispatch_action(Box::new(action), cx);
    true
}

/// The latest Inbox rows, for the composer's `#` picker.
pub(crate) struct Catalog {
    pub items: std::sync::Arc<Vec<Item>>,
    pub at: Instant,
}
impl gpui::Global for Catalog {}

pub(crate) enum Event {
    Close,
    OpenSession(i64),
    StartSession {
        item: Box<Item>,
        prompt: String,
    },
    /// A reviewer's question about a pull request. The shell starts a
    /// read-only chat that shows `question` and sends `prompt` to the model,
    /// then hands the chat back with `attach_ask`.
    /// The pane opened on a pull request with no chat yet: the shell attaches
    /// an earlier one, if the session list has it.
    FindAsk(Box<Item>),
    AskPr {
        item: Box<Item>,
        question: String,
        prompt: String,
        title: String,
    },
    ItemsChanged(Vec<Item>),
    /// Archive these threads, offered once the pull request they link to is
    /// merged.
    ArchiveThreads(Vec<i64>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Summary,
    Code,
    Checks,
}

/// The chat beside a pull request, started by asking about it.
struct AskPane {
    session: i64,
    view: Entity<Conversation>,
}

/// Progress of a link that points at an item the list does not hold.
enum LinkNotice {
    Loading(String),
    Failed { url: String, message: String },
}

/// Links followed from item to item stop growing after this many steps.
const BACK_LIMIT: usize = 20;

pub(crate) struct Inbox {
    store: Database,
    service: Service,
    projects: Vec<PathBuf>,
    appearances:
        std::collections::HashMap<String, monocode_core::project_appearance::ProjectAppearance>,
    state: ViewState,
    snapshot: Snapshot,
    /// Authors offered by the filter menu, most active first. Kept from the
    /// last fetch that wasn't narrowed to one author.
    authors: Vec<model::Person>,
    sessions: Vec<Session>,
    focus: FocusHandle,
    search: Entity<Input>,
    comment: Entity<Input>,
    summary_scroll: ListState,
    summary_width: std::rc::Rc<std::cell::Cell<Pixels>>,
    activity: Vec<ui::Activity>,
    activity_revision: u64,
    summary_rows: std::cell::RefCell<ui::SummaryRows>,
    focus_comment: bool,
    selected: Option<String>,
    /// The item opened from a link when the list does not contain it.
    pinned: Option<Item>,
    /// The current row was picked by the inbox, not the reader, so following a
    /// link from it leaves nothing to go back to.
    auto_selected: bool,
    back: Vec<Item>,
    link_notice: Option<LinkNotice>,
    link_request: u64,
    /// The address last copied and the press that copied it, until the tick fades.
    copied_link: Option<(String, u64)>,
    details: Option<Details>,
    thread: Option<Thread>,
    document: Option<Entity<document::InboxDocument>>,
    comments: Vec<(model::Comment, Entity<document::InboxDocument>)>,
    diff: Option<Diff>,
    diff_view: Entity<DiffView>,
    /// The "Ask about this PR" pane is open beside the detail.
    ask_open: bool,
    ask_input: Entity<Input>,
    /// One chat per item, by key.
    asks: std::collections::HashMap<String, AskPane>,
    ask_starting: bool,
    ask_error: Option<String>,
    focus_ask: bool,
    /// The files in the diff each chat was given, so a citation of one of them
    /// jumps to the Code tab before that tab has loaded.
    ask_paths: std::collections::HashMap<String, Vec<String>>,
    /// A citation waiting for the Code tab's diff to load.
    pending_cite: Option<(String, Option<u32>)>,
    /// The reviewer's pending comments, by pull request.
    review: std::collections::HashMap<String, Vec<model::review::PendingComment>>,
    review_open: bool,
    review_posting: bool,
    review_error: Option<String>,
    /// The URL of the review just posted.
    review_posted: Option<String>,
    checks: Option<Checks>,
    tab: Tab,
    loading: bool,
    detail_loading: bool,
    extra_loading: bool,
    posting: bool,
    acting: bool,
    error: Option<String>,
    detail_error: Option<String>,
    extra_error: Option<String>,
    post_error: Option<String>,
    thread_error: Option<String>,
    expanded_comments: std::collections::HashSet<String>,
    merge_action: String,
    reply: Option<(String, String)>,
    generation: u64,
    detail_generation: u64,
    extra_request: u64,
    filters_open: bool,
    connections_open: bool,
    filter_anchor: AnchorSlot,
    connection_anchor: AnchorSlot,
    connect_provider: Option<String>,
    connect_url: Entity<Input>,
    connect_email: Entity<Input>,
    connect_token: Entity<Input>,
    connecting: bool,
    connect_error: Option<String>,
    start_project: PathBuf,
    project_picker: bool,
    project_anchor: AnchorSlot,
    action_menu: bool,
    action_anchor: AnchorSlot,
    pr_action_anchors: [AnchorSlot; 4],
    action_error: Option<String>,
    action_notice: Option<String>,
    confirm_action: Option<String>,
    resize_start: Option<(f32, f32)>,
    active: bool,
    /// Shows one linked item without the list, as a page inside a Mono. It
    /// leaves the Inbox's saved view state alone.
    detail_only: bool,
    refresh_queued: bool,
    notifications: model::NotificationTracker,
    last_refresh: Instant,
    /// When the snapshot was last replaced by a fetch, in ms; 0 before the first.
    fetched_at: i64,
    /// What a paired phone reads when the desktop's own filters or projects
    /// would narrow it.
    phone: PhoneSnapshot,
    _subscriptions: Vec<Subscription>,
}

/// The phone's own fetch: default filters, over the desktop's projects and
/// the Monos'. Used only while that differs from what the desktop fetches;
/// otherwise the desktop's snapshot is the phone's too and nothing extra runs.
#[derive(Default)]
struct PhoneSnapshot {
    /// Projects the phone knows that the desktop's Inbox does not list itself.
    projects: Vec<PathBuf>,
    snapshot: Option<Snapshot>,
    /// ms, 0 before the first fetch ends.
    fetched_at: i64,
    last_refresh: Option<Instant>,
    loading: bool,
}

/// How often the list refreshes in the background: sooner while the Inbox is
/// on screen.
fn poll_interval(active: bool) -> Duration {
    Duration::from_secs(if active { 120 } else { 300 })
}

/// How old the snapshot may get before a watching phone starts a fetch. The
/// catalog is published every few seconds, so this is the lag a new item can
/// have.
const PHONE_REFRESH: Duration = Duration::from_secs(30);

/// What a paired phone reads from the Inbox: the cached snapshot and the state
/// shared with the desktop, borrowed.
pub(crate) struct PhoneView<'a> {
    pub items: &'a [Item],
    pub errors: &'a std::collections::BTreeMap<String, String>,
    pub connections: &'a std::collections::BTreeMap<String, model::Status>,
    pub viewers: &'a std::collections::BTreeMap<String, String>,
    pub seen: &'a model::Seen,
    pub triage: &'a model::Triage,
    /// ms, None before the first fetch ends.
    pub fetched_at: Option<i64>,
    pub interval_ms: i64,
    pub service: &'a Service,
}

/// The inbox opens scoped to the project it was opened from.
fn other_projects(projects: &[PathBuf], cwd: &PathBuf) -> Vec<String> {
    projects
        .iter()
        .filter(|project| *project != cwd)
        .map(|project| project.to_string_lossy().into_owned())
        .collect()
}

/// Every author in `items`, most items first, then by login.
fn authors(items: &[Item]) -> Vec<model::Person> {
    let mut counts = std::collections::HashMap::<String, (usize, model::Person)>::new();
    for item in items
        .iter()
        .filter(|item| model::has_authors(&item.provider) && !item.author.login.is_empty())
    {
        counts
            .entry(item.author.login.to_lowercase())
            .or_insert_with(|| (0, item.author.clone()))
            .0 += 1;
    }
    let mut authors = counts.into_values().collect::<Vec<_>>();
    authors.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.login.cmp(&y.login)));
    authors.into_iter().map(|(_, person)| person).collect()
}

impl EventEmitter<Event> for Inbox {}
impl Focusable for Inbox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Inbox {
    fn copy_link(&mut self, url: String, cx: &mut Context<Self>) {
        if url.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
        let press = self.copied_link.as_ref().map_or(0, |(_, press)| press + 1);
        self.copied_link = Some((url, press));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_link.as_ref().is_some_and(|(_, p)| *p == press) {
                    this.copied_link = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(crate) fn new(
        store: Database,
        projects: Vec<PathBuf>,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_inner(store, projects, cwd, true, cx)
    }

    /// An inbox that only shows items opened with `open_url`, without the list
    /// or background polling.
    pub(crate) fn detail(
        store: Database,
        projects: Vec<PathBuf>,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut inbox = Self::new_inner(store, projects, cwd, false, cx);
        inbox.detail_only = true;
        inbox
    }

    fn new_inner(
        store: Database,
        mut projects: Vec<PathBuf>,
        cwd: PathBuf,
        poll: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        if !projects.contains(&cwd) {
            projects.push(cwd.clone());
        }
        let (mut state, data_dir) = locked(&store)
            .map(|db| {
                (
                    ViewState::load(&db),
                    db.data_dir()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| cwd.clone()),
                )
            })
            .unwrap_or_else(|_| (ViewState::default(), cwd.clone()));
        state.filters.hidden_projects = other_projects(&projects, &cwd);
        let search = cx.new(|cx| Input::field("Filter inbox", "Filter inbox", content(0.4), cx));
        let comment = cx.new(Input::inbox_comment);
        let connect_url = cx.new(|cx| Input::field("Instance URL", "https://", content(0.4), cx));
        let connect_email = cx.new(|cx| Input::field("Email", "you@example.com", content(0.4), cx));
        let connect_token =
            cx.new(|cx| Input::field("API token", "Personal access token", content(0.4), cx));
        let diff_view = cx.new(|cx| {
            let mut view = DiffView::new(cx);
            view.can_ask = poll;
            view
        });
        let ask_input = cx.new(|cx| {
            let mut input = Input::new(cx);
            input.set_placeholder("Ask about this pull request…", cx);
            input
        });
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&comment, |this, _, _: &input::Changed, cx| {
                let count = this.summary_scroll.item_count();
                this.summary_scroll
                    .remeasure_items(count.saturating_sub(1)..count);
                cx.notify();
            }),
            cx.subscribe(&comment, |this, _, _: &input::Submitted, cx| {
                this.post_comment(cx)
            }),
            cx.subscribe(&diff_view, |this, _, event: &DiffEvent, cx| match event {
                DiffEvent::Comment(text) => {
                    this.comment
                        .update(cx, |input, cx| input.set_text(text.clone(), cx));
                    this.tab = Tab::Summary;
                    this.summary_scroll.scroll_to_end();
                    this.focus_comment = true;
                    cx.notify();
                }
                DiffEvent::Ask(text) => this.ask_question(text.clone(), cx),
                DiffEvent::Review {
                    path,
                    line,
                    side,
                    text,
                } => this.add_review_comment(path.clone(), *line, *side, text.clone(), cx),
                _ => {}
            }),
            cx.subscribe(&ask_input, |this, _, _: &input::Submitted, cx| {
                this.submit_ask(cx)
            }),
        ];
        if poll {
            cx.spawn(async move |this, cx| {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    return;
                }
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(30))
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            if this.last_refresh.elapsed() >= poll_interval(this.active) {
                                this.refresh(cx);
                            }
                            if this.active
                                && this.selected.is_some()
                                && !this.detail_loading
                                && !this.posting
                                && !this.acting
                            {
                                this.load_detail(false, cx);
                                if this.tab == Tab::Checks && !this.extra_loading {
                                    this.checks = None;
                                    this.set_tab(Tab::Checks, cx);
                                }
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        Self {
            service: Service {
                data_dir,
                cwd: cwd.clone(),
                github_viewer: Default::default(),
            },
            store,
            projects,
            appearances: Default::default(),
            state,
            snapshot: Snapshot::default(),
            authors: vec![],
            sessions: vec![],
            focus: cx.focus_handle(),
            search,
            comment,
            summary_scroll: ListState::new(0, ListAlignment::Top, px(200.)),
            summary_width: Default::default(),
            activity: vec![],
            activity_revision: 0,
            summary_rows: Default::default(),
            focus_comment: false,
            selected: None,
            pinned: None,
            auto_selected: false,
            back: vec![],
            link_notice: None,
            link_request: 0,
            copied_link: None,
            details: None,
            thread: None,
            document: None,
            comments: vec![],
            diff: None,
            diff_view,
            ask_open: false,
            ask_input,
            asks: Default::default(),
            ask_starting: false,
            ask_error: None,
            focus_ask: false,
            ask_paths: Default::default(),
            pending_cite: None,
            review: Default::default(),
            review_open: false,
            review_posting: false,
            review_error: None,
            review_posted: None,
            checks: None,
            tab: Tab::Summary,
            loading: false,
            detail_loading: false,
            extra_loading: false,
            posting: false,
            acting: false,
            error: None,
            detail_error: None,
            extra_error: None,
            post_error: None,
            thread_error: None,
            expanded_comments: Default::default(),
            merge_action: "merge".into(),
            reply: None,
            generation: 0,
            detail_generation: 0,
            extra_request: 0,
            filters_open: false,
            connections_open: false,
            filter_anchor: AnchorSlot::default(),
            connection_anchor: AnchorSlot::default(),
            connect_provider: None,
            connect_url,
            connect_email,
            connect_token,
            connecting: false,
            connect_error: None,
            start_project: cwd,
            project_picker: false,
            project_anchor: AnchorSlot::default(),
            action_menu: false,
            action_anchor: AnchorSlot::default(),
            pr_action_anchors: std::array::from_fn(|_| AnchorSlot::default()),
            action_error: None,
            action_notice: None,
            confirm_action: None,
            resize_start: None,
            active: false,
            detail_only: false,
            refresh_queued: false,
            notifications: model::NotificationTracker::default(),
            last_refresh: Instant::now(),
            fetched_at: 0,
            phone: PhoneSnapshot::default(),
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_context(
        &mut self,
        mut projects: Vec<PathBuf>,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if !projects.contains(&cwd) {
            projects.push(cwd.clone());
        }
        if self.projects != projects || self.service.cwd != cwd {
            let switched = self.service.cwd != cwd;
            if switched {
                self.state.filters.hidden_projects = other_projects(&projects, &cwd);
                self.save();
            }
            self.projects = projects;
            self.service.cwd = cwd.clone();
            self.start_project = cwd;
            if switched {
                let visible = self.visible_items(cx);
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|key| !visible.iter().any(|item| &item.key() == key))
                {
                    self.deselect(cx);
                    if self.active
                        && let Some(item) = visible.first()
                    {
                        self.select(item.key(), cx);
                    }
                }
            }
            self.refresh(cx);
        }
    }

    pub(crate) fn set_appearances(
        &mut self,
        appearances: std::collections::HashMap<
            String,
            monocode_core::project_appearance::ProjectAppearance,
        >,
        cx: &mut Context<Self>,
    ) {
        if self.appearances != appearances {
            self.appearances = appearances;
            cx.notify();
        }
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            if let Some(item) = self.item() {
                self.state.seen.mark(&item);
                self.save();
            } else if let Some(item) = self.visible_items(cx).first() {
                self.select(item.key(), cx);
                self.auto_selected = true;
            }
        }
        cx.notify();
    }

    /// The item on screen, whether or not the list holds it.
    pub(crate) fn shown(&self) -> Option<Item> {
        self.item()
    }

    /// Whether the Inbox is the view on screen.
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// The projects a phone fetch covers: the desktop's and the Monos'.
    fn phone_projects(&self) -> Vec<PathBuf> {
        let key = |path: &PathBuf| monocode_core::files::path_key(&path.to_string_lossy());
        let mut projects = self.projects.clone();
        for path in &self.phone.projects {
            if !projects.iter().any(|known| key(known) == key(path)) {
                projects.push(path.clone());
            }
        }
        projects
    }

    fn phone_scope(&self) -> String {
        format!(
            "{}:{}",
            ViewState::default().fetch_scope(&self.phone_projects()),
            monocode_core::files::path_key(&self.service.cwd.to_string_lossy())
        )
    }

    /// Whether the desktop's fetch would hide something from the phone: its
    /// filters narrow the providers' rows, or a Mono lists a project it doesn't.
    fn phone_separate(&self) -> bool {
        self.phone_scope() != self.fetch_scope()
    }

    /// The snapshot a phone reads and when it was fetched. Until the phone's own
    /// first fetch ends it reads the desktop's, flagged as never fetched.
    fn phone_source(&self) -> (&Snapshot, Option<i64>) {
        if !self.phone_separate() {
            return (
                &self.snapshot,
                (self.fetched_at > 0).then_some(self.fetched_at),
            );
        }
        match &self.phone.snapshot {
            Some(snapshot) => (
                snapshot,
                (self.phone.fetched_at > 0).then_some(self.phone.fetched_at),
            ),
            None => (&self.snapshot, None),
        }
    }

    /// Tells the Inbox which projects the Monos add for the phone.
    pub(crate) fn set_phone_projects(&mut self, projects: Vec<PathBuf>) {
        if self.phone.projects != projects {
            self.phone.projects = projects;
            self.phone.last_refresh = None;
        }
    }

    pub(crate) fn phone_view(&self) -> PhoneView<'_> {
        let (snapshot, fetched_at) = self.phone_source();
        PhoneView {
            items: &snapshot.items,
            errors: &snapshot.errors,
            connections: &snapshot.connections,
            viewers: &snapshot.viewers,
            seen: &self.state.seen,
            triage: &self.state.triage,
            fetched_at,
            interval_ms: poll_interval(self.active).as_millis() as i64,
            service: &self.service,
        }
    }

    /// Names what a phone would read, so it reads again only when this moves.
    pub(crate) fn phone_rev(&self) -> String {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let view = self.phone_view();
        let content = serde_json::to_value((view.items, view.errors, view.seen, view.triage))
            .map(|value| value.to_string())
            .unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// A phone looked at the list: refresh in the background when the last
    /// fetch is older than `PHONE_REFRESH`. The answer does not wait for it.
    pub(crate) fn refresh_for_phone(&mut self, cx: &mut Context<Self>) {
        if self.phone_separate() {
            let due = self
                .phone
                .last_refresh
                .is_none_or(|at| at.elapsed() >= PHONE_REFRESH);
            if due {
                self.refresh_phone(cx);
            }
            return;
        }
        self.phone.snapshot = None;
        let due = self.last_refresh.elapsed() >= PHONE_REFRESH;
        if due && !self.loading {
            self.refresh(cx);
        }
    }

    /// A phone asked for a fresh read: fetch now unless one is already running.
    /// `fetching` tells when the answer is ready.
    pub(crate) fn refresh_now_for_phone(&mut self, cx: &mut Context<Self>) {
        if self.phone_separate() {
            self.refresh_phone(cx);
        } else if !self.loading {
            self.refresh(cx);
        }
    }

    /// Whether the fetch a phone waits for is running or queued.
    pub(crate) fn fetching(&self) -> bool {
        if self.phone_separate() {
            self.phone.loading
        } else {
            self.loading || self.refresh_queued
        }
    }

    fn refresh_phone(&mut self, cx: &mut Context<Self>) {
        if self.phone.loading {
            return;
        }
        self.phone.loading = true;
        self.phone.last_refresh = Some(Instant::now());
        let scope = self.phone_scope();
        let service = self.service.clone();
        let projects = self.phone_projects();
        let task = cx
            .background_executor()
            .spawn(async move { service.list(&projects, &ViewState::default()) });
        cx.spawn(async move |this, cx| {
            let mut snapshot = task.await;
            let _ = this.update(cx, |this, cx| {
                this.phone.loading = false;
                if scope != this.phone_scope() {
                    // The projects or the checkout changed meanwhile: fetch again on the next look.
                    this.phone.last_refresh = None;
                    return;
                }
                // A temporary outage must not erase the last usable provider list.
                if let Some(previous) = &this.phone.snapshot {
                    for provider in snapshot.errors.keys() {
                        snapshot.items.extend(
                            previous
                                .items
                                .iter()
                                .filter(|i| &i.provider == provider)
                                .cloned(),
                        );
                    }
                }
                snapshot
                    .items
                    .sort_by_key(|item| std::cmp::Reverse(item.updated_ms()));
                if this.phone.snapshot.is_none() && this.state.seen.seeded {
                    // Rows only the phone lists start out read, as the desktop's start;
                    // the ones the desktop lists keep their own state.
                    let listed = this
                        .snapshot
                        .items
                        .iter()
                        .map(|item| item.key())
                        .collect::<std::collections::HashSet<_>>();
                    for item in &snapshot.items {
                        if !listed.contains(&item.key())
                            && !this.state.seen.items.contains_key(&item.key())
                        {
                            this.state.seen.mark(item);
                        }
                    }
                    this.save();
                }
                this.phone.fetched_at = monocode_core::time::now_ms() as i64;
                this.phone.snapshot = Some(snapshot);
                cx.notify();
            });
        })
        .detach();
    }

    /// A phone's triage action on a listed item, kept and saved with the
    /// desktop's own read state. The item as it now stands.
    pub(crate) fn phone_mark(
        &mut self,
        key: &str,
        mark: monocode_core::mobile::inbox::Mark,
        cx: &mut Context<Self>,
    ) -> Result<Item, String> {
        let item = self
            .phone_source()
            .0
            .items
            .iter()
            .find(|item| item.key() == key)
            .cloned()
            .ok_or("That item is no longer in the Inbox")?;
        let now = monocode_core::time::now_ms() as i64;
        self.state.triage.prune(now);
        mark.apply(&item, &mut self.state.seen, &mut self.state.triage, now);
        self.save();
        cx.notify();
        Ok(item)
    }

    /// Marks read what a phone's list shows, and says how many that was.
    pub(crate) fn phone_read_all(
        &mut self,
        params: &monocode_core::mobile::inbox::ListParams,
        projects: &[monocode_core::mobile::inbox::ProjectRef],
        cx: &mut Context<Self>,
    ) -> usize {
        use monocode_core::mobile::inbox::{View, visible};
        let unread: Vec<Item> = {
            let snapshot = self.phone_source().0;
            let view = View {
                seen: &self.state.seen,
                triage: &self.state.triage,
                projects,
                viewers: &snapshot.viewers,
                now: monocode_core::time::now_ms() as i64,
            };
            visible(&snapshot.items, params, &view)
                .into_iter()
                .filter(|item| self.state.seen.unseen(item))
                .cloned()
                .collect()
        };
        for item in &unread {
            self.state.seen.mark(item);
        }
        if !unread.is_empty() {
            self.save();
            cx.notify();
        }
        unread.len()
    }

    /// A phone's action changed an item at its provider. `updated` is the
    /// provider's copy, which has no attention reason until the next fetch. The
    /// item as it now stands, read because the user just acted on it.
    pub(crate) fn phone_acted(
        &mut self,
        key: &str,
        mut updated: Item,
        cx: &mut Context<Self>,
    ) -> Item {
        if let Some(listed) = self
            .phone_source()
            .0
            .items
            .iter()
            .find(|item| item.key() == key)
        {
            updated.attention_reason = listed.attention_reason.clone();
        }
        self.replace_item(key, updated.clone());
        if let Some(old) = self
            .phone
            .snapshot
            .iter_mut()
            .flat_map(|snapshot| &mut snapshot.items)
            .find(|item| item.key() == key)
        {
            *old = updated.clone();
        }
        self.state.seen.mark(&updated);
        self.save();
        self.phone_discussed(key, cx);
        updated
    }

    /// Something a phone did at the provider shows in the next read: the
    /// desktop's detail if it has the item open, and both lists.
    pub(crate) fn phone_discussed(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(key) {
            self.load_detail(false, cx);
        }
        self.refresh(cx);
        if self.phone_separate() {
            self.refresh_phone(cx);
        }
        cx.notify();
    }

    pub(crate) fn unread_count(&self) -> usize {
        self.snapshot
            .items
            .iter()
            .filter(|item| self.state.seen.unseen(item))
            .count()
    }

    pub(crate) fn connection_statuses(&self) -> std::collections::HashMap<String, (bool, String)> {
        let mut statuses = self
            .snapshot
            .connections
            .iter()
            .map(|(provider, status)| {
                let description = if !status.connected {
                    "Not connected".into()
                } else if !status.site.is_empty() {
                    status.site.clone()
                } else if !status.url.is_empty() {
                    status.url.clone()
                } else if !status.organization.is_empty() {
                    status.organization.clone()
                } else {
                    "Connected".into()
                };
                (provider.clone(), (status.connected, description))
            })
            .collect::<std::collections::HashMap<_, _>>();
        for (provider, error) in &self.snapshot.errors {
            if !statuses.contains_key(provider) {
                statuses.insert(provider.clone(), (false, error.clone()));
            }
        }
        statuses
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.refresh_queued = true;
            return;
        }
        self.loading = true;
        self.last_refresh = Instant::now();
        self.generation += 1;
        let generation = self.generation;
        let service = self.service.clone();
        let state = self.state.clone();
        let projects = self.projects.clone();
        let scope = self.fetch_scope();
        let store = self.store.clone();
        let task = cx.background_executor().spawn(async move {
            let snapshot = service.list(&projects, &state);
            let sessions = locked(&store)
                .and_then(|db| db.sessions())
                .unwrap_or_default();
            (snapshot, sessions)
        });
        cx.spawn(async move |this, cx| {
            let (mut snapshot, sessions) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                if scope != this.fetch_scope() {
                    this.refresh_queued = false;
                    this.refresh(cx);
                    return;
                }
                let appeared = this.notifications.observe(&snapshot, scope);
                // A temporary outage must not erase the last usable provider list.
                for provider in snapshot.errors.keys() {
                    snapshot.items.extend(
                        this.snapshot
                            .items
                            .iter()
                            .filter(|i| &i.provider == provider)
                            .cloned(),
                    );
                }
                snapshot
                    .items
                    .sort_by_key(|item| std::cmp::Reverse(item.updated_ms()));
                for (provider, login) in &this.snapshot.viewers {
                    snapshot
                        .viewers
                        .entry(provider.clone())
                        .or_insert_with(|| login.clone());
                }
                if this.state.filters.fetch_author().is_none() || this.authors.is_empty() {
                    this.authors = authors(&snapshot.items);
                }
                this.state.seen.seed(&snapshot.items);
                this.fetched_at = monocode_core::time::now_ms() as i64;
                this.snapshot = snapshot;
                this.sessions = sessions;
                if !this.detail_only {
                    cx.set_global(Catalog {
                        items: this.snapshot.items.clone().into(),
                        at: Instant::now(),
                    });
                }
                if this.active
                    && let Some(item) = this.item()
                {
                    this.state.seen.mark(&item);
                }
                if !this
                    .snapshot
                    .connections
                    .get(&this.state.source)
                    .is_some_and(|s| s.connected)
                    && let Some(source) = model::PROVIDERS.into_iter().find(|p| {
                        this.snapshot
                            .connections
                            .get(*p)
                            .is_some_and(|s| s.connected)
                    })
                {
                    this.state.source = source.into();
                }
                this.save();
                cx.emit(Event::ItemsChanged(appeared));
                if this.selected.is_some()
                    && this
                        .item()
                        .is_none_or(|item| item.provider != this.state.source)
                {
                    this.deselect(cx);
                    this.pinned = None;
                    this.back.clear();
                }
                if this.active
                    && this.selected.is_none()
                    && let Some(item) = this.visible_items(cx).first()
                {
                    this.select(item.key(), cx);
                    this.auto_selected = true;
                }
                if std::mem::take(&mut this.refresh_queued) {
                    this.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Rows are assigned to the active checkout, so a fetch made for another
    /// one is stale even when the project list is the same.
    fn fetch_scope(&self) -> String {
        format!(
            "{}:{}",
            self.state.fetch_scope(&self.projects),
            monocode_core::files::path_key(&self.service.cwd.to_string_lossy())
        )
    }
    fn save(&mut self) {
        if self.detail_only {
            return;
        }
        if let Err(error) = locked(&self.store).and_then(|db| self.state.save(&db)) {
            self.error = Some(error);
        }
    }
    fn viewer(&self) -> &str {
        self.snapshot
            .viewers
            .get(&self.state.source)
            .map_or("", String::as_str)
    }
    fn visible_items(&self, cx: &App) -> Vec<Item> {
        let search = self.search.read(cx).content.as_ref();
        let viewer = self.viewer();
        self.snapshot
            .items
            .iter()
            .filter(|item| {
                self.state
                    .filters
                    .matches(item, &self.state.source, search, viewer)
            })
            .filter(|item| {
                item.provider != "linear" || !self.state.hidden_linear_teams.contains(&item.team_id)
            })
            .cloned()
            .collect()
    }
    fn item(&self) -> Option<Item> {
        self.snapshot
            .items
            .iter()
            .find(|item| Some(item.key()) == self.selected)
            .or(self
                .pinned
                .as_ref()
                .filter(|item| Some(item.key()) == self.selected))
            .cloned()
    }
    /// Shows `item` as a followed link would, without looking it up.
    #[cfg(test)]
    pub(crate) fn pin_for_test(&mut self, item: Item) {
        self.selected = Some(item.key());
        self.pinned = Some(item);
    }
    /// A detail view's item is pinned, with no list row behind it.
    fn replace_item(&mut self, key: &str, item: Item) {
        if let Some(old) = self
            .snapshot
            .items
            .iter_mut()
            .chain(&mut self.pinned)
            .find(|i| i.key() == key)
        {
            *old = item;
        }
    }
    /// A detail view never polls, so the list refresh that fills `sessions`
    /// would not run before it shows an item's threads.
    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let task = cx
            .background_executor()
            .spawn(async move { locked(&store).and_then(|db| db.sessions()) });
        cx.spawn(async move |this, cx| {
            let Ok(sessions) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.sessions = sessions;
                cx.notify();
            });
        })
        .detach();
    }
    fn archive_threads(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        for session in &mut self.sessions {
            session.archived |= ids.contains(&session.id);
        }
        cx.emit(Event::ArchiveThreads(ids));
        cx.notify();
    }
    /// A row picked from the rail starts a fresh trail of linked items.
    fn select(&mut self, key: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        self.back.clear();
        self.auto_selected = false;
        if self.pinned.as_ref().is_some_and(|item| item.key() != key) {
            self.pinned = None;
        }
        self.show(key, cx);
    }
    /// Whether `open_url` would show `url` here rather than leave it to the
    /// browser.
    pub(crate) fn can_open(&self, url: &str) -> bool {
        model::link_target(url).is_some()
            || model::find_linked(&self.snapshot.items, url).is_some()
            || self
                .pinned
                .as_ref()
                .is_some_and(|item| model::find_linked(std::slice::from_ref(item), url).is_some())
    }
    /// Follow a link to a work item. Items the list holds open directly;
    /// GitHub and Linear items it does not hold are fetched and shown without
    /// joining the list. Returns false for links that belong to the browser.
    pub(crate) fn open_url(&mut self, url: &str, cx: &mut Context<Self>) -> bool {
        if self.detail_only {
            self.load_sessions(cx);
        }
        let target = model::link_target(url);
        let tab = match &target {
            Some(model::LinkTarget::GitHub(work)) if work.kind == "pr" => {
                let lower = url.to_ascii_lowercase();
                if lower.contains("/files") {
                    Tab::Code
                } else if lower.contains("/checks") {
                    Tab::Checks
                } else {
                    Tab::Summary
                }
            }
            _ => Tab::Summary,
        };
        let known = model::find_linked(&self.snapshot.items, url)
            .or(self
                .pinned
                .as_ref()
                .filter(|item| model::find_linked(std::slice::from_ref(*item), url).is_some()))
            .cloned();
        if let Some(item) = known {
            self.link_notice = None;
            self.go_to(item, tab, cx);
            return true;
        }
        let Some(target) = target else {
            return false;
        };
        self.link_request += 1;
        let request = self.link_request;
        let label = match &target {
            model::LinkTarget::GitHub(work) => format!("{}#{}", work.repo, work.number),
            model::LinkTarget::Linear(identifier) => identifier.clone(),
        };
        self.link_notice = Some(LinkNotice::Loading(label));
        let url = url.to_owned();
        let service = self.service.clone();
        let projects = self.projects.clone();
        let task = cx
            .background_executor()
            .spawn(async move { service.linked_item(&target, &projects) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.link_request != request {
                    return;
                }
                match result {
                    Ok(item) => {
                        this.link_notice = None;
                        this.go_to(item, tab, cx);
                    }
                    Err(message) => this.link_notice = Some(LinkNotice::Failed { url, message }),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }
    fn go_to(&mut self, item: Item, tab: Tab, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&item.key()) {
            self.set_tab(tab, cx);
            return;
        }
        let automatic = std::mem::take(&mut self.auto_selected);
        if let Some(current) = self.item().filter(|_| !automatic) {
            self.back.push(current);
            if self.back.len() > BACK_LIMIT {
                self.back.remove(0);
            }
        }
        self.follow(item, tab, cx);
    }
    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.back.pop() {
            self.follow(item, Tab::Summary, cx);
        }
    }
    fn follow(&mut self, item: Item, tab: Tab, cx: &mut Context<Self>) {
        let key = item.key();
        self.pinned = (!self.snapshot.items.iter().any(|i| i.key() == key)).then_some(item.clone());
        if item.provider != self.state.source {
            self.state.source = item.provider.clone();
            self.save();
        }
        self.show(key, cx);
        if tab != Tab::Summary {
            self.set_tab(tab, cx);
        }
    }
    fn show(&mut self, key: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        self.link_notice = None;
        self.selected = Some(key);
        self.reset_detail(cx);
        self.load_review(cx);
        if let Some(item) = self.item() {
            if self.active {
                self.state.seen.mark(&item);
                self.save();
            }
            if !item.project_path.is_empty() {
                self.start_project = PathBuf::from(&item.project_path);
            }
        }
        self.load_detail(true, cx);
        cx.notify();
    }
    fn deselect(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.reset_detail(cx);
        cx.notify();
    }
    /// Drops the detail pane's state and invalidates its in-flight requests.
    fn reset_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        self.detail_loading = false;
        self.detail_error = None;
        self.summary_scroll = ListState::new(0, ListAlignment::Top, px(200.));
        self.activity.clear();
        self.activity_revision += 1;
        self.summary_rows = Default::default();
        self.focus_comment = false;
        self.tab = Tab::Summary;
        self.diff = None;
        self.pending_cite = None;
        self.extra_request += 1;
        self.diff_view
            .update(cx, |view, cx| view.set_files(vec![], cx));
        self.checks = None;
        self.extra_loading = false;
        self.extra_error = None;
        self.details = None;
        self.thread = None;
        self.document = None;
        self.comments.clear();
        self.reply = None;
        self.post_error = None;
        self.thread_error = None;
        self.expanded_comments.clear();
        self.merge_action = "merge".into();
        self.confirm_action = None;
        self.action_error = None;
        self.action_notice = None;
        self.action_menu = false;
        self.comment
            .update(cx, |input, cx| input.set_text(String::new(), cx));
    }
    fn change_source(&mut self, source: String, cx: &mut Context<Self>) {
        self.state.source = source;
        self.selected = None;
        self.pinned = None;
        self.back.clear();
        self.link_notice = None;
        self.details = None;
        self.document = None;
        self.comments.clear();
        self.search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        if let Some(item) = self.visible_items(cx).first() {
            self.select(item.key(), cx);
        }
        self.save();
        cx.notify();
    }
    fn load_detail(&mut self, clear: bool, cx: &mut Context<Self>) {
        let Some(item) = self.item() else {
            return;
        };
        self.detail_generation += 1;
        let generation = self.detail_generation;
        self.detail_loading = true;
        self.detail_error = None;
        self.thread_error = None;
        if clear {
            self.document = None;
            self.comments.clear();
        }
        let service = self.service.clone();
        let task = cx
            .background_executor()
            .spawn(async move { (service.details(&item), service.thread(&item)) });
        cx.spawn(async move |this, cx| {
            let (details, thread) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.detail_generation != generation {
                    return;
                }
                let inbox = cx.weak_entity();
                this.detail_loading = false;
                match details {
                    Ok(details) => {
                        if this
                            .details
                            .as_ref()
                            .is_none_or(|old| old.body != details.body)
                        {
                            this.document = Some(cx.new(|cx| {
                                document::InboxDocument::linked(
                                    details.body.clone(),
                                    this.document_cwd(),
                                    inbox.clone(),
                                    cx,
                                )
                            }));
                        }
                        this.details = Some(details);
                    }
                    Err(error) => this.detail_error = Some(error),
                }
                match thread {
                    Ok(thread) => {
                        let mut rows = vec![];
                        for comment in &thread.comments {
                            let document = this
                                .comments
                                .iter()
                                .find(|(old, _)| old.id == comment.id && old.body == comment.body)
                                .map(|(_, document)| document.clone())
                                .unwrap_or_else(|| {
                                    cx.new(|cx| {
                                        document::InboxDocument::linked(
                                            comment.body.clone(),
                                            this.document_cwd(),
                                            inbox.clone(),
                                            cx,
                                        )
                                    })
                                });
                            rows.push((comment.clone(), document));
                            for reply in &comment.replies {
                                let document = this
                                    .comments
                                    .iter()
                                    .find(|(old, _)| old.id == reply.id && old.body == reply.body)
                                    .map(|(_, document)| document.clone())
                                    .unwrap_or_else(|| {
                                        cx.new(|cx| {
                                            document::InboxDocument::linked(
                                                reply.body.clone(),
                                                this.document_cwd(),
                                                inbox.clone(),
                                                cx,
                                            )
                                        })
                                    });
                                rows.push((reply.clone(), document));
                            }
                        }
                        this.comments = rows;
                        this.thread = Some(thread);
                        this.rebuild_activity();
                    }
                    Err(error) => this.thread_error = Some(error),
                }
                this.summary_scroll.remeasure();
                cx.notify();
            });
        })
        .detach();
    }
    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.extra_error = None;
        let Some(item) = self.item() else {
            return;
        };
        if tab == Tab::Summary
            || (tab == Tab::Code && self.diff.is_some())
            || (tab == Tab::Checks && self.checks.is_some())
        {
            self.extra_loading = false;
            cx.notify();
            return;
        }
        self.extra_loading = true;
        self.extra_request += 1;
        let request = self.extra_request;
        let key = item.key();
        let service = self.service.clone();
        let task = cx.background_executor().spawn(async move {
            if tab == Tab::Code {
                service.diff(&item).map(|diff| {
                    let files = diff::models(&diff);
                    (Some((diff, files)), None)
                })
            } else {
                service.checks(&item).map(|checks| (None, Some(checks)))
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.selected.as_ref() != Some(&key)
                    || this.tab != tab
                    || request != this.extra_request
                {
                    return;
                }
                this.extra_loading = false;
                match result {
                    Ok((diff, checks)) => {
                        if let Some((diff, files)) = diff {
                            this.diff_view.update(cx, |view, cx| {
                                view.file_count = Some(diff.files.len());
                                view.totals = Some((
                                    diff.additions.max(0).min(u32::MAX as i64) as u32,
                                    diff.deletions.max(0).min(u32::MAX as i64) as u32,
                                ));
                                view.set_files(files, cx);
                            });
                            this.diff = Some(diff);
                            if let Some((path, line)) = this.pending_cite.take() {
                                this.diff_view
                                    .update(cx, |view, cx| view.reveal(&path, line, cx));
                            }
                        }
                        if let Some(checks) = checks {
                            this.checks = Some(checks);
                        }
                    }
                    Err(error) => this.extra_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn post_comment(&mut self, cx: &mut Context<Self>) {
        if self.posting {
            return;
        }
        let Some(item) = self.item() else {
            return;
        };
        let body = self.comment.read(cx).content.trim().to_string();
        if body.is_empty() {
            return;
        }
        self.posting = true;
        self.post_error = None;
        let key = item.key();
        let service = self.service.clone();
        let reply = self.reply.as_ref().map(|r| r.0.clone()).unwrap_or_default();
        let task = cx
            .background_executor()
            .spawn(async move { service.comment(&item, body, reply) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.posting = false;
                if this.selected.as_ref() != Some(&key) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(_) => {
                        this.comment
                            .update(cx, |input, cx| input.set_text(String::new(), cx));
                        this.reply = None;
                        this.load_detail(false, cx);
                        this.refresh(cx);
                    }
                    Err(error) => this.post_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    /// The chat for the item on screen, when the pane is open.
    fn ask_pane(&self) -> Option<&AskPane> {
        let key = self.item()?.key();
        self.asks.get(&key)
    }
    /// The chat the Inbox shows beside the detail, which the shell must keep
    /// drawn.
    pub(crate) fn ask_session(&self) -> Option<i64> {
        self.ask_open
            .then(|| self.ask_pane().map(|pane| pane.session))
            .flatten()
    }
    fn open_ask(&mut self, cx: &mut Context<Self>) {
        self.ask_open = true;
        self.focus_ask = true;
        if self.ask_pane().is_none() && !self.ask_starting {
            if let Some(item) = self.item() {
                cx.emit(Event::FindAsk(Box::new(item)));
            }
        }
        cx.notify();
    }
    fn close_ask(&mut self, cx: &mut Context<Self>) {
        self.ask_open = false;
        cx.notify();
    }
    fn submit_ask(&mut self, cx: &mut Context<Self>) {
        let text = self.ask_input.read(cx).content.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.ask_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.ask_question(text, cx);
    }
    /// Sends `question` to the item's chat, starting it on the first one.
    fn ask_question(&mut self, question: String, cx: &mut Context<Self>) {
        let Some(mut item) = self.item() else {
            return;
        };
        self.ask_open = true;
        let key = item.key();
        if let Some(pane) = self.asks.get(&key) {
            pane.view
                .update(cx, |chat, cx| chat.submit_follow_up(question, cx));
            cx.notify();
            return;
        }
        if self.ask_starting {
            return;
        }
        self.ask_starting = true;
        self.ask_error = None;
        if item.project_path.is_empty() {
            item.project_path = self.start_project.to_string_lossy().into_owned();
        }
        let details = self.details.clone();
        let loaded = self.diff.clone();
        let service = self.service.clone();
        let task_item = item.clone();
        let task = cx.background_executor().spawn(async move {
            match loaded {
                Some(diff) => Some(diff),
                None => service.diff(&task_item).ok(),
            }
        });
        cx.spawn(async move |this, cx| {
            let diff = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(diff) = &diff {
                    this.ask_paths.insert(
                        item.key(),
                        diff.files.iter().map(|file| file.path.clone()).collect(),
                    );
                }
                let prompt = model::ask::prompt(&item, details.as_ref(), diff.as_ref(), &question);
                let title = model::ask::session_title(&item);
                cx.emit(Event::AskPr {
                    item: Box::new(item),
                    question,
                    prompt,
                    title,
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    /// The shell started the chat for `key`.
    pub(crate) fn attach_ask(
        &mut self,
        key: String,
        session: i64,
        view: Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        self.ask_starting = false;
        let this = cx.entity().downgrade();
        view.update(cx, |chat, cx| {
            chat.set_link_intercept(
                Some(std::rc::Rc::new(move |url, _, cx| {
                    let url = url.to_owned();
                    this.update(cx, |inbox, cx| inbox.cite(&url, cx))
                        .unwrap_or(false)
                })),
                cx,
            )
        });
        self.asks.insert(key, AskPane { session, view });
        self.ask_open = true;
        cx.notify();
    }
    /// Shows an existing chat beside the pull request on screen, and opens
    /// the pane.
    pub(crate) fn attach_selected_ask(
        &mut self,
        session: i64,
        view: Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.item().map(|item| item.key()) {
            self.attach_ask(key, session, view, cx);
            self.focus_ask = true;
        }
    }
    pub(crate) fn ask_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.ask_starting = false;
        self.ask_error = Some(message);
        cx.notify();
    }
    /// An answer cited `url`, a file and maybe a line. Jumps to it in the diff
    /// when the pull request changed that file; false leaves the link to the
    /// editor.
    fn cite(&mut self, url: &str, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.item().map(|item| item.key()) else {
            return false;
        };
        let (path, location) = crate::file_editor::file_location(url, "");
        let line = location.map(|(line, _)| line as u32);
        let in_diff = match &self.diff {
            Some(diff) => diff
                .files
                .iter()
                .any(|file| model::ask::same_file(&path, &file.path)),
            None => self
                .ask_paths
                .get(&key)
                .is_some_and(|paths| paths.iter().any(|file| model::ask::same_file(&path, file))),
        };
        if !in_diff {
            return false;
        }
        if self.diff.is_some() {
            self.set_tab(Tab::Code, cx);
            self.diff_view
                .update(cx, |view, cx| view.reveal(&path, line, cx));
        } else {
            self.pending_cite = Some((path, line));
            self.set_tab(Tab::Code, cx);
        }
        true
    }
    /// Moves the chat out of the pane into the main conversation.
    fn open_ask_in_conversation(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.item().map(|item| item.key()) else {
            return;
        };
        if let Some(pane) = self.asks.remove(&key) {
            self.ask_open = false;
            cx.emit(Event::OpenSession(pane.session));
            cx.notify();
        }
    }
    fn document_cwd(&self) -> PathBuf {
        self.item()
            .filter(|item| !item.project_path.is_empty())
            .map(|item| PathBuf::from(item.project_path))
            .unwrap_or_else(|| self.start_project.clone())
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        if let Some(mut item) = self.item() {
            if item.project_path.is_empty() {
                item.project_path = self.start_project.to_string_lossy().into_owned();
            }
            let body = self.details.as_ref().map(|d| d.body.as_str()).unwrap_or("");
            let discussion = self
                .comments
                .iter()
                .map(|(c, _)| format!("{}: {}", c.author, c.body))
                .collect::<Vec<_>>()
                .join("\n\n");
            let prompt = item.prompt(&format!("{body}\n\n{discussion}"), false);
            cx.emit(Event::StartSession {
                item: Box::new(item),
                prompt,
            });
        }
    }
    pub(crate) fn open_connection(&mut self, provider: String, cx: &mut Context<Self>) {
        self.connections_open = false;
        self.connect_error = None;
        let status = self
            .snapshot
            .connections
            .get(&provider)
            .cloned()
            .unwrap_or_default();
        let url = if !status.site.is_empty() {
            status.site
        } else if !status.url.is_empty() {
            status.url
        } else if !status.organization.is_empty() {
            status.organization
        } else if provider == "gitlab" {
            "https://gitlab.com".into()
        } else {
            String::new()
        };
        self.connect_url
            .update(cx, |input, cx| input.set_text(url, cx));
        self.connect_email
            .update(cx, |input, cx| input.set_text(status.email, cx));
        self.connect_token
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.connect_provider = Some(provider);
        cx.notify();
    }
    fn connect(&mut self, disconnect: bool, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let Some(provider) = self.connect_provider.clone() else {
            return;
        };
        let url = self.connect_url.read(cx).content.to_string();
        let email = self.connect_email.read(cx).content.to_string();
        let token = if disconnect {
            String::new()
        } else {
            self.connect_token.read(cx).content.to_string()
        };
        if provider != "github" && !disconnect && token.trim().is_empty() {
            self.connect_error = Some("Enter an API token".into());
            cx.notify();
            return;
        }
        self.connecting = true;
        self.connect_error = None;
        let service = self.service.clone();
        let source = provider.clone();
        let task = cx
            .background_executor()
            .spawn(async move { service.connect(&source, url, email, token) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.connecting = false;
                match result {
                    Ok(status) => {
                        let connected = status.connected;
                        this.snapshot.connections.insert(provider.clone(), status);
                        this.connect_token.update(cx, |input, cx| input.set_text(String::new(), cx));
                        if connected || disconnect {
                            this.connect_provider = None;
                            if connected { this.change_source(provider, cx); }
                            this.refresh(cx);
                        } else { this.connect_error = Some("GitHub is not connected. Run gh auth login in a terminal, then check again.".into()); }
                    }
                    Err(error) => this.connect_error = Some(error),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
    fn run_action(&mut self, cx: &mut Context<Self>) {
        if self.acting {
            return;
        }
        let (Some(item), Some(action)) = (self.item(), self.confirm_action.clone()) else {
            return;
        };
        self.acting = true;
        self.action_error = None;
        let service = self.service.clone();
        let key = item.key();
        let merging = matches!(action.as_str(), "merge" | "squash" | "rebase");
        let task = cx
            .background_executor()
            .spawn(async move { service.action(&item, &action) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.acting = false;
                match result {
                    Ok(item) => {
                        this.action_notice = (merging
                            && !item.state.eq_ignore_ascii_case("merged"))
                        .then(|| "Merge queued or auto-merge enabled.".into());
                        this.replace_item(&key, item);
                        this.confirm_action = None;
                        this.load_detail(false, cx);
                        this.refresh(cx);
                    }
                    Err(error) => this.action_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
