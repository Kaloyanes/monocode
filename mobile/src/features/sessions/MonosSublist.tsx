import { memo } from "react";
import { Pressable, Text, View } from "react-native";

import { PixelMascot } from "@/features/mascots/PixelMascot";
import { plainText } from "@/features/sessions/markdownParser";
import type { Mono } from "@/features/workspace/types";
import { radii, themedStyles } from "@/ui/theme";
import { Shimmer } from "@/ui/Shimmer";

/** A Mono's mascot, as a chat list's avatar. */
const AVATAR = 40;

/** What the chips above the list show. */
export type ListView = "all" | "monos";

/**
 * The chips above the Sessions page's list: All, the Monos when there are any,
 * and Working now. Working now narrows both the Monos and the sessions, as the
 * filter menu's does; Monos leaves out the sessions.
 */
export function ListChips({
  view,
  onView,
  working,
  onWorking,
  monos,
}: {
  view: ListView;
  onView: (view: ListView) => void;
  working: boolean;
  onWorking: (working: boolean) => void;
  /** How many Monos the project has; the chip is left out with none. */
  monos: number;
}) {
  const styles = useStyles();
  const chip = (label: string, on: boolean, onPress: () => void, count?: number) => (
    <Pressable
      key={label}
      accessibilityRole="button"
      accessibilityState={{ selected: on }}
      onPress={onPress}
      style={({ pressed }) => [styles.chip, on && styles.chipOn, pressed && styles.pressed]}
    >
      <Text style={[styles.chipLabel, on && styles.chipLabelOn]}>{label}</Text>
      {count ? <Text style={styles.chipCount}>{count}</Text> : null}
    </Pressable>
  );
  return (
    <View style={styles.chips}>
      {chip("All", view === "all" && !working, () => {
        onView("all");
        onWorking(false);
      })}
      {monos ? chip("Monos", view === "monos", () => onView(view === "monos" ? "all" : "monos"), monos) : null}
      {chip("Working now", working, () => onWorking(!working))}
    </View>
  );
}

/** A plain header over a group of the list, as "Sessions" heads the cards. */
export function ListHeading({ children }: { children: string }) {
  const styles = useStyles();
  return <Text style={styles.heading}>{children}</Text>;
}

/** A Mono as a chat in a messaging app's list: its mascot, its name and when
 *  it was last active, and under them what it is doing or its last line. */
export const MonoRow = memo(function MonoRow({
  mono,
  unread,
  shimmer,
  onPress,
}: {
  mono: Mono;
  /** A reply it sent that the user hasn't seen. */
  unread: boolean;
  /** Whether a working Mono's line may shimmer: only where it can be seen. */
  shimmer: boolean;
  onPress: (id: string) => void;
}) {
  const styles = useStyles();
  const detail =
    mono.status === "working"
      ? "Working…"
      : mono.status === "needs-you"
        ? "Needs you"
        : oneLine(mono.lastMessage) || "No messages yet";
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${mono.name}, ${detail}`}
      onPress={() => onPress(mono.id)}
      style={({ pressed }) => [styles.row, pressed && styles.pressed]}
    >
      <PixelMascot name={mono.mascot} color={mono.color} status={mono.status} size={AVATAR} centered />
      <View style={styles.rowText}>
        <View style={styles.rowTop}>
          <Text style={styles.rowLabel} numberOfLines={1}>
            {mono.name}
          </Text>
          {mono.lastActiveAt ? (
            <Text style={[styles.rowTime, unread && styles.rowTimeUnread]}>{when(mono.lastActiveAt)}</Text>
          ) : null}
        </View>
        <View style={styles.rowTop}>
          {mono.status === "working" && shimmer ? (
            <Shimmer style={[styles.rowDetail, styles.fill]}>{detail}</Shimmer>
          ) : (
            <Text
              style={[styles.rowDetail, styles.fill, mono.status === "needs-you" && styles.rowNeedsYou]}
              numberOfLines={1}
            >
              {detail}
            </Text>
          )}
          {unread ? <View style={styles.unread} /> : null}
        </View>
      </View>
    </Pressable>
  );
});

/** When a Mono was last active, as a chat list dates its rows: the time
 *  today, Yesterday, else the day. */
function when(at: number) {
  const date = new Date(at);
  const today = new Date();
  if (date.toDateString() === today.toDateString())
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

/** The start of a message as one plain line: its markdown read for its
 *  words, so **bold** shows as bold rather than its asterisks. Only as much
 *  as one line can show is read. */
function oneLine(text: string) {
  // A reply that is only a reaction, as the chat shows it, by its emoji.
  const reaction = text.trim().match(/^<reaction>\s*(.{1,16}?)\s*<\/reaction>$/su);
  if (reaction) return `Reacted ${reaction[1]}`;
  return plainText(text.slice(0, 600));
}

const useStyles = themedStyles((colors) => ({
  chips: {
    flexDirection: "row",
    flexWrap: "wrap",
    gap: 8,
    paddingHorizontal: 4,
    paddingBottom: 14,
  },
  chip: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    height: 34,
    paddingHorizontal: 14,
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.stroke,
  },
  chipOn: {
    borderColor: colors.selection,
    backgroundColor: colors.selection,
  },
  chipLabel: {
    color: colors.muted,
    fontSize: 15,
  },
  chipLabelOn: {
    color: colors.content,
  },
  chipCount: {
    color: colors.faint,
    fontSize: 15,
  },
  pressed: {
    opacity: 0.6,
  },
  // Dimmed, inset like the cards' text.
  heading: {
    paddingHorizontal: 14,
    paddingBottom: 4,
    color: colors.muted,
    fontSize: 13,
    fontWeight: "500",
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    gap: 14,
    minHeight: 64,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: radii.row,
    borderCurve: "continuous",
  },
  rowText: {
    flex: 1,
    minWidth: 0,
    gap: 3,
  },
  rowTop: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
  },
  rowLabel: {
    flex: 1,
    color: colors.content,
    fontSize: 17,
    fontWeight: "600",
  },
  rowTime: {
    color: colors.muted,
    fontSize: 14,
    fontVariant: ["tabular-nums"],
  },
  rowTimeUnread: {
    color: colors.accent,
  },
  rowDetail: {
    color: colors.muted,
    fontSize: 15,
  },
  fill: {
    flex: 1,
  },
  rowNeedsYou: {
    color: colors.caution,
  },
  unread: {
    width: 10,
    height: 10,
    borderRadius: 5,
    backgroundColor: colors.accent,
  },
}));
