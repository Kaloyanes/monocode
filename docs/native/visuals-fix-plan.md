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
1. Duplicates.
   - Done: a new `html` draft whose title matches one this turn already
     started revises that draft instead of adding a card
     (`visual_bridge.rs`: `same_turn_draft`, `remember_draft`,
     `forget_draft`; test
     `a_retry_with_the_same_title_revises_the_draft_instead_of_adding_a_twin`).
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
