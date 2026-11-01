# Draft cards: duplicates, performance, resolution, design language

Audit of 2026-10-31 on how draft cards are stored, drawn and cached. Status is
at the end of each step. "Verified" means checked in code or in the stored
threads; "suspected" means a code-level reading that nobody has measured yet.

## Findings

Duplicates (verified in `monocode.db`): no thread stores one draft id twice.
Cards look doubled for four reasons.
- A retry creates a twin. In the Device Hub thread the first `draft_preview`
  published its card and then reported that the page was off screen; the agent
  retried with fresh `html`, which made a second draft with the same title.
- Each iteration without a `draft` id is a new card, and up to
  `LIVE_PER_CONVERSATION = 3` stay live (`shell/drafts.rs`).
- `draft_keep` adds a picture card next to the live card.
- `draft_row` (`transcript/pages.rs`) hands the one live entity to every row
  with that id, so a second view of the thread would draw the page twice
  (trigger not confirmed).

Performance:
- Verified: `holds_live_draft` (`conversation.rs`) turns off the cached replay
  of the whole transcript while any live draft is in the thread.
- Verified: every new version runs layout passes at 375, 768, 1280 and back
  (`draft.rs`, `measure_layouts`), with an 80 ms wait each, up to 900 ms.
- Suspected: the visible card jumps through widths during those passes.
- Up to three live pages at once; CEF costs roughly 2.5 times WebKit on an
  animating canvas and always renders at 2x.

Resolution (suspected, not measured): fractional card origin in
`chrome.rs`, a 1x placeholder, kept pictures capped at 1000 px
(`draft.rs`), agent screenshots on the same cap, unknown scale on the WebKit
snapshot.
Performance (measured; see "Measurements"):
- Verified: `holds_live_draft` (`conversation.rs`) turns off the cached replay
  of the whole transcript while any live draft is in the thread. Anything else
  that redraws then costs a full transcript paint: 0.6 to 1.1 ms a frame
  instead of 0.05 to 0.1.
- Verified: every new version ran layout passes at 375, 768, 1280 and back
  (`draft.rs`, `measure_layouts`), which held each revision back about 460 ms
  (WebKit) to 550 ms (Chromium).
- Rejected: the visible card does not jump during those passes; the new
  version stays hidden until they finish.
- Verified: up to three live pages run at once. Three animating Chromium pages
  sent 180 frames a second and used 26 to 30% CPU in Chromium's processes.
- Verified: Chromium paints on demand (a static page sends no frames), is
  capped at 60 frames a second (`windowless_frame_rate` default), and rendered
  at a fixed 2x whatever the window's scale or the card's size.

Resolution (measured): Chromium at a fixed 2x rendered 0.80 pixels per device
pixel at 1.25 zoom and 1.62 times the pixels needed for a 1280-point page in a
788-point card. WebKit's frames, stills and the agent's pictures were already
at the backing scale, and the outline placeholder is drawn as vectors, not a
1x bitmap. The card origin was not snapped to device pixels. Kept pictures
shared the agent's 1000-point cap (2000 px on Retina).

Design language: the page's `html`/`body` are forced transparent (`page.rs`),
`clearFills` strips a full-width outer fill in `--background`/`--popover`, and
`draft.md` told the agent never to paint a background. The `app` topic of
`visual_docs` was never mentioned in the always-on instructions.

## Plan and status

0. Baseline. The tree had 101 uncommitted files from other work when this
   started. They were left alone: nothing was committed, stashed or reverted,
   so the changes below sit in the same working tree as that work. Still to
   add: a pixel-density check per engine, and frame time and CPU with 0, 1 and
   3 drafts.
0. Baseline. Done: `MONOCODE_DRAFT_SELFTEST_PERF` in `draft/selftest.rs`
   measures frame time, app and engine CPU, Chromium frames a second, how long
   a revision takes to show, and pixel density per engine, with 0, 1 or 3
   drafts, an optional 120 Hz redraw (`_TICK`), page width and zoom.
1. Duplicates.
   - Done: a new `html` draft whose title matches one this turn already
     started revises that draft instead of adding a card
     (`visual_bridge.rs`: `same_turn_draft`, `remember_draft`,
     `forget_draft`; test
     `a_retry_with_the_same_title_revises_the_draft_instead_of_adding_a_twin`).
   - Not done: `draft_keep` upgrading the existing card instead of adding a
     second one; rendering the live page once per id when two rows share it.
   - Done (2026-11-01): `draft_keep` upgrades the existing card. The transcript
     pairs a kept picture with its card through the `draft_keep` call's `draft`
     id (`visual_bridge::kept_upgrades`) and draws the live page, else the kept
     picture, else the closed note, at the card's own place; the picture has no
     row of its own. Tests in `transcript/pages.rs` and `visual_bridge.rs`.
   - Done: a thread draws a draft id once (`TranscriptView::shows_draft`); a
     later row naming the same draft is dropped on rebuild.
   - Done: a same-title retry after the person closed the draft starts a new
     draft instead of failing (`DRAFT_GONE`, test
     `a_retry_after_the_person_closed_the_draft_starts_a_new_one`).
   - Not done: the phone still shows a kept picture as its own card; it cannot
     tell whether the desktop's page is live.
   - Measured: a card follows its list item at a constant offset once a frame
     settles; the first paint after a scroll reports the previous position
     (test `a_live_card_sits_at_its_place_in_the_turn_and_scrolls_with_it`).
2. Performance (not started; needs a GPUI build).
   - Cache everything except the draft row instead of bypassing the cache.
   - Make the layout passes opt-in or run them on an offscreen clone.
   - Keep only the newest or on-screen draft live; freeze older ones to a
     poster that wakes on click.
   - Use the window's real scale factor for CEF.
3. Resolution (not started; needs a GPUI build).
   - Snap card origin and size to whole physical pixels.
   - Take every placeholder, still and kept picture at the backing scale.
   - Raise the picture cap to about 2x the card width; give agent screenshots
     more pixels.
2. Performance (done on `fix/draft-perf-resolution`).
   - Done: the transcript stays cached with a live draft. Each page's card
     paints a presence token (`draft/engine.rs`, `Presence`); when the card
     leaves the frame, the token drops and the page steps aside or hides. The
     WebKit popover check runs from its sentinel for replayed cards
     (`draft/web.rs`). Not done: `shell/project_settings.rs` and
     `conversation/btw.rs` still bypass the cache through `holds_live_draft`.
   - Done: layout passes run only for a draft's first version, and a pass that
     keeps the page's width no longer waits out its 900 ms timeout.
   - Done: only the draft in use runs; the others sleep to a picture of their
     page and wake on click or when the agent uses them (`shell/drafts.rs`,
     `keep_running`; `Draft::sleep`, `Draft::wake`).
   - Done: Chromium renders at the window's scale times the card's fit,
     rounded up to a sixteenth (`chrome.rs`, `render_scale`).
3. Resolution (done).
   - Done: the Chromium card origin snaps to device pixels and its frame is
     drawn at its own pixel size. WebKit places its native view itself.
   - Done: kept pictures are taken up to 1600 points wide (3200 px on
     Retina); the agent's pictures stay at 1000 points.
4. Design language and creative freedom.
   - Done: `draft.md` says a boxed mock may paint `--background`, `--sidebar`
     and the other tokens itself, says to read `visual_docs app` when mocking
     MonoCode or the user's software, and documents the same-title rule.
   - Done: one sentence in `DRAFT_INSTRUCTIONS`. The budget test for the
     instructions with thread tools went from 1500 to 1600 bytes to fit it.
   - Not done: a `surface` option on `draft_preview` (`thread`, `app`, `none`)
     and turning `clearFills` into a warning only. Both touch `page.rs` and
     the tool schema.
5. Verify. Core: `cargo test --lib visual_bridge` passes (40 tests). Still to
   do: GPUI draft self-tests in both engines, the stale test noted in memory,
   and a real-thread run with three variants, a retry, a keep and two panes.

## Blocker

The disk had 4.8 to 5 GiB free after the core test build. A GPUI debug build
needs about 14 GiB (`target/debug`, which was deleted earlier), so steps 2, 3,
the rest of 1 and the `surface` option of 4 wait on freeing space.
5. Verify. Core: `cargo test --lib visual_bridge` passes (40 tests). GPUI:
   the draft, transcript-pages and chrome tests pass (60), and
   `scripts/verify-native-draft.py` passes in both engines. Still to do: a
   real-thread run with three variants, a retry, a keep and two panes.

## Measurements

Self-test harness on a Retina Mac (scale 2), 16 filler rows plus the drafts,
6 s each, base (`snapshot/draft-fixes-base`) and the fix run interleaved,
two runs each. Frame time is from the window's render to the end of its paint.
"Redraw" adds a view that asks for a frame every frame, as a streaming answer
or a spinner would.

| Case | Frame time, ms (base → fix) | App CPU % | Engine CPU % |
| --- | --- | --- | --- |
| 0 drafts | idle | 0.3–2 → 0.3–2 | 0 → 0 |
| WebKit, 1 static, redraw | 0.75–1.07 → 0.07–0.09 | 14–39 → 6–7 | 0–18 → 0 |
| WebKit, 3 static, redraw | 0.60–0.63 → 0.06 | 12.5–13.8 → 5.2–5.8 | 0.3 → 0.2 |
| Chromium, 1 static, redraw | 0.74–0.90 → 0.07 | 4–14 → 5.3–5.8 | 0.3–0.5 → 0.3–0.5 |
| Chromium, 3 static, redraw | 0.56–0.59 → 0.05 | 11.6–12.5 → 5.0–5.1 | 0.8–1.0 → 0.5–0.6 |
| Chromium, 1 animating | 0.85–0.86 → 0.86–0.94 | 9–10.5 → 9.2–9.6 | 10.2–10.7 → 10.0–10.4 |
| Chromium, 3 animating | 0.53–0.55 → 0.54–0.57 | 13.1–13.7 → 6.7–7.2 | 26–30 → 10.4–11.1 |
| WebKit, 3 animating | (native view) | 3.1–4.4 → 1.5 | 9.4–10.2 → 3.2 |

Chromium frames a second with three animating drafts: 180 → 60. Under heavier
background load earlier the same case gave 50–52% → 28–30% (Chromium) and
25% → 9.5% (WebKit). One animating Chromium page costs the same before and
after: each of its frames still renders the draft's view and its ancestors.

Time for a revision to show: WebKit 459–480 → 6–9 ms, Chromium 541–558 →
82–92 ms. A draft's first version still takes about 460 (WebKit) and 550 ms
(Chromium) for its layout passes; a 1280-point page's first version takes
510 ms (550 on base; 1340 before the same-width pass stopped waiting).

Pixels per device pixel, Chromium frame / still: 1.00 / 1.00 at 100%, 0.80 →
1.00 at 1.25 zoom; a 1280-point page in a 788-point card 1.62 → 1.02 times
the pixels needed. WebKit: 1.00 before and after.
