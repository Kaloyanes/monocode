import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import type { SwipeableMethods } from "react-native-gesture-handler/ReanimatedSwipeable";
import Animated, { FadeIn, FadeOut, LinearTransition } from "react-native-reanimated";

import { router } from "expo-router";

import { useConnection } from "@/features/connection/ConnectionProvider";
import { FileSearchBar, SEARCH_BAR } from "@/features/files/FileSearchBar";
import { useUnreadMonos } from "@/features/workspace/monoSeen";
import type { Harness, Mono, SessionSummary, Worktree } from "@/features/workspace/types";
import { useCreateMono } from "@/features/workspace/useCreateMono";
import { useWorkspace } from "@/features/workspace/WorkspaceProvider";
import { AnimationsPaused } from "@/ui/AnimationsPaused";
import { GradualBlur } from "@/ui/GradualBlur";
import { Icon } from "@/ui/Icon";
import { themedStyles, useColors } from "@/ui/theme";
import { UndoToast } from "@/ui/UndoToast";
import { useToast } from "@/ui/useToast";

import { ArchivedSessions } from "./ArchivedSessions";
import { dayHeadings } from "./dayHeadings";
import { inWorktree } from "./inWorktree";
import { useMonosInDrawer } from "@/features/settings/monosInDrawer";
import { MonoGroup } from "./MonoGroup";
import {
  finishedFromMono,
  sameOrigin,
  groupSize,
  monosForProject,
  searchMonos,
  sessionOrigin,
  type SessionOrigin,
} from "./sessionMonos";
import { setMonoGroupPrefs, useMonoGroupPrefs } from "./monoGroupPrefs";
import { SessionCard } from "./SessionCard";
import {
  filterSessions,
  NO_FILTER,
  SessionFilterMenu,
  type SessionFilter,
} from "./SessionFilterMenu";
import { useSessionLayout } from "./SessionScreen";
import { ARRIVE_MS, useArrivals } from "./useArrivals";
import { REMOVE_MS, useSessionActions } from "./useSessionActions";
import { useSessionPages, visibleSessions } from "./useSessionPages";

/**
 * A project's sessions, newest first under the desktop sidebar's day headings
 * and paged in as the list scrolls, below the Monos pinned above them and the
 * ones working now, which leave their
 * place in the list while they run, as on the desktop sidebar. An Archived row
 * ends the list, and the filter menu has the same, opening the archive in
 * place of the list. Each session swipes left to archive, rename or delete,
 * as on the desktop sidebar; one a Mono started says so in a chip. Searched and
 * filtered from the bar where the composer rests on the chat page, and
 * narrowed to `worktree` when the header's menu picks one, as the desktop's
 * sidebar is. `active` while the page is in view: it reads the list then, and
 * its spinners only run then.
 */
export function SessionsPage({
  projectId,
  active,
  worktree,
  onOpen,
}: {
  projectId: string;
  active: boolean;
  worktree?: Worktree;
  onOpen: (session: SessionSummary) => void;
}) {
  const colors = useColors();
  const styles = useStyles();
  const { sessions, polled, edited, rememberSession, monos, allMonos, projects, canListArchived } =
    useWorkspace();
  const desktop = useConnection().pairing?.desktop.fingerprint;
  const { pageTop, dockBottom } = useSessionLayout();
  // Room for the search bar where the composer rests, and the blur under it.
  const bottom = dockBottom + SEARCH_BAR + 16;
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SessionFilter>(NO_FILTER);
  const needle = query.trim().toLowerCase();
  const recent = sessions.filter((session) => session.projectId === projectId);
  // Which sessions the poll carries, not their order: visibleSessions keeps the
  // order, so an agent finishing a turn costs no read.
  const roster = recent
    .flatMap((session) => (polled.has(session.id) ? [session.id] : []))
    .sort()
    .join(",");
  const pages = useSessionPages(projectId, needle, active, false, roster);
  const listed = visibleSessions(pages, recent, needle, edited, polled).filter((session) =>
    inWorktree(session, worktree),
  );
  // The sessions working now come from the poll, which carries every one, and
  // leave the paged list below while they do.
  const working = useMemo(
    () =>
      filterSessions(
        recent.filter(
          (session) => session.busy && inWorktree(session, worktree) && matches(session, needle),
        ),
        filter,
      ),
    [recent, worktree, needle, filter],
  );
  const rest = useMemo(
    () =>
      filterSessions(
        listed.filter((session) => !session.busy),
        filter,
      ),
    [listed, filter],
  );
  const visible = useMemo(() => [...working, ...rest], [working, rest]);
  const scope = JSON.stringify([projectId, needle, filter, worktree?.path, !!pages]);
  const arrivals = useArrivals(rest, scope, active);
  const dayStarts = dayHeadings(arrivals.sessions);
  const headingsKey = [...dayStarts].join();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const headings = useMemo(() => dayStarts, [headingsKey]);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const archive = useSessionPages(projectId, "", active && canListArchived, true, roster);
  // The agents to filter by: those in the list, in the order first seen.
  const harnesses = [...new Set<Harness>(listed.map((session) => session.harness))];
  if (filter.harness && !harnesses.includes(filter.harness)) harnesses.push(filter.harness);
  const narrowed = !!needle || filter !== NO_FILTER || !!worktree;

  const { toast, show, dismiss } = useToast();
  const sink = useMemo(() => ({ show, dismiss }), [show, dismiss]);
  const actions = useSessionActions(() => {}, sink);
  const { closeOpenRow } = actions;

  // The Monos pinned above the rows, unless the filter menu hides them.
  const prefs = useMonoGroupPrefs(desktop);
  // With "Monos in the drawer" on, they are listed there and not here.
  const [monosInDrawer] = useMonosInDrawer();
  const unread = useUnreadMonos(desktop, monos);
  const createMono = useCreateMono(projectId);
  const group = useMemo(
    () => searchMonos(monosForProject(monos, projects, projectId), needle),
    [monos, projects, projectId, needle],
  );
  // The filter offers "Monos" wherever the project has some, so it can come back.
  const hasMonos = !monosInDrawer && groupSize(monosForProject(monos, projects, projectId)) > 0;
  // Who started each session, from every Mono on the desktop.
  const everyMono = useMemo(() => new Map(allMonos.map((mono) => [mono.id, mono])), [allMonos]);
  const shownMonos = useMemo(() => new Set(monos.map((mono) => mono.id)), [monos]);
  const origins = useMemo(
    () => new Map(visible.map((session) => [session.id, sessionOrigin(session, everyMono, shownMonos)])),
    [visible, everyMono, shownMonos],
  );
  const openMono = useCallback((mono: Pick<Mono, "id">) => {
    // Pushed, as a session is, so it swipes back here and not to the drawer.
    router.push({ pathname: "/mono/[monoId]", params: { monoId: mono.id } });
  }, []);
  useEffect(() => {
    if (!active) closeOpenRow();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  // The rows' handlers keep their identity, reaching the latest render's
  // through a ref, so a row only re-renders when its own session changes.
  const monoMenu = (_: SessionSummary, mono: Mono) =>
    actions.archiveFinished(finishedFromMono(visible, mono.id), mono.name);
  const latest = useRef({ onOpen, rememberSession, actions, monoMenu });
  useLayoutEffect(() => {
    latest.current = { onOpen, rememberSession, actions, monoMenu };
  });
  const handlers = useMemo<RowHandlers>(
    () => ({
      open: (session) => {
        latest.current.rememberSession(session);
        latest.current.onOpen(session);
      },
      showActions: (session) => latest.current.actions.showActions(session),
      rename: (session) => latest.current.actions.rename(session),
      remove: (session) => latest.current.actions.remove(session),
      archive: (session) => latest.current.actions.archive(session),
      openMono,
      monoMenu: (session, mono) => latest.current.monoMenu(session, mono),
      swipeOpened: (row) => latest.current.actions.swipeOpened(row),
    }),
    [openMono],
  );
  const removingId = actions.removing?.id;
  const { arriving, moving } = arrivals;
  const renderItem = useCallback(
    ({ item, index }: { item: SessionSummary; index: number }) => {
      const heading = headings.get(item.id);
      return (
        <>
          {heading ? <SectionLabel title={heading} /> : index > 0 ? <Divider /> : null}
          <SessionRow
            session={item}
            removing={removingId === item.id}
            arriving={arriving.has(item.id)}
            origin={origins.get(item.id) ?? null}
            handlers={handlers}
          />
        </>
      );
    },
    [removingId, arriving, handlers, origins, headings],
  );

  if (archiveOpen)
    return (
      <ArchivedSessions
        projectId={projectId}
        active={active}
        worktree={worktree}
        roster={roster}
        onOpen={onOpen}
        onBack={() => setArchiveOpen(false)}
      />
    );

  return (
    <AnimationsPaused value={!active}>
      <View style={styles.page}>
        <Animated.FlatList
          itemLayoutAnimation={
            actions.removing
              ? LinearTransition.duration(REMOVE_MS)
              : moving
                ? LinearTransition.duration(ARRIVE_MS)
                : undefined
          }
          data={arrivals.sessions}
          // Re-renders the rows when they start or stop moving, so each takes up the transition.
          extraData={[moving, origins]}
          keyExtractor={(session) => session.id}
          renderItem={renderItem}
          ListHeaderComponent={
            <>
              {!monosInDrawer && prefs.shown && groupSize(group) > 0 ? (
                <MonoGroup
                  group={group}
                  unread={unread}
                  collapsed={prefs.collapsed}
                  onToggle={() => setMonoGroupPrefs(desktop, { collapsed: !prefs.collapsed })}
                  onOpen={openMono}
                  onNew={createMono}
                />
              ) : null}
              {working.length ? (
                <>
                  <SectionLabel title="Working" count={working.length} />
                  {working.map((session, index) => (
                    <View key={session.id}>
                      {index > 0 ? <Divider /> : null}
                      <SessionRow
                        session={session}
                        removing={removingId === session.id}
                        arriving={false}
                        origin={origins.get(session.id) ?? null}
                        handlers={handlers}
                      />
                    </View>
                  ))}
                </>
              ) : null}
            </>
          }
          ListEmptyComponent={
            pages && !working.length ? (
              <Text style={styles.empty}>
                {narrowed ? "No matching sessions." : "No sessions yet."}
              </Text>
            ) : null
          }
          ListFooterComponent={
            <>
              {pages?.loadingMore ? (
                <ActivityIndicator style={styles.more} color={colors.muted} />
              ) : null}
              {canListArchived && archive?.total ? (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`Archived, ${archive.total} sessions`}
                  onPress={() => {
                    closeOpenRow();
                    setArchiveOpen(true);
                  }}
                  style={({ pressed }) => [styles.archived, pressed && styles.pressed]}
                >
                  <Icon name="archive" size={18} color={colors.muted} />
                  <Text style={styles.archivedLabel}>Archived</Text>
                  <Text style={styles.archivedCount}>{archive.total}</Text>
                  <Icon name="chevronRight" size={14} color={colors.faint} />
                </Pressable>
              ) : null}
            </>
          }
          onScrollBeginDrag={closeOpenRow}
          onEndReached={pages?.loadMore}
          onEndReachedThreshold={0.5}
          initialNumToRender={12}
          windowSize={7}
          keyboardDismissMode="interactive"
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.list, { paddingTop: pageTop, paddingBottom: bottom }]}
          scrollIndicatorInsets={{ top: pageTop, bottom }}
          showsVerticalScrollIndicator={false}
        />
        <GradualBlur edge="bottom" height={bottom} color={colors.background} />
        <FileSearchBar
          value={query}
          onChange={setQuery}
          placeholder="Search sessions"
          accessory={
            <SessionFilterMenu
              value={filter}
              onChange={setFilter}
              harnesses={harnesses}
              onOpenArchived={
                canListArchived
                  ? () => {
                      closeOpenRow();
                      setArchiveOpen(true);
                    }
                  : undefined
              }
              monos={
                hasMonos
                  ? { shown: prefs.shown, onChange: (shown) => setMonoGroupPrefs(desktop, { shown }) }
                  : undefined
              }
            />
          }
        />
        <UndoToast toast={toast} onDismiss={dismiss} bottom={bottom + 8} />
      </View>
    </AnimationsPaused>
  );
}

/** Whether a session's title, model or branch has `needle` (lowercase) in it. */
function matches(session: SessionSummary, needle: string) {
  return (
    !needle ||
    [session.title, session.model, session.branch].some((field) =>
      field.toLowerCase().includes(needle),
    )
  );
}

/** A small title over a run of rows, such as the sessions working now. */
function SectionLabel({ title, count }: { title: string; count?: number }) {
  const styles = useStyles();
  return (
    <View style={styles.section} accessibilityRole="header">
      <Text style={styles.sectionTitle}>{title}</Text>
      {count ? <Text style={styles.sectionCount}>{count}</Text> : null}
    </View>
  );
}

/** A hairline between cards, inset to their text. */
function Divider() {
  const styles = useStyles();
  return <View style={styles.divider} />;
}

type RowHandlers = {
  open: (session: SessionSummary) => void;
  showActions: (session: SessionSummary) => void;
  rename: (session: SessionSummary) => void;
  remove: (session: SessionSummary) => void;
  archive: (session: SessionSummary) => void;
  /** A chip's Mono opens its chat. */
  openMono: (mono: Pick<Mono, "id">) => void;
  /** A chip held: archive the Mono's finished sessions. */
  monoMenu: (session: SessionSummary, mono: Mono) => void;
  swipeOpened: (row: SwipeableMethods) => void;
};

/** What a row shows of its session: a fresh list read hands over new objects
 *  for unchanged sessions, which shouldn't re-render their rows. */
function sameRow(a: SessionSummary, b: SessionSummary) {
  return (
    a === b ||
    (a.id === b.id &&
      a.title === b.title &&
      a.busy === b.busy &&
      a.draft === b.draft &&
      a.updatedAt === b.updatedAt &&
      a.workItem?.kind === b.workItem?.kind &&
      a.workItem?.number === b.workItem?.number &&
      a.branch === b.branch &&
      a.monoId === b.monoId &&
      a.model === b.model &&
      a.modelName === b.modelName &&
      a.harness === b.harness)
  );
}

/** A session in the list, re-rendered only when what it shows changes. */
const SessionRow = memo(function SessionRow({
  session,
  removing,
  arriving,
  origin,
  handlers,
}: {
  session: SessionSummary;
  removing: boolean;
  arriving: boolean;
  origin: SessionOrigin | null;
  handlers: RowHandlers;
}) {
  const mono = origin?.kind === "mono" ? origin.mono : undefined;
  return (
    <SessionCard
      session={session}
      detailed
      onPress={() => handlers.open(session)}
      onLongPress={() => handlers.showActions(session)}
      onRename={() => handlers.rename(session)}
      onDelete={() => handlers.remove(session)}
      onArchive={() => handlers.archive(session)}
      origin={origin}
      onOpenOrigin={mono && origin?.kind === "mono" && origin.openable ? () => handlers.openMono(mono) : undefined}
      onOriginLongPress={mono ? () => handlers.monoMenu(session, mono) : undefined}
      onSwipeOpen={handlers.swipeOpened}
      entering={arriving ? FadeIn.duration(ARRIVE_MS).delay(ARRIVE_MS * 0.3) : undefined}
      exiting={removing ? FadeOut.duration(REMOVE_MS * 0.6) : undefined}
    />
  );
},
(before, after) =>
  before.removing === after.removing &&
  before.arriving === after.arriving &&
  before.handlers === after.handlers &&
  sameOrigin(before.origin, after.origin) &&
  sameRow(before.session, after.session));

const useStyles = themedStyles((colors) => ({
  page: {
    flex: 1,
  },
  list: {
    paddingHorizontal: 10,
  },
  empty: {
    paddingHorizontal: 14,
    paddingTop: 4,
    color: colors.muted,
    fontSize: 15,
  },
  more: {
    paddingVertical: 16,
  },
  section: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    paddingHorizontal: 14,
    paddingTop: 10,
    paddingBottom: 4,
  },
  sectionTitle: {
    color: colors.muted,
    fontSize: 13,
    fontWeight: "600",
  },
  sectionCount: {
    color: colors.faint,
    fontSize: 13,
    fontVariant: ["tabular-nums"],
  },
  archived: {
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    minHeight: 44,
    marginTop: 8,
    paddingHorizontal: 14,
  },
  archivedLabel: {
    flex: 1,
    color: colors.muted,
    fontSize: 15,
    fontWeight: "500",
  },
  archivedCount: {
    color: colors.faint,
    fontSize: 14,
    fontVariant: ["tabular-nums"],
  },
  pressed: {
    opacity: 0.6,
  },
  divider: {
    height: StyleSheet.hairlineWidth,
    marginHorizontal: 14,
    backgroundColor: colors.stroke,
  },
}));
