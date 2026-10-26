//! Ephemeral, session-scoped publication of native visuals. No user MCP settings
//! are changed. Stable provider capabilities can publish only during a live turn.
use crate::visual::{self, VisualDocument, VisualFormat, DEFAULT_HTML_HEIGHT};
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
/// A publish or preview reaching the app carries its embedded images.
const MAX_HUB_BYTES: usize = visual::images::MAX_HTML_BYTES + MAX_WIRE_BYTES;
const MAX_SOURCE_BYTES: usize = visual::MAX_SOURCE_BYTES;
const ENV_ENDPOINT: &str = "MONOCODE_VISUAL_ENDPOINT";
const ENV_TOKEN: &str = "MONOCODE_VISUAL_TOKEN";
const ENV_SESSION: &str = "MONOCODE_VISUAL_SESSION";
const ENV_EXECUTABLE: &str = "MONOCODE_VISUAL_EXECUTABLE";

/// Loaded through visual_docs or the CLI only when authoring a visual. Keep the
/// SDK out of ordinary prompts, server instructions and tool descriptions.
const DOCUMENTATION: &str = r#"Session visuals come in two kinds. Pick by what you need to show:
- html: any rich or custom picture (UI mockups, diagrams, flows, animated or
  styled charts, SVG, layouts). Pass {html, height?}. See "HTML visuals" at the end,
  and call visual_preview to see the result before visual_show.
- native: small interactive widgets built from the component SDK below. Pass {source}.
Use visual_check to validate a visual, then visual_show to publish it into this
conversation. They accept {source} or {html,height?}, never both; visual_show also
accepts {title?,id?}.

Native session visuals
The source is a JavaScript function body with a `visual` SDK. Always return a render function. Use closure state for interactions. Example:
const state = visual.state({count: 0});
return () => visual.column([
  visual.text('Counter', {size: 'title'}),
  visual.stat('Count', String(state.count)),
  visual.button('Add', () => { state.count += 1; })
], {gap: 12});

Native components:
visual.column(children,{gap?}), row(children,{gap?}), grid(children,{columns?,gap?})
visual.text(text,{size?,color?}), stat(label,value,{detail?,color?})
visual.table(columns,rows)
visual.code(source,{language?,title?}) highlights source; visual.diff(before,after,{path?,title?,context?})
shows a folded line diff; visual.callout(text,{title?,color?}) marks a note, warning or result.
visual.chart('bar'|'line'|'area',[{label,values,color?}],{labels?,height?})
visual.button(label,onClick,{disabled?}), toggle(label,value,onChange)
visual.slider(label,value,min,max,onChange,{step?}), tabs(labels,selected,onChange)
visual.input(label,value,onChange,{placeholder?}), select(label,options,selectedIndex,onChange)
Input handlers receive a string (up to 4096 bytes); select handlers receive an index.
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
The native sandbox has no DOM, network, files, timers, imports, or external libraries.
No HTML, SVG markup, or shader code in native source. Source is limited to 64 KiB; keep
scripts compact. Check validates execution, the initial component tree, and representative controls;
it does not capture a screenshot. Fix errors before publishing. Set a concise
title for the published visual. Never expose capability environment variables.

HTML visuals
{html} is one complete document body: HTML with inline <style> and <script>. It
runs in an isolated web view: no network requests from your code (fetch, XHR,
WebSocket), no app access and no storage. It may load scripts, stylesheets and
fonts from these CDNs only, over https: cdn.jsdelivr.net, cdnjs.cloudflare.com,
unpkg.com and esm.sh for code and styles, plus fonts.googleapis.com and
fonts.gstatic.com for fonts. For example
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.4"></script>; pin a
version and prefer small, well known libraries. Anything else is blocked. A page that needs a library fails to render if
the reader is offline, so use plain HTML, CSS, inline SVG and vanilla JavaScript
when they are enough. Links open in the system browser.
Images: reference local files by absolute path (or ~/...) and https images
directly, in <img src>, CSS url() or a JavaScript string, e.g.
<img src="/Users/me/Desktop/Screenshot 1.png"> or
url(https://images.unsplash.com/photo-123?w=800). visual_check, visual_preview
and visual_show embed them in the page, so it never fetches anything and keeps
working offline. PNG, JPEG, GIF, WebP, AVIF, BMP, ICO and SVG work; large
rasters are scaled to 1600 pixels on their long side; at most 24 images and
6 MiB per page. Use screenshots and real photos when they show the point better
than a drawing, give every image alt text, and size them with CSS (width:100%,
aspect-ratio, object-fit). data: URLs and inline SVG work too.
{height} is the frame height in points while the page loads, 120–900 (default
480); pass the preview's contentHeight. Once loaded, the frame fits the page's
content and follows it as it grows or shrinks (up to 900), so let content set the
page height: never use 100vh, or height:100% on html or body. The page never
scrolls inside its frame. Let wide layouts shrink with the thread width (use %,
flex, grid and an SVG viewBox, not fixed pixel widths).
The app theme is injected as CSS variables on :root and follows light and dark:
--background, --foreground, --muted, --muted-foreground, --card, --card-foreground,
--popover, --popover-foreground, --secondary, --secondary-foreground, --border,
--input, --ring, --primary, --primary-foreground (solid buttons), --accent,
--accent-foreground, --accent-surface, --accent-surface-foreground, --destructive,
--destructive-foreground, --destructive-surface, --warning, --warning-foreground,
--warning-surface, --success, --success-foreground, --info, --info-foreground,
--code-background, --code-foreground, --chart-1 … --chart-6, --radius, --font-sans,
--font-mono. Use them instead of hardcoded colors so both themes work.
Layout: the page sits directly on the conversation's own surface and is part of
your reply. The base stylesheet sets box-sizing:border-box, body margin 0, the
theme font and color, and transparent html and body backgrounds; leave them
transparent. Do not wrap everything in an outer card, border or banner title. If
one element needs its own surface (a mock of a screen, a panel), give it
var(--card) or var(--popover) with var(--border), var(--radius) corners and at
least 16px padding. Make the layout fluid: % widths, flex, grid and an SVG
viewBox, never fixed pixel widths. Give charts fixed pixel heights. Keep it under
64 KiB, make text at least 12px, label every chart axis and unit, and keep
supporting prose in the conversation. Check validates size, embeds images and rejects
network use; it does not render the page.

Always preview before you publish. visual_preview (CLI: visual preview) renders
the page in the app and returns a screenshot, the page's console output and
errors, and contentHeight. Pass it the same html, plus the height you intend to
publish; pass appearance light or dark to check the other theme, which every
reader may be using. Look at the screenshot critically: alignment, clipping at the frame edge,
empty areas, contrast in the current theme, text that is too small. Fix every
console error, then preview again; publish only when the picture is good. A page
with a script error often renders blank, and only the preview shows that.
contentHeight is the height the page wants; publish with that height.

Built-in kit. Every HTML page already has MonoCode's component kit; use it
instead of styling controls from scratch, and never load another UI framework.
Classes: mc-row, mc-col, mc-grid, mc-spacer, mc-muted, mc-label, mc-mono,
mc-title, mc-divider; mc-card (tinted panel), mc-surface (floating popover with
shadow); mc-button (+ primary, accent, ghost, danger, icon, small), mc-input,
mc-badge (+ accent, success, warning, danger), mc-kbd, mc-avatar; mc-menu with
mc-menu-item (+ active, danger; an mc-kbd inside is right-aligned) and
mc-menu-sep; mc-stat (label text then <b>value</b>); mc-window with mc-window-bar
(three <i></i> for traffic lights) for app mockups; mc-stage (+ dots or
grid-lines) as the backdrop a subject sits on; mc-tabs with mc-tab
(aria-selected); mc-switch (role=switch, aria-checked); mc-skeleton; motion
classes mc-in and mc-pop that stagger by the --i custom property.
Script helpers on window.mc:
- mc.h(tag, attrs, ...children) builds elements; attrs take class, style
  objects, onclick-style handlers and plain attributes.
- mc.icon(name, {size?}) returns an inline SVG icon; mc.icons lists the names
  (check x plus minus chevron-* arrow-right arrow-left search settings user file
  folder trash copy edit play pause replay more menu bell mail home star heart
  link lock calendar clock download upload filter sun moon info alert sparkle
  terminal code git).
- mc.variants(target, [{name, note?, render(stage, ctx)}], {stage?, stageHeight?,
  controls?, replay?, playback?, autoplay?}) builds a variant switcher: tabs, a plain stage (pass {stage:'dots'} or 'grid-lines' for a pattern),
  a Replay button, a one-line note and a Playback speed slider that slows every
  CSS and Web Animations animation. render fills the stage and may return a
  cleanup function; it runs again on every switch and replay, so entrance
  animations replay for free. ctx.now() is playback-scaled time for
  requestAnimationFrame loops. Arrow keys switch variants, r replays.
- mc.slider(label, {min,max,step,value,unit?,format?,onInput}),
  mc.toggle(label, value, onChange), mc.tabs(labels, selected, onChange) return
  ready controls; put them in the variants controls option or your own bar.
- mc.stagger(elements) sets --i on each; mc.series(i) is chart color i.
- mc.chart(target, {type, labels?, series?, values?, height?, x?, y?, legend?,
  curve?, animate?}) draws a themed, responsive SVG chart with hover tooltips, a
  crosshair and a legend that toggles series. type: line, area, bar (grouped),
  stacked, hbar (horizontal; labels down the side), scatter or donut.
  series: [{name, values, color?}], values aligned with labels; scatter values
  are [x, y, r?, label?]; donut takes labels and values (plus format, center,
  centerLabel). x and y take {label, format, min, max, zero}; format is a
  function or a unit like '$', '%' or ' ms'. Returns {el, update(partialSpec)}
  for controls that change the data. Prefer it to a CDN chart library.
- mc.flow(target, {nodes:[{id, label, detail?, icon?, tone?}], edges:[[from, to,
  label?] or {from, to, label, dashed}], direction?, onSelect?}) lays out a
  diagram from its edges: ranks run left to right and wrap to new rows, or
  direction 'down' stacks them. tone: accent, success, warning, danger, muted.
  Returns {el, focus(ids)}; focus highlights those nodes and animates the edges
  between them.
- mc.steps(target, [{title, body, focus?, render?(el)}], {flow?, autoplay?,
  initial?}) is a stepper with previous/next, progress and arrow keys; with
  {flow} each step focuses its nodes, and render can draw anything per step.
- mc.code(source, {lang, title?, numbers?, highlight?: [lines], wrap?}) returns a
  highlighted code block (js/ts, rust, python, go, c-like, sh, sql, css, json,
  html/xml, diff); mc.highlight(source, lang) returns the HTML.
- mc.table(rows, {columns?: [key or {key, label, format(v,row)}], sort?,
  maxHeight?}) returns a sortable table; numbers align right.
- mc.data is the data files passed with the visual (see Data files).
Images in a page open in the app's image viewer when clicked, unless they sit in
a link or button or the page handles the click itself.

Data files. To chart or tabulate real data, pass {data: {name: "/abs/path"}} to
visual_check, visual_preview and visual_show (CLI input JSON takes the same
field). Up to 8 files of 2 MiB each: .json is parsed, .csv and .tsv become
arrays of row objects keyed by the header (numbers as numbers, empty cells as
null, leading-zero codes kept as text), anything else arrives as a string. The
page reads them as mc.data.name. Aggregate large data first (with a script) and
pass the result; never paste big datasets into the HTML.

Make HTML visuals worth opening. A single static picture is the weak result.
Treat each request as a small product demo:
- UI, interaction or animation ideas: build 3 to 5 genuinely different variants
  with mc.variants, each with a short note that says what it changes. Mount it on
  document.body. Animation plays on load; Replay
  and Playback come from the kit.
- UI mockups: real product content (names, dates, numbers, icons), the states
  that matter (hover, selected, empty, loading, error), a focused subject on an
  mc-stage instead of a whole fake website, and working interactions (open and
  close, select, type, toggle).
- Data: real numbers through data files, an mc.chart with labelled axes and
  units, the two or three numbers that matter as mc-stat, controls (range,
  grouping, metric) that call update, and an mc.table when readers will want
  exact values.
- Explanations, architecture and flows: an mc.flow driven by mc.steps, so the
  reader advances through the story while the diagram highlights each part; add
  mc.code for the lines that matter.
Size by content: mc-variants has a stage of at least 320 points; pass
{stageHeight} to change it. Respect
prefers-reduced-motion (the kit already shortens CSS motion). A script error
leaves a blank page, so preview every time.
"#;

/// A rendered page: what the agent needs to judge it before publishing.
pub struct Preview {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub content_height: u32,
    pub loaded: bool,
    pub console: Vec<(String, String)>,
}

pub struct PreviewRequest {
    pub html: String,
    pub width: u32,
    pub height: u32,
    /// Render on a light surface instead of a dark one; None follows the app.
    pub light: Option<bool>,
    pub reply: mpsc::Sender<Result<Preview, String>>,
}

type PreviewHandler = Box<dyn Fn(PreviewRequest) + Send + Sync>;
static PREVIEW_HANDLER: OnceLock<PreviewHandler> = OnceLock::new();

/// The app registers the renderer once; the loopback hub forwards requests to it.
pub fn set_preview_handler(handler: impl Fn(PreviewRequest) + Send + Sync + 'static) {
    let _ = PREVIEW_HANDLER.set(Box::new(handler));
}

pub const PREVIEW_WIDTHS: std::ops::RangeInclusive<u32> = 240..=1600;
pub const PREVIEW_HEIGHTS: std::ops::RangeInclusive<u32> = 120..=1200;
const MAX_PREVIEW_RESPONSE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Wire {
    session: i64,
    token: String,
    request_id: String,
    document: VisualDocument,
}

pub struct Publication {
    pub document: VisualDocument,
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
    if document.format == VisualFormat::Native && document.source.len() > MAX_SOURCE_BYTES {
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
    match document.format {
        VisualFormat::Native => visual::check(&document.source).map(drop),
        VisualFormat::Html => check_html(&document.source),
    }
}

/// The page runs under a strict CSP, so network and external resources fail
/// silently in the view. Say so before publishing instead.
pub fn check_html(html: &str) -> Result<(), String> {
    // Embedded images count against their own budget, not the page's.
    let images: usize = embedded_images()
        .find_iter(html)
        .map(|image| image.len())
        .sum();
    let data = html
        .strip_prefix(visual::data::BLOCK_START)
        .and_then(|rest| rest.find("</script>"))
        .map_or(0, |end| visual::data::BLOCK_START.len() + end);
    if html.len().saturating_sub(images + data) > MAX_SOURCE_BYTES {
        return Err("Visual HTML exceeds 64 KiB".into());
    }
    if html.len() > visual::images::MAX_HTML_BYTES {
        return Err("Visual HTML with its images and data exceeds 10 MiB".into());
    }
    if html.trim().is_empty() {
        return Err("HTML must not be empty".into());
    }
    let lower = html.to_ascii_lowercase();
    // Code and styles may come from a short list of CDNs over https; the
    // page's CSP admits exactly those origins.
    let allowed = |url: &str, hosts: &[&str], what: &str| -> Result<(), String> {
        if !(url.starts_with("http") || url.starts_with("//")) {
            return Ok(());
        }
        let host = url.strip_prefix("https://").map(|rest| {
            rest.split(['/', '?', '#', ':', '"', '\'', ' ', ')'])
                .next()
                .unwrap_or("")
        });
        match host {
            Some(host) if hosts.contains(&host) => Ok(()),
            _ => Err(format!(
                "{what} may only load from https://{}",
                hosts.join(", https://")
            )),
        }
    };
    let values = |tag: &str, attr: &str| -> Vec<String> {
        lower
            .match_indices(tag)
            .filter_map(|(at, _)| {
                let end = lower[at..].find('>').map_or(lower.len(), |n| at + n);
                let element = &lower[at..end];
                let i = element.find(attr)?;
                let rest = element[i + attr.len()..].trim_start_matches(['=', ' ', '"', '\'']);
                Some(rest.to_owned())
            })
            .collect()
    };
    for url in values("<script", "src") {
        allowed(&url, &visual::SCRIPT_HOSTS, "Scripts")?;
    }
    for url in values("<link", "href") {
        allowed(&url, &visual::STYLE_HOSTS, "Stylesheets")?;
    }
    for (at, _) in lower.match_indices("@import") {
        let rest = lower[at + 7..]
            .trim_start()
            .trim_start_matches("url(")
            .trim_start_matches(['"', '\'', ' ']);
        allowed(rest, &visual::STYLE_HOSTS, "@import")?;
    }
    if values("<img", "src")
        .iter()
        .any(|url| url.starts_with("http") || url.starts_with("//"))
    {
        return Err(
            "This image was not embedded; give it as an https URL or an absolute file path to an image file"
                .into(),
        );
    }
    for api in [
        "fetch(",
        "xmlhttprequest",
        "websocket",
        "eventsource",
        "sendbeacon",
    ] {
        if lower.contains(api) {
            return Err(format!("The page has no network access ({api} is blocked)"));
        }
    }
    Ok(())
}

/// What the agent wrote, with its images and data files embedded.
fn prepare_html(html: &str, input: &Value) -> Result<String, String> {
    let html = visual::images::embed_images(html)?;
    visual::data::embed_data(&html, &input["data"])
}

fn embedded_images() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"data:image/[a-zA-Z0-9.+-]+;base64,[A-Za-z0-9+/=]+").unwrap()
    })
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

/// One page renders at a time; a preview needs the app, never a session turn.
fn serve_preview(mut stream: TcpStream, hub: &Hub, wire: &Value) -> Result<(), String> {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let response = (|| -> Result<Value, String> {
        authorized(
            hub,
            wire["session"].as_i64().ok_or("Invalid visual session")?,
            wire["token"].as_str().ok_or("Invalid visual capability")?,
        )?;
        let input = &wire["preview"];
        let html = input["html"].as_str().ok_or("html must be a string")?;
        check_html(html)?;
        let dimension = |key: &str, default: u32, range: &std::ops::RangeInclusive<u32>| match input
            .get(key)
            .filter(|v| !v.is_null())
        {
            None => Ok(default),
            Some(value) => value
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| range.contains(v))
                .ok_or_else(|| format!("{key} must be {}–{}", range.start(), range.end())),
        };
        let width = dimension("width", 640, &PREVIEW_WIDTHS)?;
        let height = dimension("height", 480, &PREVIEW_HEIGHTS)?;
        let light = match input.get("appearance").and_then(Value::as_str) {
            None => None,
            Some("light") => Some(true),
            Some("dark") => Some(false),
            Some(_) => return Err("appearance must be light or dark".into()),
        };
        let handler = PREVIEW_HANDLER
            .get()
            .ok_or("Previews need the MonoCode desktop app")?;
        if BUSY.swap(true, Ordering::AcqRel) {
            return Err("Another preview is rendering; try again in a moment".into());
        }
        let (reply, result) = mpsc::channel();
        handler(PreviewRequest {
            html: html.to_owned(),
            width,
            height,
            light,
            reply,
        });
        let preview = result.recv_timeout(Duration::from_secs(40));
        BUSY.store(false, Ordering::Release);
        let preview = preview.map_err(|_| "The preview timed out".to_string())??;
        use base64::Engine;
        Ok(json!({
            "ok": true,
            "result": {
                "png": base64::engine::general_purpose::STANDARD.encode(&preview.png),
                "width": preview.width,
                "height": preview.height,
                "contentHeight": preview.content_height,
                "loaded": preview.loaded,
                "console": preview.console.iter()
                    .map(|(level, text)| json!({"level": level, "text": text}))
                    .collect::<Vec<_>>(),
            }
        }))
    })()
    .unwrap_or_else(|error| json!({"ok": false, "error": error}));
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{response}").map_err(|e| e.to_string())
}

fn serve(mut stream: TcpStream, hub: &Hub) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new((&stream).take((MAX_HUB_BYTES + 1) as u64))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if line.len() > MAX_HUB_BYTES || !line.ends_with('\n') {
        return Err("Invalid visual request".into());
    }
    if let Ok(value) = serde_json::from_str::<Value>(&line) {
        if value.get("preview").is_some() {
            return serve_preview(stream, hub, &value);
        }
    }
    let wire: Wire = serde_json::from_str(&line).map_err(|_| "Invalid visual request")?;
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
        let signature =
            Sha256::digest(serde_json::to_vec(&wire.document).map_err(|e| e.to_string())?)
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
        valid_document(&wire.document)?;
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
                    document: wire.document,
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

/// Validates either kind: `{html}` or `{source}`.
pub fn check_input(input: &Value) -> Result<Value, String> {
    match (input["source"].as_str(), input["html"].as_str()) {
        (Some(_), Some(_)) => Err("Pass either source or html, not both".into()),
        (Some(source), None) => check(source),
        (None, Some(html)) => {
            height(input)?;
            let html = prepare_html(html, input)?;
            check_html(&html).map(|()| json!({"ok":true,"format":"html","bytes":html.len()}))
        }
        (None, None) => Err("Pass source (native) or html (HTML visual)".into()),
    }
}

fn height(input: &Value) -> Result<Option<u32>, String> {
    match input.get("height") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|h| u32::try_from(h).ok())
            .map(Some)
            .ok_or_else(|| "height must be a whole number of points".into()),
    }
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
    let (source, format) = match (input["source"].as_str(), input["html"].as_str()) {
        (Some(_), Some(_)) => return Err("Pass either source or html, not both".into()),
        (Some(source), None) => (source, VisualFormat::Native),
        (None, Some(html)) => (html, VisualFormat::Html),
        (None, None) => return Err("source must be a JavaScript string".into()),
    };
    let embedded;
    let source = if format == VisualFormat::Html {
        embedded = prepare_html(source, &input)?;
        embedded.as_str()
    } else {
        source
    };
    let height = height(&input)?;
    if format == VisualFormat::Html && height.is_none() {
        // The default is applied by the view; keep the document explicit.
    }
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
        format,
        height: (format == VisualFormat::Html).then(|| height.unwrap_or(DEFAULT_HTML_HEIGHT)),
    };
    valid_document(&document)?;
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
        document,
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

/// Ask the running app to render an HTML page and report what it looks like.
fn preview_with_environment(
    input: &Value,
    environment: &HashMap<String, String>,
) -> Result<Value, String> {
    let html = input["html"].as_str().ok_or("html must be a string")?;
    let html = &prepare_html(html, input)?;
    check_html(html)?;
    let get = |key| {
        environment
            .get(key)
            .ok_or("Previews require a running MonoCode session")
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
        .set_read_timeout(Some(Duration::from_secs(50)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut preview = json!({"html": html});
    for key in ["width", "height", "appearance"] {
        if let Some(value) = input.get(key).filter(|v| !v.is_null()) {
            preview[key] = value.clone();
        }
    }
    writeln!(
        stream,
        "{}",
        json!({"session": session, "token": get(ENV_TOKEN)?, "preview": preview})
    )
    .map_err(|e| e.to_string())?;
    let mut response = String::new();
    BufReader::new(stream.take(MAX_PREVIEW_RESPONSE_BYTES as u64))
        .read_line(&mut response)
        .map_err(|e| e.to_string())?;
    let response: Value =
        serde_json::from_str(&response).map_err(|_| "The app closed without a preview")?;
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("Preview failed")
            .into());
    }
    Ok(response["result"].clone())
}

fn current_environment() -> HashMap<String, String> {
    [ENV_ENDPOINT, ENV_TOKEN, ENV_SESSION, ENV_EXECUTABLE]
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
            json!({"help":format!("Usage: monocode-gpui visual docs\n       monocode-gpui visual check|show|preview --input FILE [--title TITLE] [--id ID]\nFILE is JavaScript source, or JSON with source/title/id/requestId. Use - for stdin.\n{DOCUMENTATION}")}),
        );
    };
    if action == "docs" {
        if args.len() != 1 {
            return Err("Usage: monocode-gpui visual docs".into());
        }
        return Ok(documentation());
    }
    if !matches!(action.as_str(), "check" | "show" | "preview") {
        return Err("Expected visual docs, visual check or visual show".into());
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
    if action == "preview" {
        preview_with_environment(&input, &current_environment())
    } else if action == "check" {
        check_input(&input)
    } else {
        publish_with_environment(input, &current_environment())
    }
}

fn documentation() -> Value {
    json!({
        "documentation": DOCUMENTATION,
        "cli": {
            "check": "monocode-gpui visual check --input FILE",
            "show": "monocode-gpui visual show --input FILE --title TITLE"
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
    server["name"] = "monocode_visual".into();
    server["env"] = [ENV_ENDPOINT, ENV_TOKEN, ENV_SESSION, ENV_EXECUTABLE]
        .into_iter()
        .map(|key| json!({"name":key,"value":environment[key]}))
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
        (
            "env_vars",
            json!([ENV_ENDPOINT, ENV_TOKEN, ENV_SESSION, ENV_EXECUTABLE]),
        ),
    ] {
        // JSON's strings and arrays are valid TOML literals for these values.
        command.args(["-c", &format!("mcp_servers.monocode_visual.{key}={value}")]);
    }
    // Codex's bundled visualize plugin asks the model for a content reference
    // only the Codex app can render; in MonoCode it shows up as raw text. An
    // inline table merges into the user's plugin settings, so only this entry
    // changes. (A quoted dotted key is not parsed by Codex's -c.)
    command.args(["-c", CODEX_VISUALIZE_OFF]);
}

const CODEX_VISUALIZE_OFF: &str = r#"plugins={"visualize@openai-bundled"={enabled=false}}"#;

pub fn tool_definitions() -> Vec<Value> {
    let properties = json!({
        "source":{"type":"string","description":"Native kind: JavaScript function body using the visual SDK"},
        "html":{"type":"string","description":"HTML page instead of source"},
        "height":{"type":"integer","minimum":120,"maximum":900,"description":"HTML frame height"},
        "data":{"type":"object","additionalProperties":{"type":"string"},"description":"HTML: name to file path, read as mc.data"}
    });
    let mut show_properties = properties.clone();
    show_properties["title"] = json!({"type":"string","maxLength":160});
    show_properties["id"] = json!({"type":"string","maxLength":96,"description":"Optional document identifier. A new request appends a new visual; reuse requestId to retry a publication."});
    show_properties["requestId"] = json!({"type":"string","maxLength":128,"description":"Reuse when retrying an uncertain publication"});
    vec![
        json!({"name":"visual_docs","description":"Read the native visual SDK reference, examples and styling rules when preparing an inline visual. Does not publish.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"visual_check","description":"Validate a native or HTML visual without publishing. Read visual_docs.","inputSchema":{"type":"object","properties":properties,"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"visual_preview","description":"Render an HTML page in the app: returns a screenshot, contentHeight and console errors. Use before visual_show; see visual_docs.","inputSchema":{"type":"object","properties":{"html":{"type":"string"},"width":{"type":"integer","minimum":240,"maximum":1600},"height":{"type":"integer","minimum":120,"maximum":1200},"appearance":{"enum":["light","dark"]},"data":{"type":"object","additionalProperties":{"type":"string"}}},"required":["html"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":true}}),
        json!({"name":"visual_show","description":"Publish a native or HTML visual into the conversation. Read visual_docs; validate first.","inputSchema":{"type":"object","properties":show_properties,"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
    ]
}

fn mcp_response(request: Value, environment: &HashMap<String, String>) -> Option<Value> {
    let id = request.get("id")?.clone();
    let result = match request["method"].as_str().unwrap_or("") {
        "initialize" => Ok(
            json!({"protocolVersion":request["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"monocode_visual","version":env!("CARGO_PKG_VERSION")}}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools":tool_definitions()})),
        "tools/call" => {
            let input = request["params"]["arguments"].clone();
            let result = match request["params"]["name"].as_str().unwrap_or("") {
                "visual_docs" => Ok(documentation()),
                "visual_check" => check_input(&input),
                "visual_show" => publish_with_environment(input, environment),
                "visual_preview" => preview_with_environment(&input, environment),
                _ => Err("Unknown native visual tool".into()),
            };
            let is_preview = request["params"]["name"] == "visual_preview";
            Ok(match result {
                Ok(mut value) if is_preview => {
                    // The picture is an image block; the text keeps the numbers
                    // and console output without a megabyte of base64.
                    let png = value["png"].take();
                    let note = json!({
                        "width": value["width"],
                        "height": value["height"],
                        "contentHeight": value["contentHeight"],
                        "loaded": value["loaded"],
                        "console": value["console"],
                    });
                    json!({"content":[
                        {"type":"image","data":png,"mimeType":"image/png"},
                        {"type":"text","text":note.to_string()}
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
        assert_eq!(publication.document.title, "Hello");
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
    fn html_visuals_publish_with_a_default_height_and_need_exactly_one_kind() {
        let server = Server::start(11).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        let worker = std::thread::spawn(move || {
            publish_with_environment(
                json!({"html":"<h1>Hi</h1>","title":"Page","requestId":"html-1"}),
                &environment,
            )
        });
        let publication = server
            .publications
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(publication.document.format, VisualFormat::Html);
        assert_eq!(publication.document.height, Some(DEFAULT_HTML_HEIGHT));
        publication.reply.send(Ok(json!({"id":"x"}))).unwrap();
        worker.join().unwrap().unwrap();
        assert!(
            check_input(&json!({"source":"return () => visual.text('a')","html":"<p>a</p>"}))
                .is_err()
        );
        assert!(check_input(&json!({})).is_err());
        assert!(check_input(&json!({"html":"<p>a</p>","height":50})).is_ok());
        let mut document = VisualDocument {
            id: "page".into(),
            title: "Page".into(),
            source: "<p>a</p>".into(),
            format: VisualFormat::Html,
            height: Some(50),
        };
        assert!(valid_document(&document).is_err());
        document.height = Some(400);
        assert!(valid_document(&document).is_ok());
    }

    #[test]
    fn embedded_images_have_their_own_budget_and_reach_the_published_page() {
        let image = format!("data:image/png;base64,{}", "A".repeat(200 * 1024));
        assert!(check_html(&format!("<img src=\"{image}\">")).is_ok());
        assert!(check_html(&format!("<p>{}</p>", "x".repeat(MAX_SOURCE_BYTES))).is_err());
        let huge = format!("data:image/png;base64,{}", "A".repeat(11 * 1024 * 1024));
        assert!(check_html(&format!("<img src=\"{huge}\">"))
            .unwrap_err()
            .contains("10 MiB"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Screenshot 2026-10-26 at 9.41.12 AM.png");
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(4, 4)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&path, png.into_inner()).unwrap();
        let input = json!({"html": format!("<img src=\"{}\">", path.display())});
        let checked = check_input(&input).unwrap();
        assert_eq!(checked["format"], "html");
    }

    #[test]
    fn html_checks_allow_https_libraries_and_reject_everything_else_remote() {
        assert!(check_html(
            "<svg viewBox='0 0 10 10'><circle r='4'/></svg><script>let a=1</script>"
        )
        .is_ok());
        assert!(check_html("<p>See https://example.com for more</p>").is_ok());
        assert!(check_html(
            "<script src=\"https://cdn.jsdelivr.net/npm/chart.js@4\"></script><link rel=stylesheet href=https://fonts.googleapis.com/css2?family=Inter><script src=https://unpkg.com/x></script>"
        )
        .is_ok());
        for bad in [
            "<script src=\"https://evil.example/x.js\"></script>",
            "<script src=\"https://cdn.jsdelivr.net.evil.example/x.js\"></script>",
            "<link rel=stylesheet href=https://x.test/a.css>",
            "<style>@import url(https://evil.example/a.css)</style>",
            "<script src=\"http://cdn.example/x.js\"></script>",
            "<script src='//cdn.example/x.js'></script>",
            "<link rel=stylesheet href=http://x/y.css>",
            "<style>@import url(http://x/y.css)</style>",
            "<img src=\"https://x/y.png\">",
            "<script>fetch('/a')</script>",
            "<script>new WebSocket('ws://x')</script>",
            "   ",
        ] {
            assert!(check_html(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn previews_authenticate_validate_and_reach_the_registered_renderer() {
        let server = Server::start(21).unwrap();
        let environment = server.environment(std::path::Path::new("/test/app"));
        // No renderer yet: a clear error, and bad input is rejected first.
        assert!(preview_with_environment(
            &json!({"html":"<script src='http://x'></script>"}),
            &environment
        )
        .is_err());
        set_preview_handler(|request| {
            let _ = request.reply.send(Ok(Preview {
                png: vec![137, 80, 78, 71],
                width: request.width,
                height: request.height,
                content_height: 321,
                loaded: true,
                console: vec![("error".into(), "boom".into())],
            }));
        });
        let result = preview_with_environment(
            &json!({"html":"<p>hi</p>","width":500,"height":300}),
            &environment,
        )
        .unwrap();
        assert_eq!(result["contentHeight"], 321);
        assert_eq!(result["width"], 500);
        assert_eq!(result["console"][0]["text"], "boom");
        assert!(
            preview_with_environment(&json!({"html":"<p>hi</p>","width":10}), &environment)
                .is_err()
        );
        let mut wrong = environment.clone();
        wrong.insert(ENV_TOKEN.into(), "nope".into());
        assert!(preview_with_environment(&json!({"html":"<p>hi</p>"}), &wrong).is_err());
        // The tool returns an image block plus the numbers, not base64 text.
        let response = mcp_response(
            json!({"id":3,"method":"tools/call","params":{"name":"visual_preview","arguments":{"html":"<p>hi</p>"}}}),
            &environment,
        )
        .unwrap();
        let content = &response["result"]["content"];
        assert_eq!(content[0]["type"], "image");
        assert!(content[1]["text"]
            .as_str()
            .unwrap()
            .contains("contentHeight"));
        assert!(!content[1]["text"].as_str().unwrap().contains("png"));
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
        assert_eq!(tool_definitions().len(), 4);
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
        assert!(initialization["result"].get("instructions").is_none());
        let catalog = mcp_response(json!({"id":2,"method":"tools/list"}), &environment).unwrap();
        let metadata = catalog["result"].to_string();
        assert!(
            metadata.len() < 2_500,
            "Discovery must stay small: {} bytes",
            metadata.len()
        );
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
            .contains("visual.scene"));
        assert!(reference["documentation"]
            .as_str()
            .unwrap()
            .contains("Style is inherited from the app"));
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
        let environment = server.environment(std::path::Path::new("/test/app"));
        let mut command = std::process::Command::new("provider");
        configure_codex(&mut command, &environment);
        let arguments = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(arguments.contains("visual-mcp"));
        assert!(arguments.contains(ENV_TOKEN));
        assert!(!arguments.contains(&environment[ENV_TOKEN]));
        // Only MonoCode's own visual tools stay available to the model.
        assert!(arguments.contains(CODEX_VISUALIZE_OFF));
        let mut plain = std::process::Command::new("provider");
        configure_codex(&mut plain, &HashMap::new());
        assert_eq!(plain.get_args().count(), 0);
        let acp = acp_servers(&environment);
        assert_eq!(acp[0]["name"], "monocode_visual");
        assert_eq!(acp[0]["env"].as_array().unwrap().len(), 4);
        assert_eq!(mcp_server(&environment).unwrap()["command"], "/test/app");
    }
}
