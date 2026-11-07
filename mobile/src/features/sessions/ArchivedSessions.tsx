import { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import Animated, { FadeOut, LinearTransition } from "react-native-reanimated";

import { FileSearchBar, SEARCH_BAR } from "@/features/files/FileSearchBar";
import type { SessionSummary, Worktree } from "@/features/workspace/types";
import { useWorkspace } from "@/features/workspace/WorkspaceProvider";
import { AnimationsPaused } from "@/ui/AnimationsPaused";
import { GradualBlur } from "@/ui/GradualBlur";
import { Icon } from "@/ui/Icon";
import { themedStyles, useColors } from "@/ui/theme";
import { UndoToast } from "@/ui/UndoToast";
import { useToast } from "@/ui/useToast";

import { inWorktree } from "./inWorktree";
import { SessionCard } from "./SessionCard";
import { useSessionLayout } from "./SessionScreen";
import { REMOVE_MS, useSessionActions } from "./useSessionActions";
import { useSessionPages, visibleSessions } from "./useSessionPages";

/**
 * A project's archived sessions, newest first and paged in as the list
 * scrolls, in place of the Sessions page's list as the desktop's archive view
 * takes over its sidebar: a way back at the top and a search bar where the
 * composer rests. Each row swipes left to restore, rename or delete. `roster`
 * names the sessions the project has now, so a session archived or restored
 * elsewhere shows up when it changes.
 */
export function ArchivedSessions({
  projectId,
  active,
  worktree,
  roster,
  onOpen,
  onBack,
}: {
  projectId: string;
  active: boolean;
  worktree?: Worktree;
  roster: string;
  onOpen: (session: SessionSummary) => void;
  onBack: () => void;
}) {
  const colors = useColors();
  const styles = useStyles();
  const { rememberSession, editedArchived } = useWorkspace();
  const { pageTop, dockBottom } = useSessionLayout();
  const bottom = dockBottom + SEARCH_BAR + 16;
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  const pages = useSessionPages(projectId, needle, active, true, roster);
  const sessions = useMemo(
    () =>
      visibleSessions(pages, [], needle, editedArchived).filter((session) =>
        inWorktree(session, worktree),
      ),
    [pages, needle, editedArchived, worktree],
  );

  const { toast, show, dismiss } = useToast();
  const sink = useMemo(() => ({ show, dismiss }), [show, dismiss]);
  const actions = useSessionActions(() => {}, sink);
  const { closeOpenRow } = actions;
  useEffect(() => {
    if (!active) closeOpenRow();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  const removingId = actions.removing?.id;
  const renderItem = ({ item }: { item: SessionSummary }) => (
    <SessionCard
      session={item}
      detailed
      archived
      onPress={() => {
        rememberSession(item);
        onOpen(item);
      }}
      onLongPress={() => actions.showArchivedActions(item)}
      onRename={() => actions.rename(item)}
      onDelete={() => actions.remove(item)}
      onArchive={() => actions.unarchive(item)}
      onSwipeOpen={actions.swipeOpened}
      exiting={removingId === item.id ? FadeOut.duration(REMOVE_MS * 0.6) : undefined}
    />
  );

  const total = pages?.total;
  return (
    <AnimationsPaused value={!active}>
      <View style={styles.page}>
        <Animated.FlatList
          itemLayoutAnimation={
            actions.removing ? LinearTransition.duration(REMOVE_MS) : undefined
          }
          data={sessions}
          keyExtractor={(session) => session.id}
          renderItem={renderItem}
          ItemSeparatorComponent={Divider}
          ListHeaderComponent={
            <View style={styles.header}>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Back to sessions"
                hitSlop={8}
                onPress={onBack}
                style={({ pressed }) => [styles.back, pressed && styles.pressed]}
              >
                <Icon name="chevronLeft" size={16} color={colors.content} />
              </Pressable>
              <Text style={styles.title} accessibilityRole="header">
                Archived
              </Text>
              {total ? <Text style={styles.count}>{total}</Text> : null}
            </View>
          }
          ListEmptyComponent={
            pages ? (
              <Text style={styles.empty}>
                {needle ? "No matching sessions." : "Nothing archived."}
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
        <FileSearchBar value={query} onChange={setQuery} placeholder="Search archived" />
        <UndoToast toast={toast} onDismiss={dismiss} bottom={bottom + 8} />
      </View>
    </AnimationsPaused>
  );
}

function Divider() {
  const styles = useStyles();
  return <View style={styles.divider} />;
}

const useStyles = themedStyles((colors) => ({
  page: {
    flex: 1,
  },
  list: {
    paddingHorizontal: 10,
  },
  header: {
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    paddingHorizontal: 4,
    paddingBottom: 8,
  },
  back: {
    width: 32,
    height: 32,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: 16,
    backgroundColor: colors.fill,
  },
  pressed: {
    opacity: 0.6,
  },
  title: {
    color: colors.content,
    fontSize: 17,
    fontWeight: "600",
  },
  count: {
    color: colors.faint,
    fontSize: 15,
    fontVariant: ["tabular-nums"],
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
