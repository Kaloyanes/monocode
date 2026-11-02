# Orchestrator

Orchestrator splits a request into assignments that run as separate agent chats in the current checkout, and reports the combined result back to the chat that planned them (the lead).

## Planning

Choose **Orchestrator** in the composer, or start a message with `/orchestrator`. The planning turn is read-only. The agent submits its plan with the `orchestration_plan` tool on the MonoCode MCP server, and an invalid plan comes back to the agent to fix within the same turn. Providers without the MCP server can end their reply with a tagged JSON plan instead.

The plan appears as a card in the thread. Each assignment shows its title, model, file scopes and the assignments it waits for; open a row to read the full instructions. You can remove assignments before starting. While a plan is under review, the composer stays in Orchestrator mode, so your next message revises the plan or answers the agent's question.

An assignment can name another harness or model when you ask for one. Otherwise workers use the lead's provider, model, account and access mode.

## Running

**Start agents** creates up to four workers at once. Assignments whose file scopes overlap run one after another, and an assignment starts only after the ones it depends on finish. Each worker receives its own instructions plus the reports of the assignments it depends on.

The card follows the run: Queued, Working, Needs input, Done, Failed, Stopped, Blocked or Interrupted, with the worker's model and step count, why a queued assignment is waiting, and any files a worker edited outside its scopes. Open a worker with its link. Approvals and questions are answered in the worker's chat or from the notification.

- **Stop agents** stops queued and running assignments. **Resume** continues them, and **Send report** hands back what finished.
- **Retry** gives a failed, stopped or interrupted assignment another turn in its own chat. Assignments blocked by it start again once it succeeds.
- A worker that never starts its turn fails after a short wait with the reason, instead of staying Working.

When every assignment has settled, the report goes to the lead as soon as the lead is idle and its composer is empty, and the lead checks the combined result. If it cannot be sent, **Send report** stays on the card.

## Restarts

Runs are saved as they change. After a restart, an unfinished run comes back paused: workers that were running show as Interrupted, and **Resume** continues them from where they stopped.

Return to the [README](../README.md) for a quick start.
