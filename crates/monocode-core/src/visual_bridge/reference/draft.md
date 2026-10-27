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
5. When the result is worth keeping, draft_keep {draft, title}. Drafts do not
   survive a restart; a kept one leaves a picture card with an Open draft
   button that brings the page back live. {draft, action:{kind:"close"}}
   removes the page and leaves a note.

The card is as wide as the conversation column (up to 832 points) and as tall
as the page's content, unless you pass height. width sets the page's layout
width (240 to 1600); wider than the column, it is scaled down to fit, so a
desktop mock stays a desktop mock. Up to three drafts stay live per
conversation; opening a fourth retires the oldest.

A draft is only drawn while its card is on screen, in the conversation the user
has open. A conversation in the background gets an error, and so does a card
scrolled out of view; try again when the user is back. Several screenshots in a
row are fine: they never appear in the thread, only the card does.

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

If the page uses mc.state and mc.controls, every result also carries state:
the current values of its controls, so you can see what the person chose.

Every result also carries issues: things the page itself reveals, such as low
contrast, small tap targets, controls without a name, images without alt text
and clipped text. Fix them or know why they are fine.

## Human takeover

The user can click and type in the draft. When they do, userControl is true:
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

The app supplies CSS variables for both themes: --background, --foreground,
--muted, --muted-foreground, --card, --popover, --secondary, --border, --input,
--ring, --primary, --accent, --accent-surface, --destructive, --warning,
--success, --info, --code-background, --chart-1 through --chart-6, --radius,
--font-sans and --font-mono. A depicted product screen may use its own.

Avoid 100vh and height:100% on html/body; the card is as tall as the content. Use
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
