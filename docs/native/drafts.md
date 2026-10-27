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
  the draft's id), then the page loads once the card is painted. A draft needs
  a running turn in the open conversation; a background conversation or a card
  scrolled out of view gets an error.
- The card is as wide as the column (up to the page's width, scaled down when
  the page is wider) and as tall as the page content unless the agent sets a
  height. Up to three drafts stay live per conversation; the oldest is retired
  and its card turns into a "closed" note. Drafts do not survive a restart.
- Agent clicks and typing are sent as script events. They never take focus from
  the user's composer. When the person clicks or types in the page they have
  control: snapshots still work, actions are refused until `resume`.
- HTML runs in a sandboxed frame with a content security policy: inline scripts
  and styles, data images, and a CDN allowlist for scripts, styles and fonts;
  no network from the page. Markup may be 512 KiB, plus embedded images.
  A `url` must be on localhost, so the card doubles as a dev-server browser.
- Pages receive the app's theme through CSS variables, derived from the real
  theme at the time of the call, and follow `appearance:"light"|"dark"`.
- The native view is placed from the card's bounds on every paint, clipped to
  the thread's visible area, and hidden when its row is not painted. A popover
  over it swaps in a still.

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
