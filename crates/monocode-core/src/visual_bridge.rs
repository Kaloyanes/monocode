//! Ephemeral, session-scoped publication of native visuals and images. No user
//! MCP settings are changed. Stable provider capabilities can publish only during
//! a live turn.
use crate::visual::{self, VisualDocument};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const MAX_WIRE_BYTES: usize = 1_048_576;
/// A draft request carries its page with images embedded.
const MAX_DRAFT_BYTES: usize = draft::images::MAX_HTML_BYTES + MAX_WIRE_BYTES;
const MAX_SOURCE_BYTES: usize = visual::MAX_SOURCE_BYTES;
const ENV_ENDPOINT: &str = "MONOCODE_VISUAL_ENDPOINT";
const ENV_TOKEN: &str = "MONOCODE_VISUAL_TOKEN";
const ENV_SESSION: &str = "MONOCODE_VISUAL_SESSION";
const ENV_EXECUTABLE: &str = "MONOCODE_VISUAL_EXECUTABLE";
mod draft;
pub use draft::shared::{Outbox as DraftOutbox, Pin as DraftPin, SharedDraft, Side as DraftSide};
pub use draft::{
    csp_sources, local_url, staging_dir as draft_staging_dir, Anchor as DraftAnchor,
    Diagnostics as DraftDiagnostics, DraftAction, DraftContent, DraftReply, DraftRequest,
    Element as DraftElement, Feedback as DraftFeedback, Layout as DraftLayout,
    LogEntry as DraftLog, Snapshot as DraftSnapshot, DEFAULT_HEIGHT as DRAFT_DEFAULT_HEIGHT,
    DEFAULT_WAIT_SECONDS as DRAFT_DEFAULT_WAIT_SECONDS, DEFAULT_WIDTH as DRAFT_DEFAULT_WIDTH,
    FONT_HOSTS, LAYOUT_WIDTHS as DRAFT_LAYOUT_WIDTHS, SCRIPT_HOSTS, STYLE_HOSTS,
};

type DraftHandler = Box<dyn Fn(DraftRequest) + Send + Sync>;
static DRAFT_HANDLER: OnceLock<DraftHandler> = OnceLock::new();

/// The app registers its draft host once at startup; requests from every
/// session's MCP server arrive here, off the main thread.
pub fn set_draft_handler(handler: impl Fn(DraftRequest) + Send + Sync + 'static) {
    let _ = DRAFT_HANDLER.set(Box::new(handler));
}

const MAX_DRAFT_RESPONSE_BYTES: usize = 24 * 1024 * 1024;

/// The MCP server's name, so its tools reach the model as `mcp__monocode__*`.
pub const SERVER_NAME: &str = "monocode";
/// Records the agent's plan in the transcript, where the title bar's plan rail
/// reads it. It changes nothing, so a provider's approval prompt skips it.
pub const PLAN_TOOL: &str = "update_plan";
const SERVER_ENV: [&str; 10] = [
    ENV_ENDPOINT,
    ENV_TOKEN,
    ENV_SESSION,
    ENV_EXECUTABLE,
    "MONOCODE_APP_ENDPOINT",
    "MONOCODE_APP_TOKEN",
    "MONOCODE_APP_SESSION",
    "MONOCODE_THREADS",
    crate::orchestration::ENV,
    crate::device::ENV,
];

/// Every harness's own todo tool feeds the rail and is always loaded, while
/// Claude Code hides update_plan behind tool search, so name the built-in ones.
const PLAN_INSTRUCTIONS: &str = "The user follows your progress on a plan rail drawn from your todo list. For work with three or more steps, write the list before you start, with your own todo tool (TodoWrite or TaskCreate/TaskUpdate in Claude Code, update_plan in Codex) or else update_plan. Keep one step in progress, update the list the moment a step starts or finishes, and end with every step completed or cancelled.";
/// Says when to reach for a draft; how to build one stays in visual_docs.
pub(crate) const DRAFT_INSTRUCTIONS: &str = "When seeing or trying something would help the person more than prose (a UI idea, an animation, a layout, a diagram, a small tool), build it with draft_preview: a live page they can use and mark up once you publish it (publish:true). Design it around what they want to judge, including whatever controls that needs, such as states to flip between, slow motion or values to tune. The page is part of your reply, so don't describe it again in text. To mock MonoCode or the user's own software, read visual_docs app first and draw the real window; otherwise be creative.";
/// Harnesses ship their own subagent tools, which run out of the user's sight
/// and only on that harness. Claude Code also defers MCP tools to a name until
/// searched, so the server instructions and each harness's own system prompt
/// (see [`harness_instructions`]) are where an agent learns that MonoCode starts
/// subagents, and how that differs from opening a new thread for the user.
pub(crate) const THREAD_INSTRUCTIONS: &str = "MonoCode runs your subagents. Whenever you would hand work to a subagent or another agent, and whenever the user asks for one on another harness or model (Codex, Claude, Haiku and so on), start it with thread_start and collect its reply with thread_wait. Each runs as its own chat the user can watch in MonoCode. Use thread_start instead of your built-in subagent or task tool, and do not run agent CLIs from the shell. A subagent works for you and reports back. When the user asks for a new thread or chat instead, open it with new_thread: it is theirs, so you don't send to it or wait on it. If you can't tell which they mean, ask.";
/// What the server tells every agent at startup.
fn server_instructions(environment: &HashMap<String, String>) -> String {
    let mut instructions = format!("{PLAN_INSTRUCTIONS}\n\n{DRAFT_INSTRUCTIONS}");
    if threads_enabled(environment) {
        instructions.push_str("\n\n");
        instructions.push_str(THREAD_INSTRUCTIONS);
    }
    if devices_enabled(environment) {
        instructions.push_str("\n\n");
        instructions.push_str(DEVICE_INSTRUCTIONS);
    }
    instructions
}
const DEVICE_INSTRUCTIONS: &str = "To run or test an app on an iOS Simulator or Android Emulator, call device_open: the user watches the device in MonoCode while you drive it with the agent-device command it returns. Change a simulator's appearance, text size, accessibility settings or location with device_set.";
fn devices_enabled(environment: &HashMap<String, String>) -> bool {
    environment
        .get(crate::device::ENV)
        .is_some_and(|v| v == "1")
}
fn threads_enabled(environment: &HashMap<String, String>) -> bool {
    environment
        .get("MONOCODE_THREADS")
        .is_some_and(|v| v == "1")
}
/// What a harness adds to its own system prompt, which outranks MCP server
/// instructions: keep the plan rail current in every chat and, in the chat the
/// user talks to, draft when showing beats explaining and start subagents
/// through MonoCode.
pub fn harness_instructions(environment: &HashMap<String, String>) -> Option<String> {
    if !configured(environment) {
        return None;
    }
    Some(if lead_chat(environment) {
        format!("{PLAN_INSTRUCTIONS}\n\n{DRAFT_INSTRUCTIONS}\n\n{THREAD_INSTRUCTIONS}")
    } else {
        PLAN_INSTRUCTIONS.to_string()
    })
}
/// The chat the user talks to, which can start MonoCode threads.
pub fn lead_chat(environment: &HashMap<String, String>) -> bool {
    configured(environment) && threads_enabled(environment)
}
/// Claude Code shows these in full rather than as a name behind tool search,
/// so in a lead chat they compete on equal terms with its own subagent tool.
const ALWAYS_LOADED: [&str; 4] = ["thread_start", "thread_wait", NEW_THREAD, "draft_preview"];
/// Opens a top-level thread the user owns, unlike thread_start's children.
const NEW_THREAD: &str = "new_thread";
const MAX_PLAN_STEPS: usize = 24;
const MAX_STEP_CHARS: usize = 160;

/// Loaded through visual_docs or the CLI only when authoring a visual. Keep the
/// SDK out of ordinary prompts, server instructions and tool descriptions; only
/// the short USAGE trigger is part of the system prompt.
const DOCUMENTATION: &str = r#"Native session visuals
Use visual_check to validate a visual, then visual_show to publish it into this
conversation. They accept {source}; visual_show also accepts {title?,id?}.
The source is a JavaScript function body with a `visual` SDK. Always return a render function. Use closure state for interactions. Example:
const state = visual.state({count: 0});
return () => visual.column([
  visual.text('Counter', {size: 'title'}),
  visual.stat('Count', String(state.count)),
  visual.button('Add', () => { state.count += 1; })
], {gap: 12});

Answers that tell a story: publish one small visual per idea (a single chart,
a stat row, a gauge) rather than one dashboard holding everything. Each
visual_show adds a card to the conversation, titled with the title you give it.
Mention what it shows in your reply, but do not restate what the visual already
shows. Choose short, readable ids.

Native components:
visual.column(children,{gap?}), row(children,{gap?}), grid(children,{columns?,gap?})
visual.card(children,{gap?}) groups a chart or stats on a raised panel.
visual.text(text,{size?,color?})
visual.stat(label,value,{detail?,delta?,trend?,better?,color?}): delta is a signed
change such as '+12.4%'; trend is an array of numbers drawn as a sparkline;
better:'lower' colors a falling delta as good (latency, errors, cost).
visual.table(columns,rows)
visual.chart('bar'|'line'|'area',[{label,values,color?}],{labels?,height?})
A single-series bar chart labels its latest bar; give each series its own color.
visual.gauge(label,value,{min?,max?,unit?,color?}) for a reading against a range.
visual.donut([{label,value,color?}],{label?,value?}) for parts of a whole;
label and value are written in the middle (default: the total).
visual.heatmap(rows,{rows?,columns?,color?}): rows is an array of equal-length
number arrays, such as weekdays by weeks; labels are optional.
visual.progress(label,value,{max?,detail?,color?})
visual.button(label,onClick,{disabled?}), toggle(label,value,onChange)
visual.slider(label,value,min,max,onChange,{step?}), tabs(labels,selected,onChange)
visual.input(label,value,onChange,{placeholder?}), select(label,options,selectedIndex,onChange)
Input handlers receive a string (up to 4096 bytes); select handlers receive an index.
Select and input labels are not shown: make options and placeholders self-explanatory.
visual.canvas(shapes,{width?,height?})
Canvas shapes: visual.rect(x,y,w,h,{color?}), circle(x,y,r,{color?}),
line(x1,y1,x2,y2,{color?,width?}), polyline([[x,y],...],{color?,width?}),
label(x,y,text,{color?}).
visual.scene(objects,{width?,height?,camera?})
Scene objects: visual.object({kind:'box'|'sphere'|'cylinder'|'cone'|'plane'},
{position:[x,y,z],rotation:[x,y,z],scale:[x,y,z],color:'accent'}).
Custom geometry: {kind:'mesh',vertices:[[x,y,z],...],indices:[[a,b,c],...]}
or {kind:'line',points:[[x,y,z],...],width:1}. Rotations are radians.
Camera: {eye:[5,4,6],target:[0,0,0],fov:45}. Use controls and state to change
geometry or camera; every handler rerenders. State is local to this visual.

Style is inherited from the app. Use semantic colors only: ink, muted, faint,
accent, positive, negative, warning, series1 through series6. Text sizes are
caption, small, body, subtitle, title, hero. Choose clear labels and restrained
headings, use row/column/grid layouts, and allow the host width to determine
layout. Use gaps around 8–16 and chart/canvas/scene heights around 220–360.
The same visual also runs on the person's phone, about 360 points wide: rows
and grids wrap there, so keep each row to a few short items, and keep tables to
a few columns.
Do not draw custom buttons, simulate a whole webpage, add a page background, or
hardcode font families or CSS. Native controls and the app theme own those styles.
Include units, chart legends, readable labels, and useful empty states. Keep
supporting prose in the conversation.
The sandbox has no DOM, network, files, timers, imports, or external libraries.
No HTML, SVG markup, or shader code. Source is limited to 64 KiB; keep
scripts compact. Check validates execution, the initial component tree, and representative controls;
it does not capture a screenshot. Fix errors before publishing. Set a concise
title for the published visual. Never expose capability environment variables.
"#;

/// The system-prompt rule for when to reach for a visual. Only the trigger
/// lives here; the SDK stays behind visual_docs.
const USAGE: &str = "### Native visuals\n\nAnswer data with a trend, comparison or breakdown with an inline visual, and show any chart, diagram or visualizer the user asks for the same way: read `visual_docs` once, then `visual_check` and `visual_show`. Never start a server, open a browser or write an HTML file to show it. Ordinary coding answers need no visual.";

/// Usage guidance for a provider that received the visual MCP server. Claude
/// Code may defer MCP tools behind ToolSearch, so it is told how to load them.
pub fn instructions(environment: &HashMap<String, String>, deferred: bool) -> Option<String> {
    configured(environment).then(|| {
        if deferred {
            format!("{USAGE} The monocode_visual tools may be deferred: load them with ToolSearch before you call them.")
        } else {
            USAGE.to_owned()
        }
    })
}

/// The explicit composer command requires a publication even when the ordinary
/// automatic trigger would leave the answer in prose. Keep the SDK on demand.
pub fn requested_prompt(request: &str) -> String {
    let request = if request.trim().is_empty() {
        "Visualize the current topic using the conversation so far."
    } else {
        request
    };
    format!(
        "Create and publish at least one native inline visual in this conversation during this turn. The user invoked the visualization command, so a visual is required even if you would normally answer only in prose.\n\n\
         Read `visual_docs`, build the visual, run `visual_check`, fix any errors, and call `visual_show` before finishing. The monocode_visual MCP tools may be deferred; load them with ToolSearch if needed. If these tools are unavailable, use the executable in MONOCODE_VISUAL_EXECUTABLE: run `\"$MONOCODE_VISUAL_EXECUTABLE\" visual docs`, write the JavaScript source to a file, then run `\"$MONOCODE_VISUAL_EXECUTABLE\" visual check --input FILE` and `\"$MONOCODE_VISUAL_EXECUTABLE\" visual show --input FILE --title TITLE`.\n\n\
         Use the native visual SDK, never a server, browser or HTML file. Include the embed line returned by visual_show in your reply, alongside concise supporting prose. If publication fails, explain the failure; do not claim a visual was shown. Never print capability environment variables.\n\n\
         User request:\n{request}"
    )
}

/// Claude permission rule that pre-approves every visual tool. Publishing
/// only affects this conversation, so read-only and plan turns may use it.
pub const CLAUDE_ALLOWED_TOOLS: &str = "mcp__monocode_visual";

#[derive(Serialize, Deserialize)]
struct Wire {
    session: i64,
    token: String,
    request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    document: Option<VisualDocument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<ImageRequest>,
}

/// An image file the agent asked to show. The app copies it into the
/// conversation's folder; the original is never moved or changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRequest {
    pub path: std::path::PathBuf,
    pub title: String,
    /// The HTML page this picture is a snapshot of, kept beside it so the
    /// conversation can reopen the draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<std::path::PathBuf>,
}

/// The transcript role of a draft's card.
pub const DRAFT_ROLE: &str = "draft";

/// A live draft's place in the conversation. The page itself lives in the
/// app while the draft is open; the card only names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftCard {
    pub draft: String,
    pub title: String,
}

/// Where a kept draft's picture belongs: on the card the draft already has,
/// not in a second card beside it. `rows` is the source of one conversation
/// (or one turn of it); the result lists `(card, picture)` pairs by the index
/// of each message's first row. The `draft_keep` call that precedes a picture
/// names the draft it was made from, so no title is guessed.
pub fn kept_upgrades(rows: &[crate::transcript::Row]) -> Vec<(usize, usize)> {
    use crate::tool::ToolCall;
    use crate::transcript::images::{ImageDocument, ROLE as IMAGE_ROLE};

    let mut cards: Vec<(usize, String)> = Vec::new();
    let mut keeps: Vec<(usize, String)> = Vec::new();
    let mut pictures: Vec<usize> = Vec::new();
    let mut at = 0;
    while at < rows.len() {
        let end = (at + 1..rows.len())
            .find(|&i| rows[i].first)
            .unwrap_or(rows.len());
        let role = rows[at].role.as_str();
        let text = || rows[at..end].iter().map(|r| r.text.as_str()).collect::<String>();
        if role == DRAFT_ROLE {
            if let Ok(card) = serde_json::from_str::<DraftCard>(&text()) {
                cards.push((at, card.draft));
            }
        } else if role == "tool" && rows[at].text.contains("draft_keep") {
            let named = serde_json::from_str::<ToolCall>(&text())
                .ok()
                .filter(|call| call.name.ends_with("draft_keep"))
                .and_then(|call| serde_json::from_str::<Value>(&call.input).ok())
                .and_then(|input| input["draft"].as_str().map(str::to_owned));
            if let Some(draft) = named {
                keeps.push((at, draft));
            }
        } else if role == IMAGE_ROLE
            && serde_json::from_str::<ImageDocument>(&text())
                .is_ok_and(|image| image.draft().is_some())
        {
            pictures.push(at);
        }
        at = end;
    }
    let mut claimed = vec![false; keeps.len()];
    let mut pairs = Vec::new();
    for (n, &picture) in pictures.iter().enumerate() {
        // The call that made it comes first; a provider that reports a call
        // only when it finishes puts it after, before the next picture.
        let next = pictures.get(n + 1).copied().unwrap_or(usize::MAX);
        let keep = (0..keeps.len())
            .rev()
            .find(|&k| !claimed[k] && keeps[k].0 < picture)
            .or_else(|| {
                (0..keeps.len()).find(|&k| !claimed[k] && (picture..next).contains(&keeps[k].0))
            });
        let Some(keep) = keep else { continue };
        claimed[keep] = true;
        let card = cards
            .iter()
            .rev()
            .find(|(card, draft)| *card < picture && *draft == keeps[keep].1);
        if let Some((card, _)) = card {
            pairs.push((*card, picture));
        }
    }
    pairs
}

#[derive(Clone, Debug, Serialize)]
pub enum Content {
    Visual(VisualDocument),
    Image(ImageRequest),
    /// Published by the bridge itself when an agent opens a draft.
    Draft(DraftCard),
}

impl Wire {
    fn content(self) -> Result<Content, String> {
        match (self.document, self.image) {
            (Some(document), None) => Ok(Content::Visual(document)),
            (None, Some(image)) => Ok(Content::Image(image)),
            _ => Err("Invalid visual request".into()),
        }
    }
}

/// The authenticated part of a request, kept after its content moves on.
struct WireHead {
    session: i64,
    token: String,
    request_id: String,
}

pub struct Publication {
    pub content: Content,
    pub reply: mpsc::Sender<Result<Value, String>>,
    generation: u64,
}

struct ActiveTurn {
    generation: u64,
    send: mpsc::SyncSender<Publication>,
    publications: usize,
    /// Drafts this turn started, so a retry with the same title revises
    /// the page it already made instead of adding a second card.
    drafts: Vec<DraftCard>,
}

struct SessionCapability {
    token: String,
    generation: u64,
    active: Option<ActiveTurn>,
    receipts: HashMap<String, (String, Option<Value>)>,
    /// Cards of drafts the agent has not shown the person yet.
    unpublished: Vec<DraftCard>,
}

struct Hub {
    endpoint: String,
    sessions: Mutex<HashMap<i64, SessionCapability>>,
}

/// One process-owned listener serves stable session capabilities. Only the
/// currently attached turn receiver can accept a publication.
fn hub() -> Result<Arc<Hub>, String> {
    static HUB: OnceLock<Mutex<Option<Arc<Hub>>>> = OnceLock::new();
    let mut slot = HUB
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Visual bridge unavailable")?;
    if let Some(hub) = &*slot {
        return Ok(hub.clone());
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let endpoint = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let hub = Arc::new(Hub {
        endpoint,
        sessions: Mutex::new(HashMap::new()),
    });
    let owner = hub.clone();
    let connections = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                break;
            };
            if connections.fetch_add(1, Ordering::Relaxed) >= 16 {
                connections.fetch_sub(1, Ordering::Relaxed);
                continue;
            }
            let owner = owner.clone();
            let connections = connections.clone();
            std::thread::spawn(move || {
                let _ = serve(stream, &owner);
                connections.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });
    *slot = Some(hub.clone());
    Ok(hub)
}

pub struct Server {
    hub: Arc<Hub>,
    token: String,
    session: i64,
    generation: u64,
    publications: mpsc::Receiver<Publication>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.hub.sessions.lock() {
            if let Some(session) = sessions.get_mut(&self.session) {
                if session.generation == self.generation {
                    session.active = None;
                }
            }
        }
    }
}

impl Server {
    pub fn start(session: i64) -> Result<Self, String> {
        let hub = hub()?;
        let (send, publications) = mpsc::sync_channel(16);
        let (token, generation) = {
            let mut sessions = hub
                .sessions
                .lock()
                .map_err(|_| "Visual session unavailable")?;
            let capability = sessions
                .entry(session)
                .or_insert_with(|| SessionCapability {
                    token: format!(
                        "{:032x}{:032x}",
                        rand::random::<u128>(),
                        rand::random::<u128>()
                    ),
                    generation: 0,
                    active: None,
                    receipts: HashMap::new(),
                    unpublished: Vec::new(),
                });
            if capability.active.is_some() {
                return Err("A visual session turn is already active".into());
            }
            capability.generation = capability.generation.wrapping_add(1);
            capability.active = Some(ActiveTurn {
                generation: capability.generation,
                send,
                publications: 0,
                drafts: Vec::new(),
            });
            (capability.token.clone(), capability.generation)
        };
        Ok(Self {
            hub,
            token,
            session,
            generation,
            publications,
        })
    }

    pub fn environment(&self, executable: &std::path::Path) -> HashMap<String, String> {
        HashMap::from([
            (ENV_ENDPOINT.into(), self.hub.endpoint.clone()),
            (ENV_TOKEN.into(), self.token.clone()),
            (ENV_SESSION.into(), self.session.to_string()),
            (
                ENV_EXECUTABLE.into(),
                executable.to_string_lossy().into_owned(),
            ),
        ])
    }

    /// Takes the cards of drafts the agent never showed, for the conversation
    /// to add as the turn ends.
    pub fn unpublished(&self) -> Vec<DraftCard> {
        self.hub
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| {
                sessions
                    .get_mut(&self.session)
                    .map(|session| std::mem::take(&mut session.unpublished))
            })
            .unwrap_or_default()
    }

    pub fn drain(&self) -> Vec<Publication> {
        self.publications
            .try_iter()
            .filter_map(|publication| {
                if publication.generation == self.generation
                    && active_generation(&self.hub, self.session) == Some(self.generation)
                {
                    Some(publication)
                } else {
                    let _ = publication
                        .reply
                        .send(Err("The session turn has ended".into()));
                    None
                }
            })
            .collect()
    }
}

fn valid_document(document: &VisualDocument) -> Result<(), String> {
    if document.source.len() > MAX_SOURCE_BYTES {
        return Err("Visual source exceeds 64 KiB".into());
    }
    if document.id.is_empty()
        || document.id.len() > 96
        || !document
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(
            "Visual id must be 1–96 ASCII letters, numbers, underscores, or hyphens".into(),
        );
    }
    if document.title.chars().count() > 160 {
        return Err("Visual title exceeds 160 characters".into());
    }
    document.validate()?;
    visual::check(&document.source).map(drop)
}

fn valid_image(image: &ImageRequest) -> Result<(), String> {
    if !image.path.is_absolute() {
        return Err("Image path must be absolute".into());
    }
    if image.title.chars().count() > 160 {
        return Err("Image title exceeds 160 characters".into());
    }
    let metadata = std::fs::metadata(&image.path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Image path must be a file".into());
    }
    if metadata.len() > crate::transcript::images::MAX_BYTES {
        return Err("Image is larger than 20 MB".into());
    }
    if let Some(draft) = &image.draft {
        let staged = draft.starts_with(draft::staging_dir())
            && !draft
                .components()
                .any(|c| c == std::path::Component::ParentDir);
        let size = std::fs::metadata(draft)
            .map(|m| m.len())
            .unwrap_or(u64::MAX);
        if !staged || size > draft::images::MAX_HTML_BYTES as u64 {
            return Err("The draft page must be one the app just saved".into());
        }
    }
    let bytes = std::fs::read(&image.path).map_err(|e| e.to_string())?;
    crate::transcript::images::inspect(&bytes).map(|_| ())
}

fn valid_content(content: &Content) -> Result<(), String> {
    match content {
        Content::Visual(document) => valid_document(document),
        Content::Image(image) => valid_image(image),
        Content::Draft(_) => Err("Invalid visual request".into()),
    }
}

fn active_generation(hub: &Hub, session: i64) -> Option<u64> {
    hub.sessions
        .lock()
        .ok()?
        .get(&session)?
        .active
        .as_ref()
        .map(|active| active.generation)
}

/// One draft renders at a time; a draft needs the app, never a session turn.
fn serve_draft(mut stream: TcpStream, hub: &Hub, wire: &Value) -> Result<(), String> {
    /// One request at a time per conversation; a long wait in one must not
    /// hold up another's.
    static BUSY: Mutex<Vec<i64>> = Mutex::new(Vec::new());
    let release = |session: i64| {
        if let Ok(mut busy) = BUSY.lock() {
            busy.retain(|s| *s != session);
        }
    };
    let response = (|| -> Result<Value, String> {
        let session = wire["session"].as_i64().ok_or("Invalid visual session")?;
        authorized(
            hub,
            session,
            wire["token"].as_str().ok_or("Invalid visual capability")?,
        )?;
        let (reply, result) = mpsc::channel();
        let mut request = DraftRequest::parse(session, &wire["draft_request"], reply)?;
        let publish = match wire["draft_request"].get("publish") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(publish)) => *publish,
            Some(_) => return Err("publish must be true or false".into()),
        };
        let handler = DRAFT_HANDLER
            .get()
            .ok_or("Drafts need the MonoCode desktop app")?;
        {
            let mut busy = BUSY.lock().map_err(|_| "Drafts are unavailable")?;
            if busy.contains(&session) {
                return Err("Another draft call is still running; try again in a moment".into());
            }
            busy.push(session);
        }
        // A draft runs out of sight until the agent shows it: with publish,
        // or by asking the person to try it (a wait) or keeping it. Its card
        // joins the conversation only once the page has answered, and the
        // turn's end shows whatever the agent never did.
        let shows = publish
            || matches!(
                request.action,
                DraftAction::Wait { .. } | DraftAction::Keep { .. }
            );
        let closes = matches!(request.action, DraftAction::Close);
        let request_draft = request.draft.clone();
        // A new page titled like one this turn already made is a retry or an
        // iteration of it: revise that draft rather than add a twin card.
        if request.draft.is_none() && request.content.is_some() {
            request.draft = same_turn_draft(hub, session, request.title.as_deref());
        }
        let revises = request_draft.is_none().then(|| request.draft.clone()).flatten();
        let mut created = new_card(session, &mut request);
        let mut hidden = request
            .draft
            .as_deref()
            .and_then(|id| unhide(hub, session, id, request.title.as_deref(), shows || closes));
        // The person answers a wait, so its card comes before it.
        if matches!(request.action, DraftAction::Wait { .. }) {
            if let Some(card) = hidden.take() {
                if let Err(error) = publish_card(hub, session, card.clone()) {
                    hide(hub, session, card);
                    release(session);
                    return Err(error);
                }
            }
        }
        handler(request);
        let mut reply = result.recv_timeout(Duration::from_secs(45));
        // The person may have closed the page this call would have revised,
        // though the agent never named it: start a fresh draft instead.
        if let Some(gone) = revises.as_deref().filter(|_| {
            matches!(&reply, Ok(Err(error)) if error.starts_with(DRAFT_GONE))
        }) {
            forget_draft(hub, session, Some(gone));
            unhide(hub, session, gone, None, true);
            hidden = None;
            let (again_reply, again_result) = mpsc::channel();
            reply = match DraftRequest::parse(session, &wire["draft_request"], again_reply) {
                Ok(mut again) => {
                    created = new_card(session, &mut again);
                    handler(again);
                    again_result.recv_timeout(Duration::from_secs(45))
                }
                Err(error) => Ok(Err(error)),
            };
        }
        release(session);
        let reply = reply
            .map_err(|_| "The draft timed out".to_string())
            .and_then(|reply| reply)
            .and_then(|reply| match created.clone().or_else(|| hidden.clone()) {
                Some(card) if shows => publish_card(hub, session, card).map(|()| reply),
                Some(card) if !closes => {
                    hide(hub, session, card);
                    Ok(reply)
                }
                _ => Ok(reply),
            });
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                // A new page may still be opening; it goes, and nothing of it
                // stays in the conversation or its folder. One the agent had
                // already made stays as it was.
                if let Some(card) = created {
                    handler(DraftRequest::close(session, card.draft));
                } else if let Some(card) = hidden {
                    hide(hub, session, card);
                }
                return Err(error);
            }
        };
        if let Some(card) = &created {
            remember_draft(hub, session, card);
        }
        if closes {
            forget_draft(hub, session, request_draft.as_deref());
        }
        Ok(json!({"ok":true,"result":reply.value()}))
    })()
    .unwrap_or_else(|error| json!({"ok": false, "error": error}));
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{response}").map_err(|e| e.to_string())
}

/// What a draft request that carries the page of a draft the app no longer
/// has is refused with: the page was closed or replaced.
pub const DRAFT_GONE: &str = "That draft was closed or replaced";

/// The card of the draft a request starts, if it starts one. The bridge
/// names the draft so the conversation's card can point at it.
fn new_card(session: i64, request: &mut DraftRequest) -> Option<DraftCard> {
    (request.draft.is_none() && request.content.is_some()).then(|| {
        let card = DraftCard {
            draft: format!("draft-{session}-{:08x}", rand::random::<u32>()),
            title: request.title.clone().unwrap_or_default(),
        };
        request.id = Some(card.draft.clone());
        card
    })
}

fn normalized_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The id of the draft this turn already started under `title`, if any.
fn same_turn_draft(hub: &Hub, session: i64, title: Option<&str>) -> Option<String> {
    let title = normalized_title(title?);
    if title.is_empty() {
        return None;
    }
    let sessions = hub.sessions.lock().ok()?;
    let active = sessions.get(&session)?.active.as_ref()?;
    active
        .drafts
        .iter()
        .rev()
        .find(|card| normalized_title(&card.title) == title)
        .map(|card| card.draft.clone())
}

fn remember_draft(hub: &Hub, session: i64, card: &DraftCard) {
    if let Ok(mut sessions) = hub.sessions.lock() {
        if let Some(active) = sessions
            .get_mut(&session)
            .and_then(|capability| capability.active.as_mut())
        {
            active.drafts.push(card.clone());
            if active.drafts.len() > 64 {
                active.drafts.remove(0);
            }
        }
    }
}

fn forget_draft(hub: &Hub, session: i64, id: Option<&str>) {
    if let (Some(id), Ok(mut sessions)) = (id, hub.sessions.lock()) {
        if let Some(active) = sessions
            .get_mut(&session)
            .and_then(|capability| capability.active.as_mut())
        {
            active.drafts.retain(|card| card.draft != id);
        }
    }
}

/// Keeps a draft's card back until the agent shows the draft.
fn hide(hub: &Hub, session: i64, card: DraftCard) {
    if let Ok(mut sessions) = hub.sessions.lock() {
        if let Some(capability) = sessions.get_mut(&session) {
            let unpublished = &mut capability.unpublished;
            unpublished.retain(|held| held.draft != card.draft);
            unpublished.push(card);
            if unpublished.len() > 64 {
                unpublished.remove(0);
            }
        }
    }
}

/// The card of draft `id` if the person has not seen it yet: renamed to a
/// new `title`, and taken out of hiding when `take` is set.
fn unhide(hub: &Hub, session: i64, id: &str, title: Option<&str>, take: bool) -> Option<DraftCard> {
    let mut sessions = hub.sessions.lock().ok()?;
    let unpublished = &mut sessions.get_mut(&session)?.unpublished;
    let at = unpublished.iter().position(|card| card.draft == id)?;
    if let Some(title) = title {
        unpublished[at].title = title.to_owned();
    }
    take.then(|| unpublished.remove(at))
}

/// Puts a draft's card into the session's running turn and waits until the
/// conversation has it.
fn publish_card(hub: &Hub, session: i64, card: DraftCard) -> Result<(), String> {
    let (reply, result) = mpsc::channel();
    let generation = {
        let mut sessions = hub
            .sessions
            .lock()
            .map_err(|_| "Visual session unavailable")?;
        let active = sessions
            .get_mut(&session)
            .and_then(|capability| capability.active.as_mut())
            .ok_or("Drafts open inside a conversation turn, and this session has none running")?;
        if active.publications >= 64 {
            return Err("This turn has reached its visual publication limit".into());
        }
        active
            .send
            .try_send(Publication {
                content: Content::Draft(card),
                reply,
                generation: active.generation,
            })
            .map_err(|_| "Visual publication queue unavailable")?;
        active.publications += 1;
        active.generation
    };
    let started = Instant::now();
    loop {
        if active_generation(hub, session) != Some(generation) {
            return Err("The session turn has ended".into());
        }
        match result.recv_timeout(Duration::from_millis(25)) {
            Ok(result) => return result.map(drop),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("The draft's card was not saved".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) if started.elapsed() < Duration::from_secs(15) => {
            }
            Err(_) => return Err("The draft's card timed out".into()),
        }
    }
}

fn authorized(hub: &Hub, session: i64, token: &str) -> Result<(), String> {
    let sessions = hub
        .sessions
        .lock()
        .map_err(|_| "Visual session unavailable")?;
    let secret = &sessions
        .get(&session)
        .ok_or("Invalid visual capability")?
        .token;
    let equal = token.len() == secret.len()
        && token
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |diff, (a, b)| diff | a ^ b)
            == 0;
    if equal {
        Ok(())
    } else {
        Err("Invalid visual capability".into())
    }
}

fn serve(mut stream: TcpStream, hub: &Hub) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new((&stream).take((MAX_DRAFT_BYTES + 1) as u64))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if line.len() > MAX_DRAFT_BYTES || !line.ends_with('\n') {
        return Err("Invalid visual request".into());
    }
    if let Ok(value) = serde_json::from_str::<Value>(&line) {
        if value.get("draft_request").is_some() {
            return serve_draft(stream, hub, &value);
        }
    }
    if line.len() > MAX_WIRE_BYTES {
        return Err("Invalid visual request".into());
    }
    let wire: Wire = serde_json::from_str(&line).map_err(|_| "Invalid visual request")?;
    let (session, token, request_id) = (wire.session, wire.token.clone(), wire.request_id.clone());
    let content = wire.content();
    let wire = WireHead {
        session,
        token,
        request_id,
    };
    // Authenticate and capture the active generation before executing source.
    let generation = {
        let sessions = hub
            .sessions
            .lock()
            .map_err(|_| "Visual session unavailable")?;
        let capability = sessions
            .get(&wire.session)
            .ok_or("Invalid visual capability")?;
        let secret = &capability.token;
        let equal = wire.token.len() == secret.len()
            && wire
                .token
                .bytes()
                .zip(secret.bytes())
                .fold(0u8, |diff, (a, b)| diff | a ^ b)
                == 0;
        if !equal {
            return Err("Invalid visual capability".into());
        }
        capability.active.as_ref().map(|active| active.generation)
    };
    let response = (|| -> Result<Value, String> {
        if wire.request_id.is_empty() || wire.request_id.len() > 128 {
            return Err("Invalid request id".into());
        }
        let content = content?;
        let signature = Sha256::digest(serde_json::to_vec(&content).map_err(|e| e.to_string())?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        {
            let sessions = hub
                .sessions
                .lock()
                .map_err(|_| "Visual session unavailable")?;
            let capability = sessions
                .get(&wire.session)
                .ok_or("Invalid visual capability")?;
            if let Some((previous, result)) = capability.receipts.get(&wire.request_id) {
                if previous != &signature {
                    return Err("Request id already used for another visual".into());
                }
                return result.clone().ok_or_else(|| {
                    "Visual publication is still in progress; retry the same request id".into()
                });
            }
        }
        let generation = generation.ok_or("Visual publication requires an active session turn")?;
        valid_content(&content)?;
        let (reply, result) = mpsc::channel();
        {
            let mut sessions = hub
                .sessions
                .lock()
                .map_err(|_| "Visual session unavailable")?;
            let capability = sessions
                .get_mut(&wire.session)
                .ok_or("Invalid visual capability")?;
            let active = capability
                .active
                .as_mut()
                .filter(|active| active.generation == generation)
                .ok_or("The session turn has ended")?;
            // A concurrent retry may have reserved this request during validation.
            if capability.receipts.contains_key(&wire.request_id) {
                return Err("Visual publication is in progress; retry the same request id".into());
            }
            if active.publications >= 64 {
                return Err("This turn has reached its visual publication limit".into());
            }
            // Bound remembered receipts across a long-lived session. Pending
            // requests are never evicted; older settled receipt ids may expire.
            if capability.receipts.len() >= 512 {
                let expired = capability
                    .receipts
                    .iter()
                    .find(|(_, (_, receipt))| receipt.is_some())
                    .map(|(key, _)| key.clone());
                if let Some(expired) = expired {
                    capability.receipts.remove(&expired);
                } else {
                    return Err("Visual receipt capacity reached".into());
                }
            }
            active
                .send
                .try_send(Publication {
                    content,
                    reply,
                    generation,
                })
                .map_err(|_| "Visual publication queue unavailable")?;
            active.publications += 1;
            capability
                .receipts
                .insert(wire.request_id.clone(), (signature, None));
        }
        let started = Instant::now();
        let result = loop {
            if active_generation(hub, wire.session) != Some(generation) {
                break Err("The session turn has ended".into());
            }
            match result.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    if active_generation(hub, wire.session) != Some(generation) {
                        break Err("The session turn has ended".into());
                    }
                    break result;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err("Visual publication was not saved".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout)
                    if started.elapsed() < Duration::from_secs(30) => {}
                Err(_) => break Err("Visual publication timed out".into()),
            }
        };
        let value = match result {
            Ok(value) => json!({"ok":true,"result":value,"requestId":wire.request_id}),
            Err(error) => json!({"ok":false,"error":error,"requestId":wire.request_id}),
        };
        if let Ok(mut sessions) = hub.sessions.lock() {
            if let Some(capability) = sessions.get_mut(&wire.session) {
                if let Some((_, receipt)) = capability.receipts.get_mut(&wire.request_id) {
                    *receipt = Some(value.clone());
                }
            }
        }
        Ok(value)
    })()
    .unwrap_or_else(|error| json!({"ok":false,"error":error,"requestId":wire.request_id}));
    writeln!(stream, "{response}").map_err(|e| e.to_string())
}

/// Validates a native visual's `{source}`.
pub fn check_input(input: &Value) -> Result<Value, String> {
    check(
        input["source"]
            .as_str()
            .ok_or("source must be a JavaScript string")?,
    )
}

/// Local validation never requires an active session capability.
pub fn check(source: &str) -> Result<Value, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Visual source exceeds 64 KiB".into());
    }
    let tree = visual::check(source)?;
    Ok(json!({"ok":true,"tree":tree}))
}

fn publish_with_environment(
    input: Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    if !input.is_object() {
        return Err("Visual input must be an object".into());
    }
    for field in ["title", "id", "requestId"] {
        if input.get(field).is_some_and(|value| !value.is_string()) {
            return Err(format!("{field} must be a string"));
        }
    }
    let source = input["source"]
        .as_str()
        .ok_or("source must be a JavaScript string")?;
    let request_id = input["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
    if request_id.is_empty() || request_id.len() > 128 {
        return Err("Request id must contain 1–128 bytes".into());
    }
    let document_id = Sha256::digest(request_id.as_bytes())
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let document = VisualDocument {
        id: input["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("visual-{document_id}")),
        title: input["title"].as_str().unwrap_or("Visual").to_owned(),
        source: source.to_owned(),
    };
    valid_document(&document)?;
    send(Content::Visual(document), request_id, environment)
}

fn publish_image_with_environment(
    input: Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    if !input.is_object() {
        return Err("Image input must be an object".into());
    }
    for field in ["title", "requestId"] {
        if input.get(field).is_some_and(|value| !value.is_string()) {
            return Err(format!("{field} must be a string"));
        }
    }
    let path = input["path"].as_str().ok_or("path must be a string")?;
    // The MCP server runs in the agent's working directory.
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let request_id = input["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
    if request_id.is_empty() || request_id.len() > 128 {
        return Err("Request id must contain 1–128 bytes".into());
    }
    let image = ImageRequest {
        title: input["title"].as_str().unwrap_or("").trim().to_owned(),
        path,
        draft: None,
    };
    valid_image(&image)?;
    send(Content::Image(image), request_id, environment)
}

fn send(
    content: Content,
    request_id: String,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    let get = |key| {
        environment
            .get(key)
            .ok_or("Visual publication requires a running MonoCode session")
    };
    let endpoint: SocketAddr = get(ENV_ENDPOINT)?
        .parse()
        .map_err(|_| "Invalid visual endpoint")?;
    if !endpoint.ip().is_loopback() {
        return Err("Visual endpoint must be loopback".into());
    }
    let wire = Wire {
        session: get(ENV_SESSION)?
            .parse()
            .map_err(|_| "Invalid visual session")?,
        token: get(ENV_TOKEN)?.clone(),
        request_id,
        document: None,
        image: None,
    };
    let wire = match content {
        Content::Visual(document) => Wire {
            document: Some(document),
            ..wire
        },
        Content::Image(image) => Wire {
            image: Some(image),
            ..wire
        },
        Content::Draft(_) => return Err("Invalid visual request".into()),
    };
    let mut stream =
        TcpStream::connect_timeout(&endpoint, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(35)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&wire).map_err(|e| e.to_string())?
    )
    .map_err(|e| e.to_string())?;
    let mut response = String::new();
    BufReader::new(stream.take(MAX_WIRE_BYTES as u64))
        .read_line(&mut response)
        .map_err(|e| e.to_string())?;
    let response: Value =
        serde_json::from_str(&response).map_err(|_| "Visual session closed without a response")?;
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("Visual publication failed")
            .into());
    }
    Ok(response)
}

/// Ask the running app to show or inspect a draft in the conversation.
fn draft_with_environment(
    input: &Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    let mut prepared = input.clone();
    if let Some(html) = input.get("html") {
        let html = draft::images::embed_images(html.as_str().ok_or("html must be a string")?)?;
        prepared["html"] = html.into();
    }
    let (reply, _) = mpsc::channel();
    DraftRequest::parse(0, &prepared, reply)?;
    let get = |key| {
        environment
            .get(key)
            .ok_or("Drafts require a running MonoCode session")
    };
    let endpoint: SocketAddr = get(ENV_ENDPOINT)?
        .parse()
        .map_err(|_| "Invalid visual endpoint")?;
    if !endpoint.ip().is_loopback() {
        return Err("Visual endpoint must be loopback".into());
    }
    let session: i64 = get(ENV_SESSION)?
        .parse()
        .map_err(|_| "Invalid visual session")?;
    let mut stream =
        TcpStream::connect_timeout(&endpoint, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    writeln!(
        stream,
        "{}",
        json!({"session": session, "token": get(ENV_TOKEN)?, "draft_request": prepared})
    )
    .map_err(|e| e.to_string())?;
    let mut response = String::new();
    BufReader::new(stream.take(MAX_DRAFT_RESPONSE_BYTES as u64))
        .read_line(&mut response)
        .map_err(|e| e.to_string())?;
    let response: Value =
        serde_json::from_str(&response).map_err(|_| "The app closed without a draft")?;
    if response["ok"] != true {
        return Err(response["error"].as_str().unwrap_or("Draft failed").into());
    }
    Ok(response["result"].clone())
}

/// A draft's page with its images embedded, as the tools hand it to the app.
pub fn embed_draft_images(html: &str) -> Result<String, String> {
    draft::images::embed_images(html)
}

/// Waits for the person to send feedback from the page or hand the draft
/// back, then looks at the page again.
fn wait_for_draft(input: &Value, environment: &HashMap<String, String>) -> Result<Value, String> {
    let draft = input["draft"].as_str().ok_or("draft must be a string")?;
    let mut action = json!({"kind":"wait"});
    if let Some(seconds) = input.get("seconds").filter(|v| !v.is_null()) {
        action["seconds"] = seconds.clone();
    }
    draft_with_environment(&json!({"draft":draft,"action":action}), environment)
}

/// Saves the draft as a picture card in the conversation, with its page kept
/// beside it so the card can reopen the draft.
fn keep_draft(input: &Value, environment: &HashMap<String, String>) -> Result<Value, String> {
    let draft = input["draft"].as_str().ok_or("draft must be a string")?;
    let title = input["title"].as_str().unwrap_or("").trim();
    let kept = draft_with_environment(
        &json!({"draft":draft,"action":{"kind":"keep","title":title}}),
        environment,
    )?;
    let path = |key: &str| {
        kept[key]
            .as_str()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "The app returned no draft files".to_string())
    };
    let image = ImageRequest {
        title: title.to_owned(),
        path: path("png")?,
        draft: Some(path("html")?),
    };
    valid_image(&image)?;
    let request_id = input["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
    send(Content::Image(image), request_id, environment)
}

fn current_environment() -> HashMap<String, String> {
    SERVER_ENV
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect()
}

pub fn client(args: &[String]) -> Result<Value, String> {
    let Some(action) = args
        .first()
        .filter(|a| !matches!(a.as_str(), "help" | "--help" | "-h"))
    else {
        return Ok(
            json!({"help":format!("Usage: monocode-gpui visual docs\n       monocode-gpui visual draft --json JSON\n       monocode-gpui visual check|show --input FILE [--title TITLE] [--id ID]\n       monocode-gpui visual image --path IMAGE [--title TITLE]\nFILE is JavaScript source, or JSON with source/title/id/requestId. Use - for stdin.\nIMAGE is a PNG, JPEG, GIF or WebP file to show in the conversation.\n{DOCUMENTATION}")}),
        );
    };
    if action == "docs" {
        return match args {
            [_] => Ok(documentation()),
            _ => Err("Usage: monocode-gpui visual docs".into()),
        };
    }
    if action == "draft" {
        let json = match &args[1..] {
            [flag, json] if flag == "--json" => json,
            _ => {
                return Err("Usage: monocode-gpui visual draft --json '{\"html\":\"...\"}'".into());
            }
        };
        let input = serde_json::from_str::<Value>(json).map_err(|e| e.to_string())?;
        return draft_with_environment(&input, &current_environment());
    }
    if action == "image" {
        let mut input = json!({});
        let mut args = args[1..].iter();
        while let Some(arg) = args.next() {
            let field = match arg.as_str() {
                "--path" => "path",
                "--title" => "title",
                "--request-id" => "requestId",
                _ => return Err(format!("Unknown visual image argument: {arg}")),
            };
            input[field] = args
                .next()
                .ok_or(format!("{arg} needs a value"))?
                .clone()
                .into();
        }
        return publish_image_with_environment(input, &current_environment());
    }
    if !matches!(action.as_str(), "check" | "show") {
        return Err("Expected visual docs, visual check, visual show or visual image".into());
    }
    let mut input = None;
    let mut title = None;
    let mut id = None;
    let mut request_id = None;
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" => {
                let path = args.next().ok_or("--input needs a path")?;
                let reader: Box<dyn Read> = if path == "-" {
                    Box::new(std::io::stdin())
                } else {
                    Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?)
                };
                let mut text = String::new();
                reader
                    .take((MAX_WIRE_BYTES + 1) as u64)
                    .read_to_string(&mut text)
                    .map_err(|e| e.to_string())?;
                if text.len() > MAX_WIRE_BYTES {
                    return Err("Visual input exceeds 1 MiB".into());
                }
                input = Some(
                    serde_json::from_str::<Value>(&text)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({"source":text})),
                );
            }
            "--json" => {
                input = Some(
                    serde_json::from_str(args.next().ok_or("--json needs JSON")?)
                        .map_err(|e| e.to_string())?,
                )
            }
            "--title" => title = Some(args.next().ok_or("--title needs a value")?.clone()),
            "--id" => id = Some(args.next().ok_or("--id needs a value")?.clone()),
            "--request-id" => {
                request_id = Some(args.next().ok_or("--request-id needs a value")?.clone())
            }
            _ => return Err(format!("Unknown visual argument: {arg}")),
        }
    }
    let mut input = input.ok_or("Pass --input FILE or --json JSON")?;
    if !input.is_object() {
        return Err("Visual input must be an object".into());
    }
    if let Some(title) = title {
        input["title"] = title.into();
    }
    if let Some(id) = id {
        input["id"] = id.into();
    }
    if let Some(request_id) = request_id {
        input["requestId"] = request_id.into();
    }
    if action == "check" {
        check_input(&input)
    } else {
        publish_with_environment(input, &current_environment())
    }
}

const DRAFT_DOCUMENTATION: &str = include_str!("visual_bridge/reference/draft.md");
const KIT_DOCUMENTATION: &str = include_str!("visual_bridge/reference/kit.md");
const APP_DOCUMENTATION: &str = include_str!("visual_bridge/reference/app.md");

fn documentation_for(input: &Value) -> Result<Value, String> {
    match input.get("topic").and_then(Value::as_str) {
        None | Some("native") => Ok(documentation()),
        Some("draft") => Ok(json!({"documentation": DRAFT_DOCUMENTATION})),
        Some("kit") => Ok(json!({"documentation": KIT_DOCUMENTATION})),
        Some("app") => Ok(json!({"documentation": APP_DOCUMENTATION})),
        Some(_) => Err("topic must be native, draft, kit or app".into()),
    }
}

fn documentation() -> Value {
    json!({
        "documentation": DOCUMENTATION,
        "cli": {
            "check": "monocode-gpui visual check --input FILE",
            "show": "monocode-gpui visual show --input FILE --title TITLE",
            "image": "monocode-gpui visual image --path IMAGE --title TITLE"
        }
    })
}

pub fn configured(environment: &HashMap<String, String>) -> bool {
    [ENV_ENDPOINT, ENV_TOKEN, ENV_SESSION, ENV_EXECUTABLE]
        .iter()
        .all(|key| environment.contains_key(*key))
}

/// Provider-specific adapters use this ephemeral stdio server definition.
pub fn mcp_server(environment: &HashMap<String, String>) -> Option<Value> {
    configured(environment).then(|| {
        json!({
            "type":"stdio", "command":environment[ENV_EXECUTABLE], "args":["visual-mcp"]
        })
    })
}

pub fn acp_servers(environment: &HashMap<String, String>) -> Value {
    let Some(mut server) = mcp_server(environment) else {
        return json!([]);
    };
    // ACP's stdio server shape omits the transport discriminator.
    server.as_object_mut().unwrap().remove("type");
    server["name"] = SERVER_NAME.into();
    server["env"] = SERVER_ENV
        .into_iter()
        .filter_map(|key| {
            environment
                .get(key)
                .map(|value| json!({"name":key,"value":value}))
        })
        .collect::<Vec<_>>()
        .into();
    json!([server])
}

pub fn configure_codex(command: &mut std::process::Command, environment: &HashMap<String, String>) {
    if !configured(environment) {
        return;
    }
    for (key, value) in [
        ("command", json!(environment[ENV_EXECUTABLE])),
        ("args", json!(["visual-mcp"])),
        ("tool_timeout_sec", json!(90)),
        ("env_vars", json!(SERVER_ENV)),
    ] {
        // JSON's strings and arrays are valid TOML literals for these values.
        command.args(["-c", &format!("mcp_servers.{SERVER_NAME}.{key}={value}")]);
    }
    // Codex's bundled visualize plugin asks the model for a content reference
    // only the Codex app can render; in MonoCode it shows up as raw text. An
    // inline table merges into the user's plugin settings, so only this entry
    // changes. (A quoted dotted key is not parsed by Codex's -c.)
    command.args(["-c", CODEX_VISUALIZE_OFF]);
}

const CODEX_VISUALIZE_OFF: &str = r#"plugins={"visualize@openai-bundled"={enabled=false}}"#;

pub fn tool_definitions() -> Vec<Value> {
    let properties = json!({"source":{"type":"string","description":"JavaScript function body using the native visual SDK"}});
    let mut show_properties = properties.clone();
    show_properties["title"] = json!({"type":"string","maxLength":160});
    show_properties["id"] = json!({"type":"string","maxLength":96,"description":"Optional document identifier. A new request appends a new visual; reuse requestId to retry a publication."});
    show_properties["requestId"] = json!({"type":"string","maxLength":128,"description":"Reuse when retrying an uncertain publication"});
    vec![
        json!({"name":"visual_docs","description":"Read the native visual SDK reference, examples and styling rules when preparing an inline visual. Pass topic draft for the design-draft guide, kit for the optional component kit, or app for the real MonoCode design language to mock its UI. Does not publish.","inputSchema":{"type":"object","properties":{"topic":{"enum":["native","draft","kit","app"]}},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"visual_check","description":"Validate a native visual program and representative controls without publishing. Returns its component tree. Read visual_docs for the SDK reference.","inputSchema":{"type":"object","properties":properties,"required":["source"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"visual_show","description":"Publish a native visual into the conversation as its own card. Read visual_docs first; validate with visual_check.","inputSchema":{"type":"object","properties":show_properties,"required":["source"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
        json!({"name":PLAN_TOOL,"description":"Show the user your plan and what you are doing now. Send the whole list each time, one step in_progress. Call when multi-step work starts and on every step change.","inputSchema":{"type":"object","properties":{"explanation":{"type":"string"},"plan":{"type":"array","items":{"type":"object","properties":{"step":{"type":"string"},"status":{"enum":["pending","in_progress","completed","cancelled"]}},"required":["step","status"]}}},"required":["plan"]},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"draft_preview","description":"Build and inspect a live page for the conversation. It runs out of sight while you work; publish:true shows its card at this point in the thread, and a draft never published appears when your turn ends. Pass {html} (inline CSS/JS) or a localhost {url} to open one, with a short {title}; pass {draft} to look again. Returns a screenshot, text, controls with refs, console output and diagnostics; a new page also reports layouts, its height and overflow at phone, tablet and laptop widths. Revise with {draft,html}; resize or retheme with width/height/appearance. Actions: click {reference}, type {text,reference?,replace?} (a ref stays valid while its element is on the page), set {values} (patches the page's mc.state by key, as its own controls would), resume, close. If userControl is true the user took over: inspect, but resume only after they hand it back. Results carry feedback the person sent from the page (pinned notes, and messages the page sent with mc.send), its state and issues the page reveals; to wait for feedback use draft_wait. Read visual_docs {topic:draft} first. Keep a good result with draft_keep.","inputSchema":{"type":"object","properties":{"html":{"type":"string"},"title":{"type":"string","maxLength":160},"url":{"type":"string","description":"A dev server on localhost"},"draft":{"type":"string","maxLength":96},"action":{"type":"object","properties":{"kind":{"enum":["snapshot","click","type","set","resume","close"]},"reference":{"type":"string","maxLength":96},"values":{"type":"object","description":"For set: state keys and their new values"},"text":{"type":"string","maxLength":4096},"replace":{"type":"boolean"}},"required":["kind"],"additionalProperties":false},"width":{"type":"integer","minimum":240,"maximum":1600},"height":{"type":"integer","minimum":120,"maximum":1600,"description":"Only where the card starts; it is as tall as the page"},"appearance":{"enum":["light","dark"]},"publish":{"type":"boolean","description":"Show the draft to the person; later versions then appear in its card as you send them"}},"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":true}}),
        json!({"name":"draft_wait","description":"Pause until the person has tried the draft and sends their feedback (pinned notes on the page) or hands it back, then look at the page again. Returns the usual screenshot and numbers plus feedback, messages, userActions and event (feedback, message, handed back or timeout). Waits up to 30 seconds; call again to keep waiting. Use after asking the person to try it, not in place of draft_preview.","inputSchema":{"type":"object","properties":{"draft":{"type":"string","maxLength":96},"seconds":{"type":"integer","minimum":1,"maximum":30}},"required":["draft"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"draft_keep","description":"Save the current HTML draft as a picture in the conversation that reopens the live page after the draft is gone. Use for a result worth keeping, not every iteration.","inputSchema":{"type":"object","properties":{"draft":{"type":"string","maxLength":96},"title":{"type":"string","maxLength":160},"requestId":{"type":"string","maxLength":128}},"required":["draft","title"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
        json!({"name":"image_show","description":"Show a PNG, JPEG, GIF or WebP file you made, such as a chart or screenshot, in the conversation. The app keeps its own copy.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"title":{"type":"string","maxLength":160},"requestId":{"type":"string","maxLength":128}},"required":["path"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
    ]
}

pub(crate) fn thread_tool_definitions() -> Vec<Value> {
    ["start", "send", "wait", "read", "stop", "close", "list"].into_iter().map(|action| {
        let mut properties = json!({"requestId":{"type":"string","maxLength":128,"description":"Reuse for an uncertain retry with identical input"}});
        let required = match action {
            "start" => {
                properties["prompt"] = json!({"type":"string","minLength":1,"maxLength":240000});
                for key in ["model", "effort", "title"] { properties[key] = json!({"type":"string"}); }
                let harnesses: Vec<&str> = crate::provider::Kind::ALL.iter().map(|kind| kind.id()).collect();
                properties["harness"] = json!({"type":"string","description":format!("One of {}. Defaults to yours.", harnesses.join(", "))});
                properties["model"] = json!({"type":"string","description":"The harness's model id or alias, such as haiku for claude"});
                properties["modelSettings"] = json!({"type":"object","additionalProperties":{"type":"string"}});
                properties["workspaceMode"] = json!({"enum":["current","worktree"]});
                vec!["prompt"]
            }
            "list" => vec![],
            _ => {
                properties["threadId"] = json!({"type":"integer","minimum":1});
                match action {
                    "send" => { properties["prompt"] = json!({"type":"string","minLength":1,"maxLength":240000}); }
                    "wait" => { properties["timeoutSeconds"] = json!({"type":"integer","minimum":0,"maximum":50,"default":50}); }
                    "read" => {
                        properties["before"] = json!({"type":["string","integer"]});
                        properties["limit"] = json!({"type":"integer","minimum":1,"maximum":3});
                        properties["maxChars"] = json!({"type":"integer","minimum":200,"maximum":6000});
                    }
                    _ => (),
                }
                if action == "send" { vec!["threadId","prompt"] } else { vec!["threadId"] }
            }
        };
        let detail = match action {
            "start" => "Start a subagent: a child chat on any harness and model that works for you and reports back, which the user can watch in MonoCode. Use it instead of a built-in subagent tool or an agent CLI. When the user asks for a new thread or chat of their own, use new_thread instead.",
            "send" => "Send a follow-up to an idle child; fails while it is busy.",
            "wait" => "Wait up to 50 seconds. Returns idle with success and the final reply, working, or needsInput for a user approval or question. Tell the user when input is needed.",
            "read" => "Read recent prose exchanges with an owned child, using nextBefore for older turns.",
            "stop" => "Stop an owned child.",
            "close" => "Close a finished child: archive its chat and take it off your list and the user's sidebar. Fails while it is working or needs input; stop it first. Close children you no longer need.",
            _ => "List your own children and their current status.",
        };
        json!({"name":format!("thread_{action}"),"description":format!("{detail} Start/send return at once; call thread_wait until status is idle. Children share your checkout unless workspaceMode is worktree. Children cannot spawn or message other threads."),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":matches!(action,"wait"|"read"|"list"),"destructiveHint":false,"openWorldHint":false}})
    }).chain(std::iter::once(new_thread_definition())).collect()
}

fn new_thread_definition() -> Value {
    let harnesses: Vec<&str> = crate::provider::Kind::ALL
        .iter()
        .map(|kind| kind.id())
        .collect();
    let properties = json!({
        "prompt":{"type":"string","minLength":1,"maxLength":240000,"description":"The thread's first message"},
        "title":{"type":"string","maxLength":160},
        "harness":{"type":"string","description":format!("One of {}. Defaults to yours.", harnesses.join(", "))},
        "model":{"type":"string","description":"The harness's model id or alias, such as haiku for claude"},
        "effort":{"type":"string"},
        "modelSettings":{"type":"object","additionalProperties":{"type":"string"}},
        "workspaceMode":{"enum":["current","worktree"]},
        "draft":{"type":"boolean","description":"Leave the prompt in the new thread's composer for the user to send"},
        "reveal":{"type":"boolean","description":"Open the new thread for the user now"},
        "requestId":{"type":"string","maxLength":128,"description":"Reuse for an uncertain retry with identical input"},
    });
    json!({"name":NEW_THREAD,"description":"Open a new thread for the user: a top-level chat in this project's sidebar that the user owns and continues. Use it when the user asks for a new thread, chat or conversation. It is not a subagent: you don't send to it, wait on it or get its reply. Tell the user it is open.","inputSchema":{"type":"object","properties":properties,"required":["prompt"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}})
}

fn device_tool_definitions() -> Vec<Value> {
    let device = json!({"type":"string","description":"A device id from device_list, or its name such as \"iPhone 17 Pro\" or \"Pixel 9\""});
    let tool = |name: &str,
                description: &str,
                properties: Value,
                required: Vec<&str>,
                read_only: bool| {
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read_only,"destructiveHint":false,"openWorldHint":false}})
    };
    vec![
        tool("device_list", "List the iOS Simulators and Android Emulators on this Mac, booted first.", json!({}), vec![], true),
        tool("device_open", "Boot a simulator or emulator if needed and show it in the user's Device panel. Returns the agent-device command and the exact arguments that drive this device (tap, type, swipe, snapshot of the accessibility tree); run it from your shell.", json!({"device":device,"platform":{"enum":["ios","android"]}}), vec!["device"], false),
        tool("device_screenshot", "Take a screenshot of a device, by default the one in the Device panel.", json!({"device":device}), vec![], true),
        tool("device_close", "Shut a simulator or emulator down.", json!({"device":device}), vec!["device"], false),
        tool("device_set", "Change an iOS Simulator's setting or simulated location, by default on the one in the Device panel. The user sees the change listed under the phone, marked as yours, and can put it back; it lasts until reset, so set it back when you are done unless the user wants to keep it. Values: appearance light|dark; text-size extra-small … accessibility-extra-extra-extra-large, or XS, S, M, L (the default), XL, XXL, XXXL, AX1 … AX5; color-filter none|grayscale|red-green|green-red|blue-yellow; liquid-glass clear|tinted; reduce-motion, increase-contrast, reduce-transparency, voiceover and show-borders on|off; location \"latitude,longitude\" or clear.", json!({"device":device,"setting":{"enum":crate::device::SETTINGS},"value":{"type":"string"}}), vec!["setting", "value"], false),
    ]
}

fn call_device_tool(
    name: &str,
    input: Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    if !devices_enabled(environment) {
        return Err("Device tools are turned off in Settings › Devices".into());
    }
    let action = format!(
        "devices.{}",
        name.strip_prefix("device_").ok_or("Unknown device tool")?
    );
    crate::device::validate(&action, &input)?;
    let response = crate::mono::bridge::call(&action, input, &crate::mono::new_id(), environment)?;
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("Device request failed")
            .to_owned());
    }
    let mut value = response["result"].clone();
    if action == "devices.screenshot" {
        // The app saves the picture; it comes back here as an image block
        // rather than megabytes of base64 across the bridge.
        let path = value["path"].as_str().ok_or("The app sent no screenshot")?;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        use base64::Engine as _;
        value["png"] = base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .into();
    }
    Ok(value)
}

fn call_thread_tool(
    action: &str,
    mut input: Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    if !threads_enabled(environment) {
        return Err("Thread tools are only available to top-level threads".into());
    }
    let request = crate::app_access::optional_text(&input, "requestId", 128)?
        .unwrap_or_else(crate::mono::new_id);
    input
        .as_object_mut()
        .ok_or("Input must be an object")?
        .remove("requestId");
    let action = format!("threads.{action}");
    crate::app_access::validate_threads(&action, &input)?;
    let response = crate::mono::bridge::call(&action, input, &request, environment)?;
    if response["ok"] == true {
        Ok(response["result"].clone())
    } else {
        Err(response["error"]
            .as_str()
            .unwrap_or("Thread request failed")
            .to_owned())
    }
}

pub fn read_only_tool(name: &str) -> bool {
    matches!(
        name.strip_prefix("mcp__monocode__")
            .or_else(|| name.strip_prefix("monocode:"))
            .or_else(|| name.strip_prefix("monocode_")),
        Some(
            "update_plan"
                | "thread_wait"
                | "thread_read"
                | "thread_list"
                | "draft_wait"
                | "device_list"
                | "device_screenshot"
                | crate::orchestration::TOOL
        )
    )
}

fn planning_orchestration(environment: &HashMap<String, String>) -> bool {
    environment
        .get(crate::orchestration::ENV)
        .is_some_and(|v| v == "1")
}

/// Like update_plan, the plan lives in the tool call's input, where the app
/// reads it when the turn ends. This only refuses a plan it could not start.
fn check_orchestration_plan(
    input: &Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    if !planning_orchestration(environment) {
        return Err("Orchestration plans can only be submitted in an Orchestrator turn".into());
    }
    let plan = crate::orchestration::from_input(input)?;
    Ok(json!({
        "ok": true,
        "title": plan.title,
        "assignments": plan.tasks.len(),
        "next": "The user reviews and starts the plan in MonoCode. Reply with a short explanation of the approach; do not repeat the plan."
    }))
}

/// The plan itself lives in the tool call's input; this only refuses a list
/// the rail could not show honestly.
fn check_plan(input: &Value) -> Result<Value, String> {
    let steps = input["plan"]
        .as_array()
        .ok_or("plan must be a list of steps")?;
    if steps.is_empty() || steps.len() > MAX_PLAN_STEPS {
        return Err(format!("plan needs 1 to {MAX_PLAN_STEPS} steps"));
    }
    let mut current = 0;
    for step in steps {
        let text = step["step"].as_str().map(str::trim).unwrap_or("");
        if text.is_empty() || text.chars().count() > MAX_STEP_CHARS {
            return Err(format!("each step needs 1 to {MAX_STEP_CHARS} characters"));
        }
        match step["status"].as_str() {
            Some("pending" | "completed" | "cancelled") => {}
            Some("in_progress") => current += 1,
            _ => return Err("status must be pending, in_progress, completed or cancelled".into()),
        }
    }
    if current > 1 {
        return Err("only one step can be in_progress".into());
    }
    Ok(json!({"ok": true, "steps": steps.len()}))
}

fn mcp_response(request: Value, environment: &HashMap<String, String>) -> Option<Value> {
    let id = request.get("id")?.clone();
    let result = match request["method"].as_str().unwrap_or("") {
        "initialize" => Ok(
            json!({"protocolVersion":request["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":SERVER_NAME,"version":env!("CARGO_PKG_VERSION")},"instructions":server_instructions(environment)}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut tools = tool_definitions();
            if threads_enabled(environment) {
                tools.extend(thread_tool_definitions());
            }
            if planning_orchestration(environment) {
                tools.push(crate::orchestration::tool_definition());
            }
            if devices_enabled(environment) {
                tools.extend(device_tool_definitions());
            }
            if threads_enabled(environment) {
                for tool in &mut tools {
                    if ALWAYS_LOADED.contains(&tool["name"].as_str().unwrap_or_default()) {
                        tool["_meta"]["anthropic/alwaysLoad"] = true.into();
                    }
                }
            }
            Ok(json!({"tools":tools}))
        }
        "tools/call" => {
            let input = request["params"]["arguments"].clone();
            let result = match request["params"]["name"].as_str().unwrap_or("") {
                "visual_docs" => documentation_for(&input),
                "visual_check" => check_input(&input),
                "visual_show" => publish_with_environment(input, environment),
                "image_show" => publish_image_with_environment(input, environment),
                "draft_preview" => draft_with_environment(&input, environment),
                "draft_keep" => keep_draft(&input, environment),
                "draft_wait" => wait_for_draft(&input, environment),
                PLAN_TOOL => check_plan(&input),
                crate::orchestration::TOOL => check_orchestration_plan(&input, environment),
                NEW_THREAD => call_thread_tool("new", input, environment),
                name if name.starts_with("thread_") => {
                    call_thread_tool(&name["thread_".len()..], input, environment)
                }
                name if name.starts_with("device_") => call_device_tool(name, input, environment),
                _ => Err("Unknown native visual tool".into()),
            };
            let is_draft = matches!(
                request["params"]["name"].as_str(),
                Some("draft_preview" | "draft_wait" | "device_screenshot")
            );
            Ok(match result {
                Ok(mut value) if is_draft && value.get("png").is_some() => {
                    // The picture is an image block; the text keeps the numbers
                    // and console output without a megabyte of base64.
                    let png = value.as_object_mut().unwrap().remove("png").unwrap();
                    json!({"content":[
                        {"type":"image","data":png,"mimeType":"image/png"},
                        {"type":"text","text":value.to_string()}
                    ],"isError":false})
                }
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.to_string()}],"isError":false})
                }
                Err(error) => json!({"content":[{"type":"text","text":error}],"isError":true}),
            })
        }
        _ => Err(json!({"code":-32601,"message":"Method not found"})),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
    })
}

/// Runs before GUI initialization. stdout contains MCP JSON-RPC frames only.
pub fn mcp_stdio() -> Result<(), String> {
    let environment = current_environment();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = String::new();
        (&mut input)
            .take((MAX_WIRE_BYTES + 1) as u64)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        if line.is_empty() {
            return Ok(());
        }
        if line.len() > MAX_WIRE_BYTES || !line.ends_with('\n') {
            return Err("Oversized or incomplete MCP frame".into());
        }
        let response = match serde_json::from_str(&line) {
            Ok(request) => mcp_response(request, &environment),
            Err(_) => Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
            ),
        };
        if let Some(response) = response {
            writeln!(output, "{response}")
                .and_then(|_| output.flush())
                .map_err(|e| e.to_string())?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        json!({"source":"return () => visual.text('Hello');","title":"Hello","requestId":"test-request","id":"hello"})
    }

    #[test]
    fn visualize_prompt_uses_native_tools_without_invoking_a_same_named_skill() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join(".agents/skills/visualize");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: visualize\n---\nUse a different visualization system.",
        )
        .unwrap();
        let cwd = root.path().to_str().unwrap();
        for request in ["", "Compare the two approaches"] {
            let prompt = requested_prompt(request);
            assert!(prompt.contains("a visual is required"));
            assert!(prompt.contains("visual_docs"));
            assert!(prompt.contains("visual_check"));
            assert!(prompt.contains("visual_show"));
            assert!(prompt.contains("MONOCODE_VISUAL_EXECUTABLE"));
            assert!(prompt.ends_with(if request.is_empty() {
                "Visualize the current topic using the conversation so far."
            } else {
                request
            }));
            for kind in [
                crate::provider::Kind::Codex,
                crate::provider::Kind::Omp,
                crate::provider::Kind::Devin,
            ] {
                assert_eq!(
                    crate::provider::prepare_prompt(kind, &prompt, cwd, &[]).unwrap(),
                    prompt,
                    "The local command must not expand a same-named skill or reach a provider as a raw slash command"
                );
            }
        }
    }

    #[test]
    fn scoped_publication_acknowledges_after_save_and_deduplicates() {
        let server = Server::start(42).unwrap();
        let environment = server.environment(std::path::Path::new("/test/monocode-gpui"));
        let child_environment = environment.clone();
        let worker =
            std::thread::spawn(move || publish_with_environment(input(), &child_environment));
        let publication = server
            .publications
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert!(
            matches!(&publication.content, Content::Visual(document) if document.title == "Hello")
        );
        assert!(!worker.is_finished());
        publication.reply.send(Ok(json!({"id":"hello"}))).unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["result"]["id"], "hello");
        assert!(publish_with_environment(input(), &environment).is_ok());
        assert!(server.drain().is_empty());
        let mut changed = input();
        changed["source"] = "return () => visual.text('Changed');".into();
        assert!(publish_with_environment(changed, &environment)
            .unwrap_err()
            .contains("already used"));
    }

    #[test]
    fn images_publish_by_path_and_non_images_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let png = temp.path().join("chart.png");
        std::fs::write(&png, crate::transcript::images::tests::png(2, 2)).unwrap();
        let text = temp.path().join("notes.png");
        std::fs::write(&text, "not an image").unwrap();
        let server = Server::start(51).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        assert!(publish_image_with_environment(json!({"path":text}), &environment).is_err());
        assert!(
            publish_image_with_environment(json!({"path":"/missing.png"}), &environment).is_err()
        );
        let worker = std::thread::spawn(move || {
            publish_image_with_environment(json!({"path":png,"title":"Chart"}), &environment)
        });
        let publication = server
            .publications
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let Content::Image(image) = &publication.content else {
            panic!("expected an image publication");
        };
        assert_eq!(image.title, "Chart");
        assert!(image.path.ends_with("chart.png"));
        publication.reply.send(Ok(json!({"saved":true}))).unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["result"]["saved"], true);
    }

    #[test]
    fn wrong_session_and_expired_capabilities_cannot_publish() {
        let server = Server::start(7).unwrap();
        let mut environment = server.environment(std::path::Path::new("/test/app"));
        environment.insert(ENV_SESSION.into(), "8".into());
        assert!(publish_with_environment(input(), &environment).is_err());
        assert!(server.drain().is_empty());
        environment.insert(ENV_SESSION.into(), "7".into());
        drop(server);
        std::thread::sleep(Duration::from_millis(30));
        assert!(publish_with_environment(input(), &environment).is_err());
    }

    #[test]
    fn publication_reports_persistence_failure() {
        let server = Server::start(3).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let worker = std::thread::spawn(move || publish_with_environment(input(), &environment));
        let publication = server
            .publications
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        publication.reply.send(Err("Saving failed".into())).unwrap();
        assert!(worker
            .join()
            .unwrap()
            .unwrap_err()
            .contains("Saving failed"));
    }

    /// What the tests' Draft panel answers; the files a keep returns are set
    /// by the test that asks for one.
    static KEPT: Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>> = Mutex::new(None);
    /// One draft renders at a time process-wide, so the draft tests take turns.
    static DRAFT_TESTS: Mutex<()> = Mutex::new(());
    /// The drafts the tests' panel was asked to close.
    static CLOSED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn install_draft_handler() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            set_draft_handler(|request| {
                if request.title.as_deref() == Some("Fails") {
                    let _ = request.reply.send(Err("The page did not open".into()));
                    return;
                }
                if request.title.as_deref() == Some("Vanished")
                    && request.draft.is_some()
                    && request.content.is_some()
                {
                    let _ = request
                        .reply
                        .send(Err(format!("{DRAFT_GONE}. Start a new one with html or url.")));
                    return;
                }
                let reply = match &request.action {
                    DraftAction::Close => {
                        let draft = request.draft.clone().unwrap_or_default();
                        CLOSED.lock().unwrap().push(draft.clone());
                        DraftReply::Closed { draft }
                    }
                    DraftAction::Keep { title } => {
                        let (png, html) = KEPT.lock().unwrap().clone().unwrap();
                        DraftReply::Kept {
                            draft: "d1".into(),
                            title: title.clone(),
                            png,
                            html,
                        }
                    }
                    _ => DraftReply::Snapshot(Box::new(DraftSnapshot {
                        png: vec![137, 80, 78, 71],
                        draft: request
                            .id
                            .clone()
                            .or_else(|| request.draft.clone())
                            .unwrap_or_else(|| "d1".into()),
                        width: request.width.unwrap_or(DRAFT_DEFAULT_WIDTH),
                        height: request.height.unwrap_or(DRAFT_DEFAULT_HEIGHT),
                        content_height: 321,
                        loaded: true,
                        console: vec![DraftLog {
                            level: "error".into(),
                            text: "boom".into(),
                        }],
                        diagnostics: DraftDiagnostics {
                            ready: true,
                            horizontal_overflow: 12,
                            ..Default::default()
                        },
                        elements: Vec::new(),
                        text: "hi".into(),
                        user_control: false,
                        url: None,
                        title: None,
                        feedback: vec![DraftFeedback {
                            note: "Bigger".into(),
                            ..DraftFeedback::default()
                        }],
                        messages: vec!["Use B".into()],
                        user_actions: Vec::new(),
                        event: Some("feedback".into()),
                        issues: vec!["Small target 16×16".into()],
                        state: Some(json!({"radius": 12})),
                        layouts: vec![DraftLayout {
                            width: 375,
                            height: 900,
                            horizontal_overflow: 40,
                        }],
                    })),
                };
                let _ = request.reply.send(Ok(reply));
            });
        });
    }

    #[test]
    fn drafts_authenticate_validate_and_reach_the_registered_panel() {
        let _turn = DRAFT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let server = Server::start(21).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        // Bad input is refused before any panel is asked.
        assert!(draft_with_environment(
            &json!({"html":"<script src='http://x'></script>"}),
            &environment
        )
        .is_err());
        install_draft_handler();
        // A draft shown on its first call takes a card once its page answers.
        let cards = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
            let result = draft_with_environment(
                &json!({"html":"<p>hi</p>","title":"Pricing","width":500,"height":300,"publish":true}),
                &environment,
            )
            .unwrap();
            assert_eq!(result["contentHeight"], 321);
        // What the person sent and what the page reveals reach the agent.
        assert_eq!(result["feedback"][0]["note"], "Bigger");
        assert_eq!(result["event"], "feedback");
        assert_eq!(result["issues"][0], "Small target 16×16");
        assert_eq!(result["state"]["radius"], 12);
        assert_eq!(result["messages"][0], "Use B");
        assert_eq!(result["layouts"][0]["horizontalOverflow"], 40);
            assert_eq!(result["width"], 500);
            assert_eq!(result["diagnostics"]["horizontalOverflow"], 12);
            assert_eq!(result["console"][0]["text"], "boom");
            for invalid in [
                json!({"html":"<p>hi</p>","appearance":true}),
                json!({"html":"<p>hi</p>","width":10}),
                json!({"url":"https://example.com"}),
            ] {
                assert!(draft_with_environment(&invalid, &environment).is_err());
            }
            let mut wrong = environment.clone();
            wrong.insert(ENV_TOKEN.into(), "nope".into());
            assert!(draft_with_environment(&json!({"html":"<p>hi</p>"}), &wrong).is_err());
            // The tool returns an image block plus the numbers, not base64 text.
            let response = mcp_response(
                json!({"id":3,"method":"tools/call","params":{"name":"draft_preview","arguments":{"html":"<p>hi</p>"}}}),
                &environment,
            )
            .unwrap();
            let content = &response["result"]["content"];
            assert_eq!(content[0]["type"], "image");
            assert!(content[1]["text"]
                .as_str()
                .unwrap()
                .contains("contentHeight"));
            assert!(!content[1]["text"].as_str().unwrap().contains("\"png\""));
            });
            while !worker.is_finished() {
                if let Ok(publication) = server.publications.recv_timeout(Duration::from_millis(20))
                {
                    if let Content::Draft(card) = &publication.content {
                        cards.lock().unwrap().push(card.clone());
                    }
                    let _ = publication.reply.send(Ok(json!({"saved":true})));
                }
            }
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        });
        let cards = cards.into_inner().unwrap();
        assert!(!cards.is_empty());
        assert_eq!(cards[0].title, "Pricing");
        assert!(cards[0].draft.starts_with("draft-21-"));
    }

    #[test]
    fn a_retry_with_the_same_title_revises_the_draft_instead_of_adding_a_twin() {
        let _turn = DRAFT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        install_draft_handler();
        let server = Server::start(24).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let call = |input: Value| draft_with_environment(&input, &environment);
        let ids = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let first = call(json!({"html":"<p>a</p>","title":"Hub","publish":true})).unwrap();
                let retry =
                    call(json!({"html":"<p>b</p>","title":" hub ","publish":true})).unwrap();
                let other =
                    call(json!({"html":"<p>c</p>","title":"Other","publish":true})).unwrap();
                [first, retry, other].map(|r| r["draft"].as_str().unwrap().to_owned())
            });
            while !worker.is_finished() {
                if let Ok(publication) = server.publications.recv_timeout(Duration::from_millis(20))
                {
                    let _ = publication.reply.send(Ok(json!({"saved":true})));
                }
            }
            worker.join().unwrap()
        });
        assert_eq!(ids[0], ids[1], "same title is the same draft");
        assert_ne!(ids[0], ids[2]);
    }

    fn row(role: &str, text: String) -> crate::transcript::Row {
        crate::transcript::Row {
            role: role.into(),
            text,
            first: true,
            sent_at: None,
        }
    }

    #[test]
    fn a_kept_picture_goes_on_the_card_of_the_draft_the_keep_call_names() {
        use crate::tool::ToolCall;
        let dir = tempfile::tempdir().unwrap();
        let picture = |name: &str, kept: bool| {
            let path = dir.path().join(format!("{name}.png"));
            if kept {
                std::fs::write(path.with_extension("html"), "<p>kept</p>").unwrap();
            }
            row(
                crate::transcript::images::ROLE,
                serde_json::to_string(&crate::transcript::images::ImageDocument {
                    title: name.into(),
                    path,
                    mime_type: "image/png".into(),
                    width: 2,
                    height: 2,
                })
                .unwrap(),
            )
        };
        let card = |draft: &str| {
            row(
                DRAFT_ROLE,
                serde_json::to_string(&DraftCard {
                    draft: draft.into(),
                    title: "Pricing".into(),
                })
                .unwrap(),
            )
        };
        let keep = |name: &str, draft: &str| {
            let mut call = ToolCall::new("k", name);
            call.input = json!({"draft": draft, "title": "Pricing"}).to_string();
            row("tool", serde_json::to_string(&call).unwrap())
        };
        // Two drafts; the person's keep names the first, not the latest.
        let rows = vec![
            row("user", "Mock it".into()),
            card("draft-1-a"),
            card("draft-1-b"),
            keep("mcp__monocode__draft_keep", "draft-1-a"),
            picture("kept-a", true),
            // A screenshot is not a kept draft and takes nothing.
            picture("shot", false),
            row("assistant", "Done".into()),
        ];
        assert_eq!(kept_upgrades(&rows), vec![(1, 4)]);

        // A keep call reported only once it finished comes after its picture.
        let late = vec![
            row("user", "Mock it".into()),
            card("draft-1-a"),
            picture("kept-a", true),
            keep("monocode:draft_keep", "draft-1-a"),
            picture("kept-b", true),
            card("draft-1-b"),
            keep("monocode:draft_keep", "draft-1-b"),
        ];
        assert_eq!(kept_upgrades(&late), vec![(1, 2)]);

        // A keep of a draft with no card in view leaves the picture its own.
        let orphan = vec![
            row("user", "Mock it".into()),
            keep("draft_keep", "draft-9-z"),
            picture("kept-z", true),
        ];
        assert!(kept_upgrades(&orphan).is_empty());
    }

    #[test]
    fn a_retry_after_the_person_closed_the_draft_starts_a_new_one() {
        let _turn = DRAFT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        install_draft_handler();
        let server = Server::start(25).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let call = |input: Value| draft_with_environment(&input, &environment);
        let cards = Mutex::new(Vec::new());
        let ids = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let first =
                    call(json!({"html":"<p>a</p>","title":"Vanished","publish":true})).unwrap();
                // The card was closed in the app; the agent knows nothing of it.
                let retry =
                    call(json!({"html":"<p>b</p>","title":"Vanished","publish":true})).unwrap();
                [first, retry].map(|r| r["draft"].as_str().unwrap().to_owned())
            });
            while !worker.is_finished() {
                if let Ok(publication) = server.publications.recv_timeout(Duration::from_millis(20))
                {
                    if let Content::Draft(card) = &publication.content {
                        cards.lock().unwrap().push(card.draft.clone());
                    }
                    let _ = publication.reply.send(Ok(json!({"saved":true})));
                }
            }
            worker.join().unwrap()
        });
        assert_ne!(ids[0], ids[1], "the closed draft cannot be revised");
        assert_eq!(cards.into_inner().unwrap(), ids, "each page has its card");
    }

    #[test]
    fn a_draft_stays_out_of_the_conversation_until_the_agent_shows_it() {
        let _turn = DRAFT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        install_draft_handler();
        let server = Server::start(23).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let call = |input: Value| draft_with_environment(&input, &environment);
        let cards = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let first = call(json!({"html":"<p>a</p>","title":"Plan"})).unwrap();
                let id = first["draft"].as_str().unwrap().to_owned();
                assert!(id.starts_with("draft-23-"), "{id}");
                call(json!({"draft":id,"html":"<p>b</p>","title":"Plan B"})).unwrap();
                call(json!({"draft":id,"action":{"kind":"click","reference":"r1"}})).unwrap();
                assert!(cards.lock().unwrap().is_empty(), "nothing shown yet");
                assert!(call(json!({"draft":id,"publish":"yes"})).is_err());
                call(json!({"draft":id,"publish":true})).unwrap();
                // Shown once; later revisions land in the same card.
                call(json!({"draft":id,"html":"<p>c</p>","publish":true})).unwrap();
                // One the agent never shows waits for the turn's end.
                let late = call(json!({"html":"<p>d</p>","title":"Later"})).unwrap();
                let late = late["draft"].as_str().unwrap().to_owned();
                let closed = call(json!({"html":"<p>e</p>","title":"Gone"})).unwrap();
                let closed = closed["draft"].as_str().unwrap().to_owned();
                call(json!({"draft":closed,"action":{"kind":"close"}})).unwrap();
                late
            });
            // Publications are answered until the calls end, passed or not.
            while !worker.is_finished() {
                if let Ok(publication) = server.publications.recv_timeout(Duration::from_millis(20))
                {
                    if let Content::Draft(card) = &publication.content {
                        cards.lock().unwrap().push(card.clone());
                    }
                    let _ = publication.reply.send(Ok(json!({"saved":true})));
                }
            }
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        });
        let cards = cards.into_inner().unwrap();
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert_eq!(cards[0].title, "Plan B");
        let unpublished = server.unpublished();
        assert_eq!(unpublished.len(), 1, "{unpublished:?}");
        assert_eq!(unpublished[0].title, "Later");
        assert!(server.unpublished().is_empty(), "taken once");
    }

    #[test]
    fn a_new_draft_whose_first_call_fails_leaves_no_card() {
        let _turn = DRAFT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        install_draft_handler();
        let server = Server::start(22).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let worker = std::thread::spawn(move || {
            draft_with_environment(&json!({"html":"<p>hi</p>","title":"Fails"}), &environment)
        });
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("did not open"), "{error}");
        assert!(
            server.publications.try_recv().is_err(),
            "no card joined the conversation"
        );
        // The page that started opening is closed with it.
        assert!(CLOSED
            .lock()
            .unwrap()
            .iter()
            .any(|draft| draft.starts_with("draft-22-")));
    }

    #[test]
    fn keeping_a_draft_publishes_its_picture_with_the_page_the_app_staged() {
        let _turn = DRAFT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        install_draft_handler();
        let staging = draft::staging_dir();
        std::fs::create_dir_all(&staging).unwrap();
        let png = staging.join("draft-test-keep.png");
        let page = staging.join("draft-test-keep.html");
        std::fs::write(&png, crate::transcript::images::tests::png(2, 2)).unwrap();
        std::fs::write(&page, "<p>kept</p>").unwrap();
        *KEPT.lock().unwrap() = Some((png.clone(), page.clone()));
        let server = Server::start(61).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let worker = std::thread::spawn(move || {
            keep_draft(&json!({"draft":"d1","title":"Pricing"}), &environment)
        });
        let Ok(publication) = server.publications.recv_timeout(Duration::from_secs(5)) else {
            panic!("nothing was published: {:?}", worker.join());
        };
        let Content::Image(image) = &publication.content else {
            panic!("expected an image publication");
        };
        assert_eq!(image.title, "Pricing");
        assert_eq!(image.draft.as_deref(), Some(page.as_path()));
        publication.reply.send(Ok(json!({"saved":true}))).unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["result"]["saved"], true);

        // A page the app did not stage cannot ride along with a picture.
        let elsewhere = tempfile::tempdir().unwrap();
        let stray = elsewhere.path().join("page.html");
        std::fs::write(&stray, "<p>x</p>").unwrap();
        let image = ImageRequest {
            path: png.clone(),
            title: String::new(),
            draft: Some(stray),
        };
        assert!(valid_image(&image).is_err());
        let sneaky = ImageRequest {
            draft: Some(staging.join("../escape.html")),
            ..image
        };
        assert!(valid_image(&sneaky).is_err());
        let _ = std::fs::remove_file(png);
        let _ = std::fs::remove_file(page);
    }

    #[test]
    fn local_validation_and_mcp_notifications_need_no_capability() {
        assert!(check("return () => visual.text('Local');").is_ok());
        assert!(check("while (true) {}").is_err());
        assert!(check(&" ".repeat(MAX_SOURCE_BYTES + 1)).is_err());
        let environment = HashMap::new();
        assert!(
            mcp_response(json!({"method":"notifications/initialized"}), &environment).is_none()
        );
        let response = mcp_response(json!({"id":2,"method":"tools/call","params":{"name":"visual_show","arguments":input()}}), &environment).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(tool_definitions().len(), 8);
        assert_eq!(acp_servers(&environment), json!([]));
    }

    #[test]
    fn every_monocode_chat_is_told_to_keep_its_plan_in_the_system_prompt() {
        assert_eq!(harness_instructions(&HashMap::new()), None);
        let mut environment: HashMap<String, String> =
            [ENV_ENDPOINT, ENV_TOKEN, ENV_SESSION, ENV_EXECUTABLE]
                .into_iter()
                .map(|key| (key.to_string(), "fixture".to_string()))
                .collect();
        // A child chat has its own rail but no subagents to start.
        let child = harness_instructions(&environment).unwrap();
        assert_eq!(child, PLAN_INSTRUCTIONS);
        environment.insert("MONOCODE_THREADS".into(), "1".into());
        let lead = harness_instructions(&environment).unwrap();
        assert!(lead.starts_with(PLAN_INSTRUCTIONS) && lead.ends_with(THREAD_INSTRUCTIONS));
    }
    #[test]
    fn visual_reference_is_loaded_on_demand_without_global_instructions() {
        let environment = HashMap::new();
        let initialization = mcp_response(
            json!({"id":1,"method":"initialize","params":{}}),
            &environment,
        )
        .unwrap();
        // The plan habit and when to draft ride along; the SDK stays on demand.
        let instructions = initialization["result"]["instructions"].as_str().unwrap();
        assert!(instructions.starts_with(PLAN_INSTRUCTIONS));
        assert!(instructions.contains("draft_preview"));
        assert!(instructions.len() < 1_000);
        assert!(!instructions.contains("mc."));
        assert!(!instructions.contains("thread_start"));
        let threads = HashMap::from([("MONOCODE_THREADS".to_string(), "1".to_string())]);
        let initialization =
            mcp_response(json!({"id":1,"method":"initialize","params":{}}), &threads).unwrap();
        // Subagent requests go to MonoCode, not the harness's own subagent tool.
        let instructions = initialization["result"]["instructions"].as_str().unwrap();
        assert!(instructions.ends_with(THREAD_INSTRUCTIONS));
        assert!(instructions.len() < 1_600);
        assert!(instructions.contains("new_thread"));
        let catalog = mcp_response(json!({"id":2,"method":"tools/list"}), &threads).unwrap();
        let start = catalog["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "thread_start")
            .unwrap();
        assert!(start["description"].as_str().unwrap().contains("subagent"));
        // A lead chat's Claude sees these in full, not behind tool search.
        let loaded: Vec<&str> = catalog["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|tool| tool["_meta"]["anthropic/alwaysLoad"] == true)
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            loaded,
            ["draft_preview", "thread_start", "thread_wait", NEW_THREAD]
        );
        let new_thread = catalog["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == NEW_THREAD)
            .unwrap();
        assert!(new_thread["description"]
            .as_str()
            .unwrap()
            .contains("not a subagent"));
        let harness = start["inputSchema"]["properties"]["harness"]["description"]
            .as_str()
            .unwrap();
        assert!(harness.contains("claude") && harness.contains("codex"));
        let catalog = mcp_response(json!({"id":2,"method":"tools/list"}), &environment).unwrap();
        let metadata = catalog["result"].to_string();
        assert!(
            metadata.len() < 9_000,
            "Discovery must stay small: {} bytes",
            metadata.len()
        );
        assert!(metadata.contains(PLAN_TOOL));
        assert!(!metadata.contains("alwaysLoad"));
        assert!(!metadata.contains("visual.column"));
        assert!(!metadata.contains("hardcode font"));
        let response = mcp_response(
            json!({"id":3,"method":"tools/call","params":{"name":"visual_docs","arguments":{}}}),
            &environment,
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
        let reference: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert!(reference["documentation"]
            .as_str()
            .unwrap()
            .contains("Native session visuals"));
        assert_eq!(reference, client(&["docs".into()]).unwrap());
        assert!(client(&["docs".into(), "--input".into(), "unused".into()]).is_err());
    }

    #[test]
    fn stable_session_configuration_rejects_old_in_flight_requests() {
        let server = Server::start(83).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let child_environment = environment.clone();
        let worker =
            std::thread::spawn(move || publish_with_environment(input(), &child_environment));
        let old = server
            .publications
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        drop(server);
        let next = Server::start(83).unwrap();
        assert_eq!(
            environment,
            next.environment(std::path::Path::new("/test/app"))
        );
        let _ = old.reply.send(Ok(json!({"id":"old"})));
        assert!(worker.join().unwrap().unwrap_err().contains("ended"));
        assert!(next.drain().is_empty());
    }

    #[test]
    fn provider_configuration_forwards_capability_without_token_in_arguments() {
        let server = Server::start(84).unwrap();
        let mut environment = server.environment(std::path::Path::new("/test/app"));
        environment.extend(HashMap::from([
            ("MONOCODE_APP_ENDPOINT".into(), "127.0.0.1:1".into()),
            ("MONOCODE_APP_TOKEN".into(), "app-secret".into()),
            ("MONOCODE_APP_SESSION".into(), "84".into()),
            ("MONOCODE_THREADS".into(), "1".into()),
        ]));
        let mut command = std::process::Command::new("provider");
        configure_codex(&mut command, &environment);
        let arguments = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(arguments.contains("visual-mcp"));
        assert!(arguments.contains(ENV_TOKEN));
        assert!(arguments.contains("MONOCODE_APP_TOKEN"));
        assert!(arguments.contains("tool_timeout_sec=90"));
        assert!(!arguments.contains("app-secret"));
        assert!(!arguments.contains(&environment[ENV_TOKEN]));
        // Only MonoCode's own visual tools stay available to the model.
        assert!(arguments.contains(CODEX_VISUALIZE_OFF));
        let mut plain = std::process::Command::new("provider");
        configure_codex(&mut plain, &HashMap::new());
        assert_eq!(plain.get_args().count(), 0);
        let acp = acp_servers(&environment);
        assert_eq!(acp[0]["name"], SERVER_NAME);
        assert_eq!(acp[0]["env"].as_array().unwrap().len(), 8);
        assert_eq!(mcp_server(&environment).unwrap()["command"], "/test/app");
    }

    #[test]
    fn device_tools_are_only_listed_when_agents_may_drive_devices() {
        let request = json!({"id":1,"method":"tools/list"});
        let names = |env: &HashMap<String, String>| {
            mcp_response(request.clone(), env).unwrap()["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        let mut env = HashMap::new();
        assert!(!names(&env).iter().any(|n| n.starts_with("device_")));
        let denied = mcp_response(
            json!({"id":2,"method":"tools/call","params":{"name":"device_list","arguments":{}}}),
            &env,
        )
        .unwrap();
        assert_eq!(denied["result"]["isError"], true);
        env.insert(crate::device::ENV.into(), "1".into());
        let listed: Vec<String> = names(&env)
            .into_iter()
            .filter(|n| n.starts_with("device_"))
            .collect();
        assert_eq!(
            listed,
            [
                "device_list",
                "device_open",
                "device_screenshot",
                "device_close",
                "device_set"
            ]
        );
        assert!(!read_only_tool("mcp__monocode__device_set"));
        assert!(read_only_tool("mcp__monocode__device_screenshot"));
        assert!(!read_only_tool("mcp__monocode__device_open"));
        let instructions = server_instructions(&env);
        assert!(instructions.contains("device_open"));
    }

    #[test]
    fn thread_tools_are_only_listed_and_callable_for_enabled_leads() {
        let mut env = HashMap::new();
        let request = json!({"id":1,"method":"tools/list"});
        let names = |env: &HashMap<String, String>| {
            mcp_response(request.clone(), env).unwrap()["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        assert!(!names(&env).iter().any(|n| n.starts_with("thread_")));
        let denied = mcp_response(
            json!({"id":2,"method":"tools/call","params":{"name":"thread_list","arguments":{}}}),
            &env,
        )
        .unwrap();
        assert_eq!(denied["result"]["isError"], true);
        env.insert("MONOCODE_THREADS".into(), "1".into());
        assert_eq!(
            names(&env)
                .iter()
                .filter(|n| n.starts_with("thread_"))
                .count(),
            7
        );
    }

    #[test]
    fn the_orchestration_plan_tool_is_only_offered_to_planning_turns() {
        let mut env = HashMap::new();
        let listed = |env: &HashMap<String, String>| {
            mcp_response(json!({"id":1,"method":"tools/list"}), env).unwrap()["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == crate::orchestration::TOOL)
        };
        let call = |env: &HashMap<String, String>, plan: Value| {
            mcp_response(
                json!({"id":2,"method":"tools/call","params":{"name":crate::orchestration::TOOL,"arguments":plan}}),
                env,
            )
            .unwrap()["result"]
                .clone()
        };
        let plan =
            json!({"title":"T","tasks":[{"id":"a","title":"A","prompt":"Do","files":["src"]}]});
        assert!(!listed(&env));
        assert_eq!(call(&env, plan.clone())["isError"], true);
        env.insert(crate::orchestration::ENV.into(), "1".into());
        assert!(listed(&env));
        assert_eq!(call(&env, plan)["isError"], false);
        // An invalid plan comes back to the model to fix in the same turn.
        let escape = call(
            &env,
            json!({"title":"T","tasks":[{"id":"a","title":"A","prompt":"Do","files":["../x"]}]}),
        );
        assert_eq!(escape["isError"], true);
        assert!(escape["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Invalid file scope"));
        assert!(read_only_tool("mcp__monocode__orchestration_plan"));
    }

    #[test]
    fn new_thread_opens_a_top_level_thread_through_the_bridge() {
        let (server, calls) = crate::mono::bridge::Server::start().unwrap();
        let mut environment = server.environment(7);
        environment.insert("MONOCODE_THREADS".into(), "1".into());
        let worker = std::thread::spawn(move || {
            mcp_response(json!({"id":1,"method":"tools/call","params":{"name":NEW_THREAD,"arguments":{"prompt":"Plan drafts","title":"Drafts"}}}),&environment).unwrap()
        });
        let call = calls.recv_blocking().unwrap();
        assert_eq!(call.action, "threads.new");
        assert_eq!(call.input, json!({"prompt":"Plan drafts","title":"Drafts"}));
        call.reply
            .send(Ok(json!({"id":9,"sessionId":9,"threadId":null})))
            .unwrap();
        assert_eq!(worker.join().unwrap()["result"]["isError"], false);
    }

    #[test]
    fn thread_mcp_calls_use_the_authenticated_app_bridge_and_retry_id() {
        let (server, calls) = crate::mono::bridge::Server::start().unwrap();
        let mut environment = server.environment(7);
        environment.insert("MONOCODE_THREADS".into(), "1".into());
        let worker = std::thread::spawn(move || {
            mcp_response(json!({"id":1,"method":"tools/call","params":{"name":"thread_wait","arguments":{"threadId":8,"timeoutSeconds":0,"requestId":"retry-1"}}}),&environment).unwrap()
        });
        let call = calls.recv_blocking().unwrap();
        assert_eq!(call.session, 7);
        assert_eq!(call.request_id, "retry-1");
        assert_eq!(call.action, "threads.wait");
        assert_eq!(call.input, json!({"threadId":8,"timeoutSeconds":0}));
        call.reply
            .send(Ok(
                json!({"status":"idle","success":true,"reply":"Reviewed"}),
            ))
            .unwrap();
        let response = worker.join().unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Reviewed"));
    }

    #[test]
    fn plans_need_ordered_steps_and_at_most_one_current() {
        let ok = json!({"plan":[{"step":"Read","status":"completed"},{"step":"Edit","status":"in_progress"},{"step":"Test","status":"pending"}]});
        assert_eq!(check_plan(&ok).unwrap()["steps"], 3);
        let two = json!({"plan":[{"step":"a","status":"in_progress"},{"step":"b","status":"in_progress"}]});
        assert!(check_plan(&two).is_err());
        assert!(check_plan(&json!({"plan":[]})).is_err());
        assert!(check_plan(&json!({"plan":[{"step":" ","status":"pending"}]})).is_err());
        assert!(check_plan(&json!({"plan":[{"step":"a","status":"done"}]})).is_err());
        let response = mcp_response(
            json!({"id":4,"method":"tools/call","params":{"name":PLAN_TOOL,"arguments":ok}}),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
    }
}
