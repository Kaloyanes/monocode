# Ask a PR: plan

You are the reviewer. You open a pull request, select some code or just ask,
and an agent answers with references into the diff. Nothing you ask is visible
to the PR author unless you add it to your review.

This is the main PR feature. `pr-questions-plan.md` covers the opposite
direction (the agent asking the PR author's side); treat it as a later add-on
that can reuse the same sidebar.

The visual mock is the "Reviewing a PR: ask the agent about the diff" draft.

## Flow

1. Open a PR from the Inbox. The diff shows with old and new line numbers.
2. Click a gutter number to select a line, click another to extend to a range.
   A bar appears: "retry.rs:41-42 selected, Ask about these lines".
   Or press the "Ask about this PR" button (⌘J) for the whole PR.
3. A sidebar opens beside the diff with a chat scoped to the selection or the
   whole PR. Suggested questions: summarize, what could break, is it tested.
4. The agent answers with citations (`retry.rs:42`, `jobs/upload.rs:88`). A
   citation highlights its lines in the diff, or expands the code inline when
   it points outside the diff.
5. "Add as review comment" turns an answer into a pending comment anchored to
   the selected line. It shows inline in the diff, marked "only you can see
   it", and counts toward "N pending" in the header. Undo removes it.
6. "Open in conversation" moves the chat into the main conversation as its own
   tab, with the same history and pending comments. "Move to sidebar" reverses
   it.
7. "Submit review" posts the pending comments to GitHub in one review.

## Design rules

- Read-only. The ask agent cannot edit files, push, or post.
- Private by default. Answers stay local until you add them to a review.
- Every claim about code carries a citation, and a citation that does not
  resolve (file or line missing at the PR's head commit) is shown as plain text,
  not as a chip.
- One chat, two places. The sidebar and the conversation tab show the same
  session, not a copy.

## Architecture

- **Ask session:** a scoped, read-only agent session with the PR as context:
  title, body, base and head, the diff, linked issues, and CI status. A
  selection adds the file, range, and surrounding hunk. It is a side session in
  the same sense as a Mono pane (`shell/mono_panes.rs`), so the pane split and
  promote-to-tab path are reused.
- **Reading code outside the diff:** the agent needs files at the PR's head
  commit. Two options: a throwaway worktree at the head SHA (full tooling,
  needs the repo cloned), or `gh api` file reads (works for any PR, slower,
  no search). Start with the worktree when the project is local and fall back
  to `gh api`.
- **Tools:** `pr_diff`, `pr_file(path, ref)`, `pr_threads`, `pr_checks`. These
  belong in the MCP surface, which has uncommitted work from another session in
  `crates/monocode-core/src/mcp.rs` and `mcp/`: do not edit those until it lands.
  Until then the prompt carries the diff and the worktree provides file reads.
- **Citations:** the agent writes `[[path:line]]`. The host parses them, checks
  they exist at the head commit, and renders chips.
- **Pending review:** a local store per PR: `{ id, path, line, side, text,
  source_message }`. Submit uses GraphQL `addPullRequestReview` with
  `threads`. This is a new write path next to the existing
  `addPullRequestReviewThreadReply`.
- **Today:** the Inbox shows the diff and review threads and can reply to a
  thread (`inbox/github.rs`, `inbox/ui.rs`). `start()` builds a flat text dump
  of the PR and starts a normal session. Nothing selects lines, scopes a
  question, or stores pending comments.

## Plan

Each step ships on its own.

1. **Line selection in the Inbox diff.** Gutter click and range, selection bar.
   Test: selection state and the label for add, delete and context lines.
2. **Ask session.** Replace the `start()` text dump with a structured PR
   context, read-only tools, and the scope (whole PR or range). Test with a
   fixture PR and a stub model.
3. **Sidebar chat.** Pane beside the diff, suggested questions, scope chip,
   composer. ⌘J toggles it.
4. **Citations.** Parse, validate against the head commit, render chips,
   highlight in the diff.
5. **Pending review.** Local store, inline comment rendering, header count,
   undo.
6. **Submit review.** `addPullRequestReview` with threads. Comment only at
   first. Test the GraphQL payload with the existing fixture style.
7. **Open in conversation.** Promote the sidebar session to a tab and back.
8. **Head-commit file access.** Worktree at the PR head, `gh api` fallback.
9. **Phone.** Read the chat and add comments; selecting lines stays desktop.
10. **Screenshots and tests.** Home-shots fixtures for the four states.

## Decisions I need from you

- Is the agent allowed to read the whole repository at the PR's head commit
  (worktree), or only the diff and files fetched through GitHub?
- Should "Submit review" also offer Approve and Request changes, or stay
  comment-only for now?
- Do answers persist with the PR across restarts (my default: yes, per PR),
  or reset each time you open it?

## Not in scope

Merging, approving without your click, posting anything automatically,
non-GitHub providers (the core types are provider-neutral, so GitLab and Azure
DevOps can follow).
