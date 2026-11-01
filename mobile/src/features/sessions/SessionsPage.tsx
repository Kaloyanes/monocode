import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { router } from "expo-router";
import { ActivityIndicator, StyleSheet, Text, View } from "react-native";
import type { SwipeableMethods } from "react-native-gesture-handler/ReanimatedSwipeable";
import Animated, { FadeIn, FadeOut, LinearTransition } from "react-native-reanimated";

import { useConnection } from "@/features/connection/ConnectionProvider";
import { FileSearchBar, SEARCH_BAR } from "@/features/files/FileSearchBar";
import { useUnreadMonos } from "@/features/workspace/monoSeen";
import type { Harness, SessionSummary, Worktree } from "@/features/workspace/types";
import { useWorkspace } from "@/features/workspace/WorkspaceProvider";
import { AnimationsPaused } from "@/ui/AnimationsPaused";
import { GradualBlur } from "@/ui/GradualBlur";
import { themedStyles, useColors } from "@/ui/theme";

import { ListChips, ListHeading, MonoRow, type ListView } from "./MonosSublist";
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

/** The list with the sessions left out, kept one array so it isn't seen as a change. */
const NONE: SessionSummary[] = [];

/**
 * A project's Monos, then its sessions, newest first and paged in as the list
 * scrolls, under chips for All, Monos and Working now. Each session
 * swipes left to rename or delete, as on the desktop sidebar. Searched and
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
  const { sessions, monos: roster, edited, rememberSession } = useWorkspace();
  const desktop = useConnection().pairing?.desktop.fingerprint;
  const { pageTop, dockBottom } = useSessionLayout();
  // Room for the search bar where the composer rests, and the blur under it.
  const bottom = dockBottom + SEARCH_BAR + 16;
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SessionFilter>(NO_FILTER);
  const needle = query.trim().toLowerCase();
  const recent = sessions.filter((session) => session.projectId === projectId);
  const pages = useSessionPages(projectId, needle, active);
  const listed = visibleSessions(pages, recent, needle, edited).filter((session) =>
    inWorktree(session, worktree),
  );
  const visible = filterSessions(listed, filter);
  // The Monos that work in this project, and any that work in none, as the
  // search and Working now narrow them.
  const [view, setView] = useState<ListView>("all");
  const unread = useUnreadMonos(desktop, roster);
  const projectMonos = useMemo(
    () =>
      roster
        .filter((mono) => !mono.projectIds.length || mono.projectIds.includes(projectId))
        .sort((a, b) => (b.lastActiveAt || 0) - (a.lastActiveAt || 0)),
    [roster, projectId],
  );
  const monos = projectMonos.filter(
    (mono) =>
      (!needle || mono.name.toLowerCase().includes(needle)) &&
      (!filter.working || mono.status !== "idle"),
  );
  const onlyMonos = view === "monos" && projectMonos.length > 0;
  const openMono = useCallback(
    (monoId: string) => router.replace({ pathname: "/mono/[monoId]", params: { monoId } }),
    [],
  );
  const scope = JSON.stringify([projectId, needle, filter, worktree?.path, !!pages]);
  const arrivals = useArrivals(onlyMonos ? NONE : visible, scope, active);
  // The agents to filter by: those in the list, in the order first seen.
  const harnesses = [...new Set<Harness>(listed.map((session) => session.harness))];
  if (filter.harness && !harnesses.includes(filter.harness)) harnesses.push(filter.harness);
  const narrowed = !!needle || filter !== NO_FILTER || !!worktree;

  const actions = useSessionActions(() => {});
  const { closeOpenRow } = actions;
  useEffect(() => {
    if (!active) closeOpenRow();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  // The rows' handlers keep their identity, reaching the latest render's
  // through a ref, so a row only re-renders when its own session changes.
  const latest = useRef({ onOpen, rememberSession, actions });
  useLayoutEffect(() => {
    latest.current = { onOpen, rememberSession, actions };
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
      swipeOpened: (row) => latest.current.actions.swipeOpened(row),
    }),
    [],
  );
  const removingId = actions.removing?.id;
  const { arriving, moving } = arrivals;
  const renderItem = useCallback(
    ({ item }: { item: SessionSummary }) => (
      <SessionRow
        session={item}
        removing={removingId === item.id}
        arriving={arriving.has(item.id)}
        handlers={handlers}
      />
    ),
    [removingId, arriving, handlers],
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
          extraData={moving}
          keyExtractor={(session) => session.id}
          renderItem={renderItem}
          ItemSeparatorComponent={Divider}
          ListHeaderComponent={
            <View>
              <ListChips
                view={onlyMonos ? "monos" : "all"}
                onView={setView}
                working={filter.working}
                onWorking={(working) => setFilter((previous) => ({ ...previous, working }))}
                monos={projectMonos.length}
              />
              {monos.length ? (
                <View style={styles.monos}>
                  <ListHeading>Monos</ListHeading>
                  {monos.map((mono) => (
                    <MonoRow
                      key={mono.id}
                      mono={mono}
                      unread={unread.has(mono.id)}
                      shimmer={active}
                      onPress={openMono}
                    />
                  ))}
                </View>
              ) : null}
              {onlyMonos ? null : <ListHeading>Sessions</ListHeading>}
            </View>
          }
          ListEmptyComponent={
            onlyMonos ? (
              monos.length ? null : (
                <Text style={styles.empty}>No matching Monos.</Text>
              )
            ) : pages ? (
              <Text style={styles.empty}>
                {narrowed ? "No matching sessions." : "No sessions yet."}
              </Text>
            ) : null
          }
          ListFooterComponent={
            pages?.loadingMore ? <ActivityIndicator style={styles.more} color={colors.muted} /> : null
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
            <SessionFilterMenu value={filter} onChange={setFilter} harnesses={harnesses} />
          }
        />
      </View>
    </AnimationsPaused>
  );
}

/**
 * Whether a session works in `worktree`; any does without one. An older
 * desktop doesn't say where a session works, so it goes by the branch, which
 * only one working copy can have checked out.
 */
function inWorktree(session: SessionSummary, worktree: Worktree | undefined) {
  if (!worktree) return true;
  if (session.worktreePath) return samePath(session.worktreePath, worktree.path);
  return session.branch === worktree.name;
}

function samePath(a: string, b: string) {
  return a.replace(/\/+$/, "") === b.replace(/\/+$/, "");
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
      a.updatedAt === b.updatedAt &&
      a.branch === b.branch &&
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
  handlers,
}: {
  session: SessionSummary;
  removing: boolean;
  arriving: boolean;
  handlers: RowHandlers;
}) {
  return (
    <SessionCard
      session={session}
      detailed
      onPress={() => handlers.open(session)}
      onLongPress={() => handlers.showActions(session)}
      onRename={() => handlers.rename(session)}
      onDelete={() => handlers.remove(session)}
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
  sameRow(before.session, after.session));

const useStyles = themedStyles((colors) => ({
  page: {
    flex: 1,
  },
  list: {
    paddingHorizontal: 10,
  },
  monos: {
    paddingBottom: 18,
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
  divider: {
    height: StyleSheet.hairlineWidth,
    marginHorizontal: 14,
    backgroundColor: colors.stroke,
  },
}));
