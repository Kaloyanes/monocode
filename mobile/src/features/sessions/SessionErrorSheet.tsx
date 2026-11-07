import { BottomSheet, RNHostView } from "@expo/ui";
import { Pressable, ScrollView, Text, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { Icon } from "@/ui/Icon";
import { useSheetColor } from "@/ui/sheetColor";
import { SHEET_INSET, SHEET_TOP, SheetCard, SheetTitle, useSheetStyles } from "@/ui/SheetParts";
import { themedStyles, useColors, withAlpha } from "@/ui/theme";

import { sessionErrorContent } from "./sessionError";

/** One sheet per transcript, outside the virtualized rows so it stays mounted. */
export function SessionErrorSheet({
  message,
  isPresented,
  onDismiss,
}: {
  message: string;
  isPresented: boolean;
  onDismiss: () => void;
}) {
  const colors = useColors();
  const sheetColor = useSheetColor();
  const parts = useSheetStyles();
  const styles = useStyles();
  const { width, height } = useWindowDimensions();
  const { bottom } = useSafeAreaInsets();
  const content = sessionErrorContent(message);
  return (
    <BottomSheet
      isPresented={isPresented}
      onDismiss={onDismiss}
      contentPadding={0}
      containerColor={sheetColor}
    >
      <RNHostView matchContents={{ vertical: true }}>
        <View style={[styles.sheet, { width, paddingBottom: Math.max(bottom, 16) + 8 }]}>
          <View style={styles.badge}>
            <Icon name={content.recovery ? "lock" : "chat"} size={24} color={colors.accent} />
          </View>
          <SheetTitle title={content.title} />
          <ScrollView
            style={[styles.details, { maxHeight: height * 0.35 }]}
            contentContainerStyle={styles.detailsContent}
            showsVerticalScrollIndicator={false}
          >
            <Text style={styles.message} selectable>{content.message}</Text>
            {content.recovery ? (
              <SheetCard style={styles.recovery}>
                <Icon name="computer" size={22} color={colors.muted} />
                <View style={styles.recoveryText}>
                  <Text style={styles.recoveryTitle}>Switch accounts</Text>
                  <Text style={styles.recoveryMessage}>{content.recovery}</Text>
                </View>
              </SheetCard>
            ) : null}
          </ScrollView>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Got it"
            onPress={onDismiss}
            style={({ pressed }) => [parts.primary, styles.button, pressed && parts.pressed]}
          >
            <Text style={parts.primaryLabel}>Got it</Text>
          </Pressable>
        </View>
      </RNHostView>
    </BottomSheet>
  );
}

const useStyles = themedStyles((colors) => ({
  sheet: {
    alignItems: "center",
    paddingTop: SHEET_TOP + 8,
    paddingHorizontal: SHEET_INSET,
  },
  badge: {
    width: 56,
    height: 56,
    borderRadius: 28,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: withAlpha(colors.accent, 0.12),
    marginBottom: 14,
  },
  details: {
    alignSelf: "stretch",
    marginTop: 8,
    marginBottom: 20,
  },
  detailsContent: {
    gap: 16,
  },
  message: {
    color: colors.muted,
    fontSize: 15,
    lineHeight: 21,
    textAlign: "center",
  },
  recovery: {
    flexDirection: "row",
    alignItems: "flex-start",
    gap: 12,
    padding: SHEET_INSET,
  },
  recoveryText: {
    flex: 1,
    gap: 4,
  },
  recoveryTitle: {
    color: colors.content,
    fontSize: 15,
    fontWeight: "600",
    lineHeight: 20,
  },
  recoveryMessage: {
    color: colors.muted,
    fontSize: 14,
    lineHeight: 20,
  },
  button: {
    alignSelf: "stretch",
  },
}));
