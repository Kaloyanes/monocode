import { BottomSheet, RNHostView } from "@expo/ui";
import { useRef, useState } from "react";
import {
  Pressable,
  Text,
  TextInput,
  useWindowDimensions,
  View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { Icon } from "@/ui/Icon";
import { useSheetColor } from "@/ui/sheetColor";
import {
  SHEET_INSET,
  SHEET_TOP,
  SheetCard,
  SheetDivider,
  SheetTitle,
  useSheetStyles,
} from "@/ui/SheetParts";
import { themedStyles, useColors } from "@/ui/theme";

import { type NewAction, useTerminalActions } from "./terminalActions";

/**
 * The commands a project's terminal menu offers, in a native sheet the height
 * of the list. They are the desktop footer's actions: a change here is sent
 * to the desktop, and one made there shows here. Each runs in the project's
 * folder when picked from the session's terminal button.
 */
export function TerminalActionsSheet({
  projectId,
  projectName,
  isPresented,
  onDismiss,
}: {
  projectId: string;
  projectName: string;
  isPresented: boolean;
  onDismiss: () => void;
}) {
  const sheetColor = useSheetColor();
  const { width } = useWindowDimensions();
  const { bottom } = useSafeAreaInsets();
  return (
    <BottomSheet
      isPresented={isPresented}
      onDismiss={onDismiss}
      contentPadding={0}
      containerColor={sheetColor}
    >
      <RNHostView matchContents={{ vertical: true }}>
        <View style={{ width, paddingBottom: Math.max(bottom, 16) + 8 }}>
          {/* Read fresh each time it opens, after the menu may have changed them. */}
          {isPresented ? (
            <ActionsEditor projectId={projectId} projectName={projectName} />
          ) : null}
        </View>
      </RNHostView>
    </BottomSheet>
  );
}

// No ScrollView: the sheet is as tall as the list, and a scroll view inside a
// hosted view has crashed Fabric as the page mounts (see CommitSheet).
function ActionsEditor({
  projectId,
  projectName,
}: {
  projectId: string;
  projectName: string;
}) {
  const [actions, save] = useTerminalActions(projectId);
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [error, setError] = useState<string | null>(null);
  const styles = useStyles();
  const parts = useSheetStyles();
  const colors = useColors();
  const commandInput = useRef<TextInput>(null);
  const ready = name.trim() !== "" && command.trim() !== "";
  const change = (next: NewAction[]) => {
    setError(null);
    save(next).catch((reason: unknown) =>
      setError(reason instanceof Error ? reason.message : String(reason)),
    );
  };
  const add = () => {
    if (!ready) return;
    change([
      ...actions,
      { name: name.trim(), command: command.trim(), icon: "play" },
    ]);
    setName("");
    setCommand("");
  };
  return (
    <View style={styles.content}>
      <SheetTitle
        title="Actions"
        detail={`${projectName} · the same as the desktop’s footer`}
      />
      <SheetCard>
        {actions.map((action, index) => (
          <View key={action.id ?? `new-${index}`}>
            {index > 0 ? <SheetDivider /> : null}
            <View style={styles.row}>
              <View style={styles.rowText}>
                <Text style={styles.name} numberOfLines={1}>
                  {action.name}
                </Text>
                <Text style={styles.command} numberOfLines={1}>
                  {action.command}
                </Text>
              </View>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`Remove ${action.name}`}
                hitSlop={10}
                onPress={() => change(actions.filter((_, at) => at !== index))}
              >
                <Icon name="trash" size={18} color={colors.muted} />
              </Pressable>
            </View>
          </View>
        ))}
        {actions.length === 0 ? (
          <Text style={styles.empty}>
            No actions yet. Add one here or from the desktop.
          </Text>
        ) : null}
      </SheetCard>
      <View style={styles.form}>
        <TextInput
          style={parts.field}
          value={name}
          onChangeText={setName}
          onSubmitEditing={() => commandInput.current?.focus()}
          placeholder="Name, like Run Dev"
          placeholderTextColor={colors.muted}
          returnKeyType="next"
          submitBehavior="submit"
          accessibilityLabel="New action's name"
        />
        <View style={styles.add}>
          <TextInput
            ref={commandInput}
            style={[parts.field, styles.mono, styles.grow]}
            value={command}
            onChangeText={setCommand}
            onSubmitEditing={add}
            placeholder="npm run dev"
            placeholderTextColor={colors.muted}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="done"
            accessibilityLabel="New action's command"
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Add action"
            disabled={!ready}
            onPress={add}
            style={[parts.primary, styles.addButton, !ready && parts.dimmed]}
          >
            <Text style={parts.primaryLabel}>Add</Text>
          </Pressable>
        </View>
      </View>
      {error ? <Text style={styles.error}>{error}</Text> : null}
    </View>
  );
}

const useStyles = themedStyles((colors) => ({
  content: { paddingHorizontal: SHEET_INSET, paddingTop: SHEET_TOP, gap: 16 },
  row: {
    flexDirection: "row",
    alignItems: "center",
    gap: 12,
    minHeight: 56,
    paddingHorizontal: SHEET_INSET,
    paddingVertical: 9,
  },
  rowText: { flex: 1, gap: 2 },
  name: { color: colors.content, fontSize: 16, fontWeight: "500" },
  command: { color: colors.muted, fontSize: 13, fontFamily: "Menlo" },
  empty: { color: colors.muted, padding: SHEET_INSET, fontSize: 15 },
  form: { gap: 8 },
  add: { flexDirection: "row", gap: 8 },
  mono: { fontFamily: "Menlo", fontSize: 15 },
  grow: { flex: 1 },
  addButton: { height: 48, paddingHorizontal: 22, borderRadius: 24 },
  error: { color: colors.deletions, fontSize: 13 },
}));
