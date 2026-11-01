export type MobileSelection = { projectIds: string[]; monoIds: string[] };

/**
 * Whether monos are shown. A project brings its monos with it, so they are no
 * longer picked on their own; `monoIds` stays in the selection for the sake of
 * what an earlier version stored.
 */
export const MONOS_ENABLED = true;
export function normalizeSelection(value: unknown): MobileSelection | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (!Array.isArray(item.projectIds) || !Array.isArray(item.monoIds))
    return null;
  const ids = (values: unknown[]) => [
    ...new Set(
      values.filter((value): value is string => typeof value === "string"),
    ),
  ];
  return { projectIds: ids(item.projectIds), monoIds: ids(item.monoIds) };
}
export function selectedWorkspace<
  T extends {
    projects: { id: string }[];
    monos: { id: string; projectIds: string[] }[];
    sessions: { projectId: string; monoId?: string }[];
  },
>(catalog: T, selection: MobileSelection | null) {
  const projects = new Set(selection?.projectIds ?? []);
  // A mono comes with any of its projects; one with none belongs to no project
  // to bring it, so it is always shown.
  const monos = MONOS_ENABLED
    ? catalog.monos.filter(
        (mono) =>
          !mono.projectIds.length ||
          mono.projectIds.some((id) => projects.has(id)),
      )
    : [];
  const monoIds = new Set(monos.map((mono) => mono.id));
  return {
    ...catalog,
    projects: catalog.projects.filter((project) => projects.has(project.id)),
    monos,
    sessions: catalog.sessions.filter(
      (session) =>
        projects.has(session.projectId) ||
        (!!session.monoId && monoIds.has(session.monoId)),
    ),
  };
}
