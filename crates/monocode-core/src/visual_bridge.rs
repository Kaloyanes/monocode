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
pub use draft::{
    csp_sources, local_url, staging_dir as draft_staging_dir, Diagnostics as DraftDiagnostics,
    DraftAction, DraftContent, DraftReply, DraftRequest, Element as DraftElement,
    Feedback as DraftFeedback, LogEntry as DraftLog, Snapshot as DraftSnapshot,
    DEFAULT_HEIGHT as DRAFT_DEFAULT_HEIGHT, DEFAULT_WAIT_SECONDS as DRAFT_DEFAULT_WAIT_SECONDS,
    DEFAULT_WIDTH as DRAFT_DEFAULT_WIDTH, FONT_HOSTS, SCRIPT_HOSTS, STYLE_HOSTS,
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
const SERVER_ENV: [&str; 8] = [
    ENV_ENDPOINT,
    ENV_TOKEN,
    ENV_SESSION,
    ENV_EXECUTABLE,
    "MONOCODE_APP_ENDPOINT",
    "MONOCODE_APP_TOKEN",
    "MONOCODE_APP_SESSION",
    "MONOCODE_THREADS",
];

const PLAN_INSTRUCTIONS: &str = "Keep the user's plan rail current with update_plan: call it when you start work that takes three or more steps, and again each time a step starts or finishes.";
const MAX_PLAN_STEPS: usize = 24;
const MAX_STEP_CHARS: usize = 160;

/// Loaded through visual_docs or the CLI only when authoring a visual. Keep the
/// SDK out of ordinary prompts, server instructions and tool descriptions.
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
}

struct SessionCapability {
    token: String,
    generation: u64,
    active: Option<ActiveTurn>,
    receipts: HashMap<String, (String, Option<Value>)>,
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
                });
            if capability.active.is_some() {
                return Err("A visual session turn is already active".into());
            }
            capability.generation = capability.generation.wrapping_add(1);
            capability.active = Some(ActiveTurn {
                generation: capability.generation,
                send,
                publications: 0,
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
        // A new draft takes its place in the conversation first: that card is
        // where the page is drawn, so the page cannot load before it.
        if request.draft.is_none() && request.content.is_some() {
            let id = format!("draft-{session}-{:08x}", rand::random::<u32>());
            let card = DraftCard {
                draft: id.clone(),
                title: request.title.clone().unwrap_or_default(),
            };
            if let Err(error) = publish_card(hub, session, card) {
                release(session);
                return Err(error);
            }
            request.id = Some(id);
        }
        handler(request);
        let reply = result.recv_timeout(Duration::from_secs(45));
        release(session);
        let reply = reply.map_err(|_| "The draft timed out".to_string())??;
        Ok(json!({"ok":true,"result":reply.value()}))
    })()
    .unwrap_or_else(|error| json!({"ok": false, "error": error}));
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{response}").map_err(|e| e.to_string())
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
                return Err("Usage: monocode-gpui visual draft --json '{\"html\":\"...\"}'".into())
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
        json!({"name":"draft_preview","description":"Build and inspect a live page inside the conversation, as a card at this point in the thread. Pass {html} (inline CSS/JS) or a localhost {url} to open one, with a short {title}; pass {draft} to look again. Returns a screenshot, text, controls with refs, console output and diagnostics. Revise with {draft,html}; resize or retheme with width/height/appearance. Actions: click {reference}, type {text,replace?}, resume, close. If userControl is true the user took over: inspect, but resume only after they hand it back. Results carry feedback the person sent from the page and issues the page reveals; to wait for feedback use draft_wait. Read visual_docs {topic:draft} first. Keep a good result with draft_keep.","inputSchema":{"type":"object","properties":{"html":{"type":"string"},"title":{"type":"string","maxLength":160},"url":{"type":"string","description":"A dev server on localhost"},"draft":{"type":"string","maxLength":96},"action":{"type":"object","properties":{"kind":{"enum":["snapshot","click","type","resume","close"]},"reference":{"type":"string","maxLength":96},"text":{"type":"string","maxLength":4096},"replace":{"type":"boolean"}},"required":["kind"],"additionalProperties":false},"width":{"type":"integer","minimum":240,"maximum":1600},"height":{"type":"integer","minimum":120,"maximum":1600},"appearance":{"enum":["light","dark"]}},"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":true}}),
        json!({"name":"draft_wait","description":"Pause until the person has tried the draft and sends their feedback (pinned notes on the page) or hands it back, then look at the page again. Returns the usual screenshot and numbers plus feedback, userActions and event (feedback, handed back or timeout). Waits up to 30 seconds; call again to keep waiting. Use after asking the person to try it, not in place of draft_preview.","inputSchema":{"type":"object","properties":{"draft":{"type":"string","maxLength":96},"seconds":{"type":"integer","minimum":1,"maximum":30}},"required":["draft"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"draft_keep","description":"Save the current HTML draft as a picture in the conversation that reopens the live page after the draft is gone. Use for a result worth keeping, not every iteration.","inputSchema":{"type":"object","properties":{"draft":{"type":"string","maxLength":96},"title":{"type":"string","maxLength":160},"requestId":{"type":"string","maxLength":128}},"required":["draft","title"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
        json!({"name":"image_show","description":"Show a PNG, JPEG, GIF or WebP file you made, such as a chart or screenshot, in the conversation. The app keeps its own copy.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"title":{"type":"string","maxLength":160},"requestId":{"type":"string","maxLength":128}},"required":["path"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
    ]
}

pub(crate) fn thread_tool_definitions() -> Vec<Value> {
    ["start", "send", "wait", "read", "stop", "list"].into_iter().map(|action| {
        let mut properties = json!({"requestId":{"type":"string","maxLength":128,"description":"Reuse for an uncertain retry with identical input"}});
        let required = match action {
            "start" => {
                properties["prompt"] = json!({"type":"string","minLength":1,"maxLength":240000});
                for key in ["harness", "model", "effort", "title"] { properties[key] = json!({"type":"string"}); }
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
            "start" => "Start a child on the requested harness and model.",
            "send" => "Send a follow-up to an idle child; fails while it is busy.",
            "wait" => "Wait up to 50 seconds. Returns idle with success and the final reply, working, or needsInput for a user approval or question. Tell the user when input is needed.",
            "read" => "Read recent prose exchanges with an owned child, using nextBefore for older turns.",
            "stop" => "Stop an owned child.",
            _ => "List your own children and their current status.",
        };
        json!({"name":format!("thread_{action}"),"description":format!("{detail} Start/send return at once; call thread_wait until status is idle. Children share your checkout unless workspaceMode is worktree. Children cannot spawn or message other threads."),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":matches!(action,"wait"|"read"|"list"),"destructiveHint":false,"openWorldHint":false}})
    }).collect()
}

fn call_thread_tool(
    name: &str,
    mut input: Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    if environment.get("MONOCODE_THREADS").is_none_or(|v| v != "1") {
        return Err("Thread tools are only available to top-level threads".into());
    }
    let request = crate::app_access::optional_text(&input, "requestId", 128)?
        .unwrap_or_else(crate::mono::new_id);
    input
        .as_object_mut()
        .ok_or("Input must be an object")?
        .remove("requestId");
    let action = format!(
        "threads.{}",
        name.strip_prefix("thread_").ok_or("Unknown thread tool")?
    );
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
        Some("update_plan" | "thread_wait" | "thread_read" | "thread_list" | "draft_wait")
    )
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
            json!({"protocolVersion":request["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":SERVER_NAME,"version":env!("CARGO_PKG_VERSION")},"instructions":PLAN_INSTRUCTIONS}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut tools = tool_definitions();
            if environment
                .get("MONOCODE_THREADS")
                .is_some_and(|v| v == "1")
            {
                tools.extend(thread_tool_definitions());
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
                name if name.starts_with("thread_") => call_thread_tool(name, input, environment),
                _ => Err("Unknown native visual tool".into()),
            };
            let is_draft = matches!(
                request["params"]["name"].as_str(),
                Some("draft_preview" | "draft_wait")
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

    fn install_draft_handler() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            set_draft_handler(|request| {
                let reply = match &request.action {
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
                        draft: "d1".into(),
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
                        user_actions: Vec::new(),
                        event: Some("feedback".into()),
                        issues: vec!["Small target 16×16".into()],
                        state: Some(json!({"radius": 12})),
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
        // Each new draft takes a card in the conversation before its page loads.
        let stop = std::sync::atomic::AtomicBool::new(false);
        let cards = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            scope.spawn(|| {
            let result = draft_with_environment(
                &json!({"html":"<p>hi</p>","title":"Pricing","width":500,"height":300}),
                &environment,
            )
            .unwrap();
            assert_eq!(result["contentHeight"], 321);
        // What the person sent and what the page reveals reach the agent.
        assert_eq!(result["feedback"][0]["note"], "Bigger");
        assert_eq!(result["event"], "feedback");
        assert_eq!(result["issues"][0], "Small target 16×16");
        assert_eq!(result["state"]["radius"], 12);
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
                stop.store(true, Ordering::Relaxed);
            });
            while !stop.load(Ordering::Relaxed) {
                if let Ok(publication) = server.publications.recv_timeout(Duration::from_millis(20))
                {
                    if let Content::Draft(card) = &publication.content {
                        cards.lock().unwrap().push(card.clone());
                    }
                    let _ = publication.reply.send(Ok(json!({"saved":true})));
                }
            }
        });
        let cards = cards.into_inner().unwrap();
        assert!(!cards.is_empty());
        assert_eq!(cards[0].title, "Pricing");
        assert!(cards[0].draft.starts_with("draft-21-"));
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
    fn visual_reference_is_loaded_on_demand_without_global_instructions() {
        let environment = HashMap::new();
        let initialization = mcp_response(
            json!({"id":1,"method":"initialize","params":{}}),
            &environment,
        )
        .unwrap();
        // Only the plan habit rides along; the visual SDK stays on demand.
        assert_eq!(initialization["result"]["instructions"], PLAN_INSTRUCTIONS);
        let catalog = mcp_response(json!({"id":2,"method":"tools/list"}), &environment).unwrap();
        let metadata = catalog["result"].to_string();
        assert!(
            metadata.len() < 9_000,
            "Discovery must stay small: {} bytes",
            metadata.len()
        );
        assert!(metadata.contains(PLAN_TOOL));
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
            6
        );
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
