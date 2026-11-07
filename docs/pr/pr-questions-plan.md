# PR questions: audit and plan

Goal: when an agent works on a pull request, every open question about that PR
(a reviewer's thread, a CI failure it cannot judge, a decision only the user
can make) lands in one place where it can be answered in a few clicks. The
answers drive the agent, and replies go back to GitHub.

The visual mock is the "PR questions dock" draft in the conversation that
produced this file.

## Audit: what exists today

| Area | Where | State |
| --- | --- | --- |
| Open a PR from a session | `monocode-gpui/src/conversation/open_pr.rs`, `monocode-core/src/scm/open_pr.rs` | `/pr` pushes and creates the PR, links it to the session. Done. |
| PR in the Inbox | `monocode-core/src/inbox.rs`, `inbox/github.rs`, `monocode-gpui/src/inbox/ui.rs` | Lists PRs, shows details, review decision, commits, checks, diff, review threads, and a reply box. |
| Review threads | `inbox/github.rs` (`reviewThreads(last: 20)`, comments `first: 8`) | Read and reply (`addPullRequestReviewThreadReply`). No resolve. Hard caps with no paging. |
| Reply path | `Service::comment(item, body, reply_to)` | Works for a thread id. Posts immediately, no draft, no batching. |
| Start work from a PR | `inbox.rs` `start()`, `Item::prompt(body, ask)` | `start()` always passes `ask = false`. The "discuss" prompt exists but nothing calls it. The prompt is a flat text dump of the body and every comment. |
| Session to PR link | `session_groups.rs` (`resolve_work_item`, `parse_github_work_item`) | Sessions resolve to a PR from the message, a URL, or the branch. |
| Agent asks the user | `monocode-core/src/agent.rs` (`Question`, `QuestionItem`, `QuestionOption`), `monocode-gpui/src/questions.rs`, `conversation/mobile.rs` | Provider-neutral card, one question per page, replaces the composer. Mapped to the phone. Has no notion of a source, a code excerpt, or a side effect. |
| Agent reads/writes PR data | `monocode-core/src/mcp.rs` | Nothing PR-specific. (File has uncommitted work from another session: do not edit it here.) |

## Gaps

1. **No single queue.** Reviewer threads live in the Inbox, agent questions in a
   modal card, CI state in a third place. Nobody can see "what is waiting on me
   for this PR".
2. **The agent cannot see threads as structure.** It gets a text dump, so it
   cannot tell resolved from open, outdated from current, or reply to a thread.
3. **Questions carry no context.** `Question` has a prompt and options only. A
   review question needs the code excerpt, the author, a link, and an agent
   recommendation.
4. **Answers have no effect.** Picking an option does not draft a reply, push,
   or resolve a thread.
5. **Posting is immediate and irreversible.** No draft, no edit, no "after the
   push succeeds".
6. **Caps hide questions.** 20 threads, 8 comments per thread, no pagination, and
   `isOutdated` is not read.
7. **No resolve.** `resolveReviewThread` is not implemented.
8. **Dead path.** `Item::prompt(.., ask = true)` has no caller.

## Design

One **PR questions dock** per session that has a linked PR. It sits where the
composer is, like the existing question card, and has three states:

- **Collapsed strip:** "4 questions about this PR", with Answer (⌘J).
- **Answering:** a pill per question (source and state), the context (code
  excerpt, quote, agent's read), radio options with one marked Recommended, an
  editable reply draft, and a Resolve checkbox.
- **Review and send:** one line per answer, what will happen (one commit, N
  replies, M resolved), and Send. Nothing posts to GitHub before the push
  succeeds.

### Revision: a PR sidebar with its own agent chat

The dock above stays as the quick-answer surface, but the main way in is a
**PR sidebar**, a pane beside the conversation:

- An "Ask the agent" button (⌘J) on the PR card, and an "Ask" button on each
  open item (reviewer thread, agent decision, failing check). Either opens the
  sidebar scoped to that item. Chips at the top switch the scope.
- The sidebar is a chat with the agent about that item: the agent reads the
  diff, thread and CI, answers, offers options (one Recommended), edits the
  branch if asked, and drafts the reply. The user edits the draft and queues it.
  Queued replies show in a footer ("1 reply queued, posts after push") with
  Review and send.
- **Open in conversation** moves the sidebar chat into the main conversation as
  its own tab ("#482 Q&A") with the same context and queue. **Move to sidebar**
  reverses it. It is one chat session shown in two places, not a copy.
- The sidebar is a scoped side session (like a Mono pane, see
  `shell/mono_panes.rs`), so it reuses the pane split and the promote-to-tab
  path rather than a new window type.

Two sources feed the same queue:

- **Reviewer** items come from open, non-outdated review threads and failing
  checks.
- **Agent** items come from the agent when it hits a decision (the existing
  `Question` path, extended).

Unanswered items stay open and the agent says so in its summary.

### Data model (core)

```
PrQuestion {
  id, session_id, pr: WorkItem,
  source: Reviewer { thread_id, author, url } | Agent | Check { name, url },
  location: Option<{ path, line, excerpt: Vec<Line> }>,
  text, agent_read,
  options: Vec<PrOption { id, label, description, recommended,
                          effect: Effect }>,
  state: Open | Answered(option_id, reply_draft, resolve) | Skipped,
}
Effect { code_change: bool, reply: Option<String>, resolve_thread: bool, rerun_check: bool }
```

`Effect` is what the Review step prints and what the agent executes. Replies
are stored as drafts on the session and posted by the host after a successful
push, never by the model directly.

## Plan

Each step is shippable on its own and verifiable.

1. **Read model.** Extend the GitHub thread query: `isOutdated`, `originalLine`,
   diff hunk for the excerpt, paging past 20 threads and 8 comments. Return
   `PrThread { id, resolved, outdated, path, line, excerpt, comments }`.
   Test: parse fixtures with outdated, resolved, and truncated threads.
2. **Write model.** Add `resolveReviewThread`. Keep `Service::comment` for
   replies. Test with the existing GitHub fixture style in `inbox/github.rs`.
3. **Agent tools.** Add `pr_threads`, `pr_ask`, and `pr_reply_draft` to the
   MCP surface. `pr_ask` creates `PrQuestion`s. Wait for the in-flight
   `mcp.rs` and `mcp/` work to land first to avoid a conflict.
4. **Queue state.** `PrQuestion` store per session, seeded from step 1 and from
   `pr_ask`. Persist answers and reply drafts so a restart keeps them. Test the
   state machine (open, answered, skipped, undo send).
5. **Dock UI (GPUI).** New `pr_questions.rs` beside `questions.rs`, reusing its
   page slide and focus handling. Strip, answering, and review states from the
   mock. Keyboard: ⌘J open, ⌘↵ next or send, 1 to 9 pick an option.
6. **Send pipeline.** On Send: hand answers to the agent as one structured
   message, push, then post drafts and resolve threads from the host. Show the
   result in the strip ("3 replies posted, 2 resolved") with Undo available
   until the push finishes.
7. **Entry points.** Inbox PR detail gets "Ask the agent" (wires the unused
   `ask = true` prompt) and "Address review". Replace `start()`'s text dump
   with the thread list from step 1.
8. **Phone.** Reuse the `question_prompt` mapping: add source and excerpt
   fields. The phone can answer but not approve a push (same rule as
   approvals).
9. **Screenshots and docs.** Home-shots style fixtures for the three states,
   plus a test that the dock never covers the composer's send path.

## Decisions I need from you

- Should replies to reviewers post **automatically after the push**, or always
  wait for an explicit "Post replies" click?
- Is the Inbox the only entry, or should a session with a linked PR also show
  the dock on its own as soon as a new review thread arrives?
- Reviewer threads only for now, or CI failures in the same queue from day one?

## Not in scope

Merging, approving a PR, requesting reviewers, and non-GitHub providers (the
core types are provider-neutral, so GitLab and Azure DevOps can follow).
