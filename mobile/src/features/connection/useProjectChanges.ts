import { useCallback, useMemo } from "react";
import type { ChangeSummary } from "@/features/workspace/types";
import { useCommand } from "./ConnectionProvider";
import { mergeChanges, type Changes } from "./changed";
import { polledResource } from "./polledResource";
import { usePolled } from "./usePolled";

/** A desktop that hashes its replies says only this when the phone has the latest. */
type Reply = Changes | { unchanged: true; hash: string; worktreePath?: string };

const resourceFor = (projectId: string, worktreePath: string | undefined, limit?: number) =>
  polledResource<Changes>(
    `changes\n${projectId}\n${worktreePath ?? ""}\n${limit === 0 ? "summary" : "list"}`,
    { merge: mergeChanges },
  );

/** What a reply counts, or what its files add up to from an older desktop. */
export function summarize(changes: Changes | null): ChangeSummary {
  if (!changes) return { files: 0, staged: 0, additions: 0, deletions: 0 };
  if (changes.total !== undefined)
    return {
      files: changes.total,
      staged: changes.stagedTotal ?? 0,
      additions: changes.additions ?? 0,
      deletions: changes.deletions ?? 0,
    };
  const { files } = changes;
  return {
    files: files.length,
    staged: files.filter((file) => file.staged && !file.unstaged).length,
    additions: files.reduce((sum, file) => sum + file.additions, 0),
    deletions: files.reduce((sum, file) => sum + file.deletions, 0),
  };
}

/**
 * The project checkout's changes, or a worktree's when given its path. With
 * `limit`, only that many files are read, staged first, though the summary
 * counts them all; at 0, only the summary, for screens that just count them.
 * Every screen reading the same working copy shares one poller, which runs
 * only while one is up and focused and the app is in the foreground.
 */
export function useProjectChanges(
  projectId: string,
  enabled: boolean,
  worktreePath?: string,
  limit?: number,
) {
  const command = useCommand();
  const resource = useMemo(
    () => resourceFor(projectId, worktreePath, limit),
    [projectId, worktreePath, limit],
  );
  const fetch = useCallback(async () => {
    const before = resource.getSnapshot().value;
    // With the hash of what is held, the desktop answers in a few bytes when
    // nothing changed, rather than resending every file.
    const reply = await command<Reply>("project.changes", {
      projectId,
      ...(worktreePath ? { worktreePath } : {}),
      ...(before?.hash ? { since: before.hash } : {}),
      ...(limit !== undefined ? { limit } : {}),
    });
    // A desktop that predates worktrees answers with the checkout's
    // changes; say so rather than show them as the worktree's.
    if (reply.worktreePath !== worktreePath)
      throw new Error("Update or reload Monocode on your desktop to see this worktree's changes.");
    if ("unchanged" in reply && before) return before;
    return reply as Changes;
  }, [command, projectId, worktreePath, limit, resource]);
  const { value, error } = usePolled(resource, fetch, enabled);
  const summary = useMemo(() => summarize(value), [value]);
  return { result: value, error, reload: resource.reload, summary };
}
