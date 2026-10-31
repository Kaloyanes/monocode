# Drafts on the phone: feature and plan

A draft is a live page the agent builds in a thread. The thread is the same on
the desktop and on a paired phone, so the draft is too: the person can open it
on either screen, move its controls, pin notes and hand it back, and both
screens and the agent see the same thing. How it works today is in
[drafts.md](drafts.md#on-the-phone); this file says what the feature is, what
is built, and what is left.

## What the person sees

**In the transcript.** Each draft, and each picture kept from one, is a card in
the conversation: a pencil icon, the draft's title (up to two lines), a hint
line, and a chevron. The hint is **Draft · Open** for a live draft and **Kept
draft · Open** for a kept one. On iOS the card is drawn by the native
transcript (`TranscriptPresentation.swift`); on Android by `TranscriptTurn.tsx`.
Both look like the error card. Older phone builds skip the card.

**The draft sheet.** Tapping the card opens a full-screen modal over the chat,
titled with the draft's name, with a close button on the left. Under the header
is one bar, the same controls the desktop card has:

| State | Status line | Buttons |
| --- | --- | --- |
| Nobody has touched it | (empty) | Comment |
| The person moved something | You're using the draft | Comment, Hand back |
| Comment mode | Tap the page to leave a note | Comment (selected) |
| Notes pinned, not sent | (as above) | Comment, Send N notes, Hand back |
| The agent called `draft_wait` | ● Waiting for you | Comment, Hand back |

Below the bar is the page: the phone's own copy, laid out at the phone's width
and in the phone's light or dark theme.

**Working together.** Moving a control on the phone moves it in the desktop
card a moment later, and the other way round. A note pinned on either screen
shows on both with the same number, and **Send N notes** on either screen sends
all of them. **Hand back** on either screen gives the draft back to the agent.

**Pins land on their element.** A pin remembers the element it was placed on,
so a note made on the desktop's wide layout sits on the same button in the
phone's narrow one, and pins move with their element when the page lays out
again.

**Dev servers.** A localhost `url` draft cannot load on the phone, which has no
way to reach the desktop's localhost. The phone shows pictures of the
desktop's copy instead, about once a second, under "Live from your desktop ·
localhost:5173". Nothing on it can be touched; the person uses it on the
desktop.

**What does not travel.** Scroll position, focus, and anything a page keeps
outside `mc.state` (a variable, the DOM) stay on the screen where they
changed.

## How it is built

- **One record per draft**, owned by the desktop: `SharedDraft` in
  `crates/monocode-core/src/visual_bridge/draft/shared.rs`. It holds the page,
  `mc.state`, the pins and pending notes, who is driving, whether the agent is
  waiting, and revision counters. The desktop card (`Draft` in
  `crates/monocode-gpui/src/draft.rs`) draws that record and keeps its page in
  step with it (`reconcile`).
- **Two phone methods**, in `crates/monocode-gpui/src/shell/mobile_host/drafts.rs`
  and allowed in `APP_METHODS`:
  - `draft.read` waits up to 15 seconds for the record to change (`since`
    revision) and leaves out the page HTML when the phone already has that
    generation.
  - `draft.action` takes `pin`, `send`, `input`, `state`, `message` and
    `handBack`, with command ids so a retry does not run twice.
  - `draft.frame` answers a dev-server draft with a JPEG of the desktop's copy
    (at most 1290 pixels wide), leasing the page so it runs off screen.
- **The phone**, in `mobile/src/features/drafts/`: `DraftScreen.tsx` runs the
  read loop and the web view (`react-native-webview`), `DraftFrame.tsx` shows a
  dev server's pictures (two image slots that take turns, so it never blinks),
  and `sync.ts` holds the rules for merging, so they can be tested without a
  device.
- **Kept drafts** go by their file name, `kept:<file>.html`, inside the
  thread's images folder; no desktop paths reach the phone. Opening one makes it
  live on both screens.
- **The agent** sees the desktop's copy. Each agent request leases that page,
  so it runs even when its card is not on screen; the desktop app must be
  running.

### Sync rules

1. **Starting values.** When a page loads, its first report is its own starting
   values. The record keeps what was chosen elsewhere and sends it to the page.
2. **Grace.** A page given new values may report its old ones for up to a
   second before it catches up; those reports are not the person's changes.
3. **Own change in flight.** While the phone's own change is on the way, it
   does not apply an older answer from the desktop, so a slider does not jump
   back under the person's thumb.
4. **Older never wins.** An answer with an older page generation or revision
   never replaces a newer one.
5. **Pins** carry the side they were made on. A page does not redraw its own
   pins, except after a reload, when it has none. Each pin also carries an
   anchor: the element's child indexes from `<body>` and the point inside its
   box. Every screen loads the same page, so the path names the same element
   on each; a pin whose element is missing falls back to its percentages. The
   anchor stays between the screens: the agent's notes carry the percentages
   only.
6. **Trust.** The phone passes on what the page reports only within 2 seconds
   of a real touch, and a note only while comment mode is on (and within 2
   minutes of the tap that opened it). The desktop has the same rule for real
   mouse and key input. A page's script cannot speak for the person.

## Done

- The shared record, the desktop card on top of it, `draft.read`,
  `draft.action` and `draft.frame`, the transcript cards on both platforms, the
  draft sheet.
- Pins anchored to their element. Checked in headless Chrome with the probe's
  own code: a pin made at a wide layout follows its element when the page
  reflows to a phone's width, where its percentage alone lands about 680
  pixels away.
- Dev-server drafts on the phone, as pictures.
- Drafts survive a restart (saved under `threads/<session>/drafts/`).
- The agent can look at a draft whose conversation is not open on the desktop.
- `package-lock.json` differs from before only by the web view's entry.
- `scripts/verify-native-draft.py` passes in full, the resize check included.
- Tests: the record and anchors (core), the desktop sync, the phone handlers
  end to end (including a dev-server draft), the transcript items, the
  mobile-link row, the phone's sync rules, and the draft sheet and the picture
  view as components. An iOS simulator build succeeds.

## Left to do

1. **Try it on a real phone.** `react-native-webview` is a new native
   dependency, so this needs a new app build and cannot ship over the air. Check
   the sheet's layout, the keyboard over a note, the comment-mode tap and a
   dev server's pictures on iOS and Android.

Not planned: running drafts without the desktop app. The agents run in the
desktop app, and the phone reaches everything through it, so with the app
closed there is no agent to look at a draft and nothing for the phone to read.

## Related: composer drafts

Unsent text in a thread's composer follows the thread to the phone the same
way: type on the desktop, open the thread on the phone, and the text is in the
phone's composer, and the other way round. The text travels; files, the mode
chip and Mono chats do not. How it works is in
[composer-drafts-sync.md](composer-drafts-sync.md).
