import type { SessionSummary, Worktree } from "@/features/workspace/types";

/**
 * Whether a session works in `worktree`; any does without one. An older
 * desktop doesn't say where a session works, so it goes by the branch, which
 * only one working copy can have checked out.
 */
export function inWorktree(session: SessionSummary, worktree: Worktree | undefined) {
  if (!worktree) return true;
  if (session.worktreePath) return samePath(session.worktreePath, worktree.path);
  return session.branch === worktree.name;
}

function samePath(a: string, b: string) {
  return a.replace(/\/+$/, "") === b.replace(/\/+$/, "");
}
