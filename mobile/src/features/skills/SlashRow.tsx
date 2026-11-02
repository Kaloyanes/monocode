import { Pressable, Text, View } from "react-native";

import { Icon } from "@/ui/Icon";
import { themedStyles, useColors } from "@/ui/theme";

import type { SlashEntry } from "./slash";

/** A skill or provider command: its icon, `/name` and what it does. */
export function SlashRow({
  entry,
  onPress,
  lines = 1,
}: {
  entry: SlashEntry;
  onPress?: () => void;
  /** How many lines its description may take. */
  lines?: number;
}) {
  const styles = useStyles();
  const colors = useColors();
  const body = (
    <>
      <View style={styles.icon}>
        <Icon
          name={entry.kind === "skill" ? "sparkle" : "terminal"}
          size={16}
          color={colors.muted}
        />
      </View>
      <View style={styles.text}>
        <Text style={styles.name} numberOfLines={1}>
          /{entry.name}
        </Text>
        {entry.detail ? (
          <Text style={styles.detail} numberOfLines={lines}>
            {entry.detail}
          </Text>
        ) : null}
      </View>
    </>
  );
  if (!onPress) {
    return (
      <View accessible accessibilityLabel={`${entry.name}. ${entry.detail}`} style={styles.row}>
        {body}
      </View>
    );
  }
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${entry.kind === "skill" ? "Skill" : "Command"} ${entry.name}`}
      accessibilityHint={entry.detail || undefined}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed && styles.pressed]}
    >
      {body}
    </Pressable>
  );
}

const useStyles = themedStyles((colors) => ({
  row: {
    flexDirection: "row",
    alignItems: "center",
    gap: 12,
    minHeight: 48,
    paddingVertical: 7,
    paddingHorizontal: 6,
    borderRadius: 12,
  },
  pressed: {
    backgroundColor: colors.fill,
  },
  icon: {
    width: 30,
    height: 30,
    borderRadius: 9,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: colors.fill,
  },
  text: {
    flex: 1,
    gap: 1,
  },
  name: {
    color: colors.content,
    fontSize: 15,
    fontWeight: "500",
  },
  detail: {
    color: colors.muted,
    fontSize: 13,
    lineHeight: 18,
  },
}));
