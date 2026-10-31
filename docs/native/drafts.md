# Drafts

An agent can build a live page inside the conversation, look at it, and let
you use it. The page is a card in the thread at the point where the agent made
it: a WebKit view hosted in MonoCode's own window (macOS only), clipped to the
transcript as it scrolls. It is not a side panel and not a separate window.
Native visuals (`visual_show`) are unchanged; see [session visuals](visuals.md).

## Tools

`visual_docs` with `{"topic": "draft"}` returns the full guide; `kit` returns
the optional component kit and `app` returns MonoCode's real design language
(theme-derived ink alphas, radii, type and sizes) so a mock of MonoCode looks
like MonoCode.

The kit has an optional controls part: `mc.state` (a store mirrored to
`--s-*` CSS variables and `data-s-*` attributes) and `mc.controls` (sliders,
segmented controls, selects, steppers, swatches, text, toggles and buttons in a
rail, bar or dock outside the content). A page that uses them reports the
values as `state` in every snapshot. `examples/controls-kit-demo.html` shows
the three layouts and `examples/controls-kit-all.html` every control type;
`scripts/verify-native-draft.py` drives both.

`draft_preview` takes `html` or a localhost `url` plus a short `title` to start
a draft, or `draft` to continue one, with an optional `action`:

| Action | Effect |
| --- | --- |
| `{kind:"snapshot"}` (default) | Look again, optionally at a new `width`, `height` or `appearance` |
| `{kind:"click", reference}` | Click a control by the ref from the last snapshot |
| `{kind:"type", text, replace}` | Type into the focused input |
| `{kind:"resume"}` | Take control back after the person had it |
| `{kind:"close"}` | Close the draft |

Every call returns a screenshot, the page text, controls with refs, console
output and diagnostics. The screenshots go to the agent only: the thread shows
the card, not a picture per call. `draft_keep {draft, title}` saves a picture
and the page; the thread gets a picture card whose **Open draft** button turns
it back into the live page in place, after the draft is gone or the app was
restarted.

## Feedback loop

The agent can look, fix and look again on its own, and the person can answer
from the page itself:

- **Comment** on the card turns on comment mode. Clicking the page pins a
  numbered note to the element under the click; **Send N notes** hands them
  over. Notes carry the element's tag and label and their place on the page,
  and the pins show in the agent's screenshots.
- `draft_wait {draft, seconds?}` pauses the agent (up to 30 seconds per call)
  until the person sends notes or presses **Hand back**. The card shows
  "Waiting for you". It returns the usual screenshot and numbers with
  `feedback`, `userActions` (clicks they made by hand) and `event`
  (`feedback`, `handed back` or `timeout`).
- If the agent was not waiting, **Send** delivers the notes as a message in the
  conversation, queued to steer a running turn where the provider can take it.
- Every result also lists `issues` the page reveals: low contrast, small tap
  targets, controls without a name, images without alt text, clipped text.
- The draft tool calls themselves are hidden from the thread; the card is the
  display.

## Behavior

- A new draft first publishes its card into the running turn (the bridge picks
  the draft's id). Each agent request holds a lease on the page, so it opens
  and runs whether or not the card is on screen: WebKit parks the view clear
  and out of the pointer's way in the window, Chromium runs its tab off
  screen. If Chromium gives up while the agent waits (it is tried three
  times), the lease moves to WebKit and the page opens there. Without a lease
  an unseen page pauses.
- The card is as wide as the column (up to the page's width, scaled down when
  the page is wider) and as tall as the page content unless the agent sets a
  height. Up to three drafts stay live per conversation; the oldest is retired
  and its card turns into a "closed" note with **Open again**.
- The app saves every draft as it changes (page, title, size, appearance and
  `mc.state`) in `threads/<session>/drafts/<id>.json` beside the
  conversation's other files. After a restart a card whose page is not open
  opens it again from there the first time the card is drawn, under the same
  id, so the agent can keep working with it. Closing a draft deletes its file;
  deleting the conversation deletes the folder.
- Agent clicks and typing are sent as script events. They never take focus from
  the user's composer. When the person clicks or types in the page they have
  control: snapshots still work, actions are refused until `resume`. The
  page's own scripts can post the same reports, so a report only counts within
  two seconds of a real click or key press in the view: a page cannot take
  control, pin notes or message the agent for the person.
- The page pings the app (a main-frame-only `mcWake` message) whenever it
  reports something, and the app reads it then, so takeover, notes and Send
  land at once. A one-second tick backs the ping up.
- HTML runs in a sandboxed frame with a content security policy: inline scripts
  and styles, data images, and a CDN allowlist for scripts, styles and fonts;
  no network from the page. Markup may be 512 KiB, plus embedded images.
  A `url` must be on localhost, so the card doubles as a dev-server browser.
- Pages receive the app's theme through CSS variables, derived from the real
  theme at the time of the call, and follow `appearance:"light"|"dark"`.
- The native view is placed from the card's bounds on every paint, clipped to
  the thread's visible area, and hidden when its row is not painted. A popover
  over it swaps in a still.

## Page engine

**Settings › Visualizations › Page engine** picks what draws drafts. Open
drafts move when it changes: the card shows a picture of the page while the
other engine loads it, and `mc.state` and pins come back from the shared
record. Anything else the page kept (scroll, focus, typed text outside
`mc.state`) starts over.

- **WebKit** (default) is built into macOS: the native view described above.
- **Chromium** is the Chromium Embedded Framework (CEF), downloaded the first
  time it is chosen (about 135 MB, 330 MB unpacked) into
  `~/Library/Application Support/<app>/cef/<version>`. One version is pinned
  in `src/chromium.rs` and the archive must match its SHA-256 before it is
  unpacked. Until it is here, and for any draft whose Chromium fails to start
  three times, drafts use WebKit. Settings › Visualizations then says why,
  from the host's last `FATAL` log line or panic, until a page opens in
  Chromium again.
- CEF runs in its own process, `monocode-cef-host`
  (`crates/monocode-cef-host`), which ships beside the app's executable and
  is the only code that loads the framework: the download is not signed by
  this team, so the host is signed to allow that (`Entitlements.plist`) and
  the app keeps its strict signing. The app lays out a small `MonoCode
  CEF.app` around the download with copies of the host it ships, replaced
  when the app's copy changes, because CEF needs its helper app and framework
  in fixed places. Copies, not links: Chromium starts each helper by its
  resolved path, and a linked helper looks for the framework beside the app
  and quits.
- One host serves every Chromium draft and quits with the last. Each draft is
  a browser there with in-memory storage of its own, driven over DevTools
  (commands on fd 3, events on fd 4). Frames never leave the GPU: CEF renders
  off screen into an IOSurface, the host copies it into one of three
  surfaces of its own and sends that over Mach, and the card draws it with
  GPUI's `paint_surface`. So the card is ordinary GPUI content: popovers and
  scrims cover it with no still and no sentinel. A static page sends one
  frame and stops; a card out of sight hides its browser, and a page out of
  sight is frozen, as WebKit pauses a hidden one, unless the agent is working
  with it. The person's mouse and keys reach the page as DevTools input; copy
  and paste go through the app's clipboard.
- The same guards apply: the agent's page is served from a fixed origin by
  request interception and may navigate nowhere else, a `url` draft may load
  only local addresses, other links (and new windows the person opens) go to
  your browser, downloads and file pickers are refused, and dialogs are
  dismissed.
- Not drawn: `<select>` dropdown lists, tooltips and context menus. Text input
  takes typed characters; IME composition is not passed through. A page more
  than 8,192 points tall is drawn at a lower scale.

## On the phone

A draft belongs to its thread, not to one screen. A paired phone shows each
draft, and each picture kept from one, as a card in the conversation. Tapping it
opens the page in a full-screen sheet:

- The phone draws its own copy of an HTML draft in a web view, with the same
  sandbox and content security policy, laid out at the phone's width and
  themed like the phone. A kept draft opens live for both screens, as
  **Open draft** does on the desktop.
- What the two copies share is one record per draft, held by the desktop
  (`SharedDraft` in `monocode-core`): the page's `mc.state`, the notes and
  their pins, who has control, and whether the agent is waiting. Move a
  control on the phone and the card follows; pin a note on either screen and
  it shows on both, with one **Send N notes** for all of them. **Hand back**
  works from either screen.
- Scroll, focus and anything a page keeps outside `mc.state` stay on the screen
  where they changed. The agent's clicks and typing act on the desktop's copy.
- The phone reads the record with `draft.read`, which waits up to 15 seconds
  for it to change, and changes it with `draft.action` (`pin`, `send`,
  `input`, `state`, `message`, `handBack`). Its page reports reach the desktop
  only right after the person's own touch, and notes only in comment mode, so
  a page's script cannot speak for the person.
- A localhost `url` draft cannot load on the phone. The phone shows pictures
  of the desktop's copy instead, about once a second, from `draft.frame`; it
  cannot be touched there.
- A pin carries the element it was placed on (its child indexes from `<body>`
  and the point inside its box), so it lands on the same element at the other
  screen's width and moves with it when the page lays out again. The agent's
  notes carry only the percentages.
- The agent still sees the desktop's copy. Its requests lease that page, so
  the conversation need not be open there, but the desktop app must be
  running.

## Verifying

```sh
cd crates/monocode-gpui && cargo build --profile native-dev
python3 scripts/verify-native-draft.py
```

The script opens a self-test window with the draft inside a thread, drives `docs/native/examples/draft-spike.html`
through snapshot, click, type, takeover, resume, resize, keep and close, and
checks what the agent would receive. For ad hoc pages set
`MONOCODE_DRAFT_SELFTEST=page.html` (debug builds, add
`MONOCODE_DRAFT_SELFTEST_THREAD=1` to draw it in a thread); `MONOCODE_DRAFT_ACTIONS`
takes a JSON array of further `draft_preview` inputs, and it writes
`page.html.N.png` and `.json` for each step.

To run the same checks in Chromium, build the host and lay out a bundle
around the CEF SDK its build downloaded, then point the self-test at it
(debug builds):

```sh
cd crates/monocode-cef-host
CEF_PATH=~/Library/Caches/monocode-cef cargo build
../../scripts/cef-host-bundle.sh target/debug/monocode-cef-host \
  ~/Library/Caches/monocode-cef/156.0.14/cef_macos_aarch64/"Chromium Embedded Framework.framework" /tmp/mc-cef
cd ../.. && MONOCODE_DRAFT_ENGINE=chromium MONOCODE_CEF_APP="/tmp/mc-cef/MonoCode CEF.app" \
  python3 scripts/verify-native-draft.py
```

`target/debug/cef-smoke "/tmp/mc-cef/MonoCode CEF.app" frame.tga` drives the
host on its own and saves a frame. Release builds (`npm run native:build`)
build the host and sign it into the app bundle; that needs cmake and ninja.
