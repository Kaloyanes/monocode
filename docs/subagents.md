# Subagents

Any chat you start yourself can start subagents on any harness and model, such as a Codex review or a Haiku search. Each one is its own chat, shown under the message that started it. Claude, Codex and Pi are told in their system prompt to use MonoCode for subagents instead of their own subagent tool. Other harnesses get the same advice from MonoCode's tools.

A lead doesn't have to stay running while its subagents work. It can end its turn, and when they have all finished MonoCode sends it their results in one message, so it can check the work and tell you the outcome. The message waits until the lead is idle and its composer is empty. A subagent that is stopped, closed or already read by the lead isn't included, and stopping the lead cancels the message. Results not yet sent when the app quits are not sent after a restart; the lead can still read them with `thread_list`.

When a lead needs a result sooner, `thread_wait` waits for one subagent, or with `threadIds` for the first of several to finish.

A lead can give a subagent less access than its own, such as read-only for a reviewer, but never more. A subagent started with `workspaceMode: worktree` reports its folder and branch. Merging that work back is still up to the lead.

`thread_wait` and `thread_list` also show how long a subagent has been working, its step count, what it is doing now, the files its edit tools changed, and any of those files another subagent in the same checkout also changed. Files changed through shell commands aren't included.

A finished subagent can be closed: the lead calls `thread_close`, or you press **Remove** on its row or **Clear finished** above the list. Closing archives the subagent's chat and takes it off the lead's list and the sidebar. A working subagent has to be stopped first. To bring one back, unarchive it from **Settings > Archive**.

By default, Claude Code's `Agent` tool and Codex's multi-agent feature are turned off in these chats, so every subagent is a MonoCode chat. To let them use their own subagent tools again, such as Claude's quick Explore helper, turn off **Settings > Providers > Advanced > Subagents only through MonoCode**. The change applies from the next turn.

Return to the [README](../README.md) for a quick start.
