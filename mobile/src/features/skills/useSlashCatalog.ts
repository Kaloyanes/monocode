import { useCallback, useEffect, useRef, useState } from "react";

import { useConnection } from "@/features/connection/ConnectionProvider";

import type { ProviderCommand, Skill } from "./slash";

/** Where skills are looked up, and the chat whose provider commands come too. */
export type SlashScope = {
  projectId: string;
  worktreePath?: string;
  conversationId?: string;
};

/** Skills change rarely; within this they're reused rather than asked for again. */
const FRESH_MS = 30_000;
/** Pi and OMP list their commands on request; the answer is asked for again after this. */
const COMMANDS_RETRY_MS = 1_500;

type Cached = { at: number; skills: Skill[] };
// Shared by the composer, the skills sheet and settings, for the app's life.
const skillCache = new Map<string, Cached>();

/**
 * The skills of a working copy, and a chat's provider commands, asked of the
 * desktop while `active` and kept briefly so typing `/` again is instant. A
 * chat not yet open on the desktop has no commands until it runs.
 */
export function useSlashCatalog(scope: SlashScope | undefined, active: boolean) {
  const { command } = useConnection();
  const key = scope ? `${scope.projectId}\n${scope.worktreePath ?? ""}` : "";
  const projectId = scope?.projectId;
  const worktreePath = scope?.worktreePath;
  const conversationId = scope?.conversationId;
  const [skills, setSkills] = useState<{ key: string; list: Skill[] } | null>(null);
  const [commands, setCommands] = useState<{ id: string; list: ProviderCommand[] } | null>(null);
  const [error, setError] = useState<{ key: string; message: string } | null>(null);
  const [revision, setRevision] = useState(0);
  const forced = useRef(false);

  useEffect(() => {
    if (!active || !projectId) return;
    const cached = skillCache.get(key);
    if (cached && !forced.current && Date.now() - cached.at < FRESH_MS) return;
    forced.current = false;
    let disposed = false;
    command<{ skills: Skill[] }>("project.skills", {
      projectId,
      ...(worktreePath ? { worktreePath } : {}),
    }).then(
      (value) => {
        skillCache.set(key, { at: Date.now(), skills: value.skills });
        if (disposed) return;
        setSkills({ key, list: value.skills });
        setError(null);
      },
      (reason) => {
        if (!disposed)
          setError({ key, message: reason instanceof Error ? reason.message : String(reason) });
      },
    );
    return () => {
      disposed = true;
    };
  }, [active, command, key, projectId, worktreePath, revision]);

  useEffect(() => {
    if (!active || !conversationId) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ask = (again: boolean) =>
      command<{ commands: ProviderCommand[]; loading?: boolean }>("session.commands", {
        conversationId,
      }).then(
        (value) => {
          if (disposed) return;
          setCommands({ id: conversationId, list: value.commands });
          if (value.loading && again) timer = setTimeout(() => void ask(false), COMMANDS_RETRY_MS);
        },
        // An older desktop doesn't know the method; skills still show.
        () => {},
      );
    void ask(true);
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [active, command, conversationId, revision]);

  const refresh = useCallback(() => {
    forced.current = true;
    setRevision((value) => value + 1);
  }, []);

  // The last answer for this working copy, or one another screen fetched.
  const known = skills?.key === key ? skills.list : skillCache.get(key)?.skills;
  const shownCommands = commands && commands.id === conversationId ? commands.list : [];
  const shownError = error?.key === key ? error.message : null;
  const loading = active && !!projectId && !known && !shownError;
  return { skills: known ?? [], commands: shownCommands, loading, error: shownError, refresh };
}
