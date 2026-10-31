# Drafts

A draft is a live page you build inside the conversation. It appears as a card
at this point in the thread, between your messages, and the user watches it
take shape and can click around in it right there. Use it to design UI, mock a
screen, try an interaction, build a small tool or explainer with full HTML,
CSS and JavaScript, or check a running dev server. There is no side panel.

## The loop

1. draft_preview {html, title} (or {url}) puts the card in the thread and
   returns a screenshot, the page text, controls with refs, console output and
   diagnostics.
2. Look at the screenshot. Fix console errors, clipping, overflow and anything
   that reads badly. draft_preview {draft, html} replaces the page in the same
   card.
3. Test it like a person would: {draft, action:{kind:"click", reference:"REF"}},
   then {draft, action:{kind:"type", text:"hello", replace:true}} for the focused
   input. Refs expire on the next snapshot, so inspect again before reusing one.
4. {draft, width:375, appearance:"dark"} checks another viewport and theme.
5. The app saves each draft as it changes, so it comes back live after a
   restart under the same id. draft_keep {draft, title} also leaves a picture
   card with an Open draft button, for a result worth pointing back to.
   {draft, action:{kind:"close"}} removes the page for good and leaves a note,
   so close only what the person no longer needs.

The card is as wide as the conversation column (up to 832 points) and as tall
as the page's content, however tall that is, and it follows the content as it
grows or shrinks; the card never scrolls the page inside itself (past 12000
points it does). Passing height only sets where the card starts. While a new version loads, the
card shows the previous version (or the page's outline) and the page fades in
once it is ready, already at its height, so build pages that are complete on
load rather than assembling themselves in view. width sets the page's layout
width (240 to 1600); wider than the column, it is scaled down to fit, so a
desktop mock stays a desktop mock. Up to three drafts stay live per
conversation; opening a fourth retires the oldest.

The page runs for each of your requests whether or not the person can see its
card: in a conversation in the background, scrolled out of view, or before the
card was ever on screen. Start and drive a draft whenever it helps; the card
shows the latest version when the person gets to it. draft_wait is the
exception: it waits for the person, who has to see the card. A new draft's
card appears once its first call succeeds; a call that fails leaves nothing
behind, so just try again. Several screenshots in a row are fine: they never
appear in the thread, only the card does.

## Other widths and the full view

Every new version you send is also laid out, out of sight, at a phone, tablet
and laptop width (375, 768 and 1280). The result carries layouts: the height
each needs and how far it overflows to the right. A horizontalOverflow above 0
at 375 means the page breaks on a phone; fix it unless the page is a fixed
desktop mock.

The person can open the draft in a full view over the window. A page you gave
a width keeps it there, up to life size; any other page reflows to fill the
view, so fluid layouts get the room. While it is open, your results describe
the full view's viewport; pass width or height to bring it back to the card.

## Feedback from the person

The card has a Comment button. In comment mode the person clicks anywhere on the
page and writes a note; it is pinned there with a number. Send N notes hands
them to you:

- If you called draft_wait, it returns with event "feedback" and the notes. Use
  it after you ask the person to try the page: say what to look at, then wait.
  A wait lasts up to 30 seconds (event "timeout" if nothing came); call it
  again to keep waiting. "handed back" means they pressed Hand back with no
  notes.
- If you were busy, the notes arrive as a message in the conversation.

Each note has the element it sits on (tag, label or text) and its place as a
percentage of the page, and the pins are numbered in the screenshots, so look
at the page again before you change it. userActions lists what the person did
by hand (clicks on named controls) since you last looked. Answer notes by
revising the same draft, then say what you changed.

If the page uses mc.state, every result also carries state: the current values
of its controls, so you can see what the person chose. Lines the page sent with
mc.send arrive in messages.

Every result also carries issues: things the page itself reveals, such as low
contrast, small tap targets, controls without a name, images without alt text
and clipped text. Fix them or know why they are fine.

## On the phone

The person may open the draft on their paired phone. The phone draws its own
copy of an HTML page, at a phone's width, and shares three things with the card
and with you: mc.state, the notes (with their pins), and who is driving. A note
or a tap there reaches you the same way as on the desktop. Nothing else is
shared: a value the page keeps only in a variable or in the DOM stays on the
screen where it changed, and your clicks and typing act on the desktop's copy
only. So put anything the person might change, and anything you want to read
back, in mc.state. Make the page work at 375 points wide too. A localhost url
cannot load on the phone: it shows pictures of the desktop's copy, which the
person cannot touch there.

## Human takeover

The user can click and type in the draft, on the desktop or on the phone. When they do, userControl is true:
snapshots still work, but click, type, replace and resize are refused so you do
not fight them. Use {kind:"resume"} once they say you may continue.

Your clicks and typing are sent as script events, not real input. They never take
focus from the user's composer. Pages that depend on trusted events (file
pickers, clipboard, fullscreen) cannot be driven this way.

## Writing the page

Pass {html} with inline <style> and <script>. Plain HTML, CSS, SVG and JavaScript
work; the optional mc kit (visual_docs {topic:"kit"}) adds components and charts.
For UI mockups use the product's real content, visual identity and working
controls. Keep explanation in the conversation, not in the page.

## Design for the person

The page is yours, and so are its controls. Start from what the person has to
judge or decide, then invent the controls that let them do it: which states the
subject moves between and what the trigger is called ("Collapse", "Send
message", "Lose connection"), whether motion needs slow motion, a scrubber,
step-by-step frames or a replay, which values are worth a slider, what a
comparison needs to sit side by side. Name everything in the subject's own words.
A draft sits in the thread with no frame of its own, so any control it needs is
part of the page. Some shapes that often fit:
- Compare versions of one component: side by side in one row, driven by one
  shared trigger, each titled in a line. Tabs would hide the difference.
- Review a page or a flow: one page per tab or step.
- Judge motion: a trigger that flips between the states, plus whatever timing
  control the motion needs. Without motion, none of these.
- Tune values: controls bound to the thing they change, close to it.
- Approve a look: a single static page with no controls.
These are defaults, not rules: build what the intent calls for, and leave out
what it does not. The kit (visual_docs {topic:"kit"}) supplies the parts that
are hard to make yourself, such as a clock that slows every animation on the
page and state the agent can read and set; its ready-made bars are examples.

Make the page look like part of the conversation:
- Leave html, body and the outermost element without a background; the page
  sits on the thread's own background. The app clears the page's own html and
  body background, and removes a full-width outermost fill in --background or
  --popover and reports it as an issue.
- No outer card, border or banner title around everything: the card already
  carries the title, and the page is part of your reply.
- Fluid width with no side padding on the outermost element.
- A box that needs its own background (a mock of a screen, a control panel)
  gets at least 16px of padding and var(--radius) corners.
After showing a draft, don't describe it again in your reply: say only what the
page doesn't.

## The page talking back

Two ways for the page and you to work together, both optional:
- mc.state({key: value}) holds what the page's controls change. You read it as
  `state` in every result, and you can change it yourself with
  {draft, action:{kind:"set", values:{key: value}}}, which updates the page the
  way its own controls would. Put what you will want to drive by name there:
  the current state, speed, the chosen variant.
- mc.send(text) sends a line to you as from the person, for a control you offer
  such as "Use this one" or "Looks right". It works only straight after the
  person's own click or key press. It arrives in `messages` (a wait returns
  with event "message"), or as a message in the conversation when you are busy.

The app supplies CSS variables for both themes: --background, --foreground,
--muted, --muted-foreground, --card, --popover, --secondary, --border, --input,
--ring, --primary, --accent, --accent-surface, --destructive, --warning,
--success, --info, --code-background, --chart-1 through --chart-6, --radius,
--font-sans and --font-mono. A depicted product screen may use its own.

Avoid 100vh, min-height:100vh and height:100% on html/body; the card is as tall as
the content, so a page sized by its own viewport never settles, and the card
stops following it and scrolls. Use
fluid layouts and SVG viewBox. Respect prefers-reduced-motion.

## Limits

The page has no filesystem, storage or app bridge. fetch, XHR and WebSocket are
blocked. Scripts and styles load over HTTPS only from cdn.jsdelivr.net,
cdnjs.cloudflare.com, unpkg.com, esm.sh, cdn.skypack.dev, ajax.googleapis.com
and cdn.tailwindcss.com; fonts also from fonts.googleapis.com, fonts.gstatic.com,
fonts.bunny.net and fontshare. Pin versions. Anything else is plain HTML, CSS,
SVG, canvas, WebGL, Web Audio and JavaScript: the page is yours. Links open in the user's browser.

Images: use an absolute path, ~/path or https URL in img src, CSS url() or a
JavaScript string. They are embedded before the page opens (PNG, JPEG, GIF, WebP,
AVIF, BMP, ICO, SVG; at most 24 images and 6 MiB). HTML is limited to 512 KiB
without images and 10 MiB with them.

## Running apps

draft_preview {url:"http://localhost:5173"} opens a dev server on this machine in
the same card. localhost, 127.0.0.1 and ::1 only, no credentials, an ephemeral
profile. The page keeps its own styles, network and hot reload. The response
includes the current url and title. Pass {draft, url} to navigate and take a new
snapshot after hot reload or user input. Running apps cannot be kept; take
pictures with {draft} instead.

This is WebKit, so Chromium-specific behavior still needs a Chromium check.
Popup windows, downloads, cross-origin iframe controls and recording are not
supported.

## Diagnostics

ready: loaded and fonts settled. horizontalOverflow and verticalOverflow: how far
the content exceeds the viewport. failedImages: images that did not load. An
incomplete or overflowing render needs another look even when loaded is true.
