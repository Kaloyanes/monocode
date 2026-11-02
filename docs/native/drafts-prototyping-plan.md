# Drafts as a prototyping tool: audit and plan

Goal: a draft is for fast prototyping. The agent reads what the person wants and
builds the controls that fit, with full freedom over the page. The app supplies
good parts; it never forces chrome on a draft that does not need it.

## Audit

### Scroll hijack (bug)

`draft/web.rs` puts a plain `WKWebView` as a subview of the window's content
view, above the gpui view. A wheel event over the card goes to WebKit and is
never forwarded, so the thread stops scrolling while the pointer is over a
draft. Auto-height pages (the default) cannot scroll themselves, so the card
swallows scrolling for nothing.

### Controls out of step with the app

- The segmented control's selected pill uses `--background`, which is darker than
  its track: it reads as a hole. The app's own tab plate lifts the selected tab
  with a lighter ink alpha.
- A scripted click calls `focus()` on the control, and with no real pointer in
  the page WebKit treats that as keyboard focus, so a pink `:focus-visible`
  ring stays on the control the agent last clicked.
- `--ring` is the accent colour; the app's own focus treatment is quieter.

### Motion today

- `kit.js` already slows every CSS animation, transition and Web Animation
  (`mc.setSpeed`, patched `Element.prototype.animate`), and exposes a scaled
  clock (`mc.now`) for `requestAnimationFrame` loops. Only `mc.variants` with
  `playback:true` uses it, and only when the agent remembers to.
- Script-driven motion that does not use `mc.now` (GSAP, hand-written
  `performance.now` loops, `setTimeout` chains) is not slowed.
- `mc.variants` shows one variant at a time behind tabs. The reference example
  (a sidebar collapse, three variants) shows A, B and C side by side, driven by
  one Collapse/Expand button, one Slow motion switch and Auto-play.
- Nothing names a page's states ("collapsed", "expanded") or replays a
  transition, so every prototype hand-rolls its trigger buttons.

### What the new controls kit already gives

`mc.state` and `mc.controls` cover buttons, toggles, sliders and segmented
controls bound to page state, and the agent sees the values. They lack a
shared clock and any notion of a trigger that plays a transition.

## Principles

1. **Intent decides.** Motion prototypes get playback controls; page designs get
   tabs for the pages; three variants of one component sit side by side; a
   data explorer gets sliders. Nothing is added to a draft that does not need it.
2. **The agent chooses.** The app offers parts and documents when to use them.
   Any page, with or without the kit, stays valid.
3. **Controls live outside the content** and use the app's own look.

## Plan

### 1. Fix what is broken

- Forward wheel events from the web view to the thread when the page cannot
  consume them. The page reports whether it scrolls (document taller than its
  frame, or an inner scroller under the pointer); otherwise the web view hands
  the event to its parent view so gpui scrolls the thread.
- Restyle the segmented control with ink alphas (selected 10%, track 4%), and
  suppress the focus ring after a scripted click.

### 2. Motion parts the agent can opt into

- `mc.clock`: one shared speed and pause state. `mc.setSpeed` stays; add
  `mc.pause()`, `mc.resume()` and a time dilation for `performance.now`,
  `Date.now`, `requestAnimationFrame` timestamps and timer delays, installed by
  the page probe before page scripts so libraries that read those clocks slow
  down too.
- `mc.transport(target, {clock, replay, loop})`: Play/Pause, Replay, speed chips
  (1x, 1/2x, 1/4x) and a loop toggle, as a bar outside the content.
- `mc.states(target, {names, apply})`: named states with one trigger button that
  flips between them ("Collapse", "Expand"), plus "Auto-play" to loop it. This is
  the missing piece for transition prototypes.
- `mc.variants` gets `layout: 'side'` (columns, a title and one line each) next
  to the existing tabs, and every variant shares the clock and trigger.

### 3. Teach intent in `visual_docs`

A short table in the `draft` and `kit` topics:

| The person wants | Build |
| --- | --- |
| to compare 2 to 4 versions of one component | variants side by side, one shared trigger |
| to see an animation | states + transport, slow motion on, autoplay off until asked |
| to review a page or flow | tabs per page, device-width switch if responsive |
| to tune values | `mc.controls` bound to `mc.state` |
| to approve a look | a single static page, no controls |

State plainly that the table is a default: the agent may do anything the intent
calls for.

### 4. Verify

- Self-test: wheel over a draft scrolls the thread; wheel over a scrollable page
  scrolls the page.
- A live motion prototype built from the docs alone: three sidebar variants, slow
  motion, trigger and autoplay, run by an agent that never saw the example.

## Order

1. Scroll forwarding and control polish (bugs, small).
2. `mc.clock` and `mc.transport` (largest value, smallest surface).
3. `mc.states` and side-by-side `mc.variants`.
4. Docs table and the live verification.
