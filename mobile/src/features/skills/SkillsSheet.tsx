import { BottomSheet, RNHostView } from "@expo/ui";
import { useState } from "react";
import {
  ActivityIndicator,
  ScrollView,
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
  SheetHeading,
  SheetTitle,
} from "@/ui/SheetParts";
import { SHEET_FADE, SHEET_FILL, SheetBody, SheetFade } from "@/ui/SheetScroll";
import { themedStyles, useColors, useScheme } from "@/ui/theme";

import { commandEntry, rank, scopeLabel, skillEntry, type SlashEntry } from "./slash";
import { SlashRow } from "./SlashRow";
import { useSlashCatalog, type SlashScope } from "./useSlashCatalog";

/** Where a row's text starts: the divider between rows lines up with it. */
const ROW_TEXT = SHEET_INSET + 30 + 12;

/**
 * Every skill and provider command the next message could use, from the add
 * menu: searchable, the chat's commands first, then skills by where they
 * live. Picking one hands back its name for the composer to put in.
 */
export function SkillsSheet({
  isPresented,
  onDismiss,
  scope,
  onPick,
}: {
  isPresented: boolean;
  onDismiss: () => void;
  scope: SlashScope;
  onPick: (name: string) => void;
}) {
  const styles = useStyles();
  const colors = useColors();
  const sheetColor = useSheetColor();
  const scheme = useScheme();
  const { width, height } = useWindowDimensions();
  const { bottom } = useSafeAreaInsets();
  const [query, setQuery] = useState("");
  const catalog = useSlashCatalog(scope, isPresented);

  // A command and a skill of one name show once, as the command: the
  // provider takes `/name` first.
  const commandNames = new Set(catalog.commands.map((command) => command.name.toLowerCase()));
  const skills = catalog.skills.filter((skill) => !commandNames.has(skill.name.toLowerCase()));
  const scopes = [...new Set(["project", "user", ...skills.map((skill) => skill.scope)])];
  const sections: { title: string; entries: SlashEntry[] }[] = query.trim()
    ? [{ title: "", entries: rank(query.trim(), "/", catalog.skills, catalog.commands) }]
    : [
        { title: "Commands", entries: catalog.commands.map(commandEntry) },
        ...scopes.map((scopeId) => ({
          title: `${scopeLabel(scopeId)} skills`,
          entries: skills.filter((skill) => skill.scope === scopeId).map(skillEntry),
        })),
      ];
  const shown = sections.filter((section) => section.entries.length > 0);

  const close = () => {
    setQuery("");
    onDismiss();
  };
  const pick = (name: string) => {
    setQuery("");
    onPick(name);
  };

  return (
    <BottomSheet
      isPresented={isPresented}
      onDismiss={close}
      snapPoints={[{ height: Math.round(height * 0.7) }, "full"]}
      contentPadding={0}
      containerColor={sheetColor}
      modifiers={SHEET_FILL}
    >
      <RNHostView>
        <SheetBody width={width} style={styles.sheet}>
          <SheetTitle title="Skills and commands" />
          <View style={styles.search}>
            <Icon name="search" size={16} color={colors.muted} />
            <TextInput
              accessibilityLabel="Search skills and commands"
              placeholder="Search"
              placeholderTextColor={colors.muted}
              value={query}
              onChangeText={setQuery}
              autoCapitalize="none"
              autoCorrect={false}
              returnKeyType="search"
              keyboardAppearance={scheme}
              cursorColor={colors.accent}
              selectionColor={colors.accent}
              style={styles.input}
            />
          </View>
          <SheetFade>
            <ScrollView
              keyboardShouldPersistTaps="handled"
              keyboardDismissMode="on-drag"
              showsVerticalScrollIndicator={false}
              contentContainerStyle={{
                paddingTop: SHEET_FADE - 14,
                paddingBottom: Math.max(bottom, SHEET_FADE) + 12,
              }}
            >
              {catalog.loading ? (
                <ActivityIndicator style={styles.status} color={colors.muted} />
              ) : catalog.error ? (
                <Text style={styles.empty}>{catalog.error}</Text>
              ) : shown.length === 0 ? (
                <Text style={styles.empty}>
                  {query.trim()
                    ? "Nothing matches."
                    : "No skills here yet. Add them on your desktop, in Settings → Skills."}
                </Text>
              ) : (
                shown.map((section) => (
                  <View key={section.title || "matches"}>
                    {section.title ? <SheetHeading>{section.title}</SheetHeading> : null}
                    <SheetCard>
                      {section.entries.map((entry, index) => (
                        <View key={`${entry.kind}:${entry.name}`}>
                          {index > 0 ? <SheetDivider inset={ROW_TEXT} /> : null}
                          <SlashRow
                            entry={entry}
                            lines={2}
                            onPress={() => pick(entry.name)}
                            grouped
                          />
                        </View>
                      ))}
                    </SheetCard>
                  </View>
                ))
              )}
            </ScrollView>
          </SheetFade>
        </SheetBody>
      </RNHostView>
    </BottomSheet>
  );
}

const useStyles = themedStyles((colors) => ({
  sheet: {
    paddingTop: SHEET_TOP,
    paddingHorizontal: SHEET_INSET,
    gap: 14,
  },
  search: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
    minHeight: 48,
    paddingHorizontal: SHEET_INSET,
    borderRadius: 16,
    borderCurve: "continuous",
    backgroundColor: colors.fill,
  },
  input: {
    flex: 1,
    color: colors.content,
    fontSize: 16,
    paddingVertical: 12,
  },
  status: {
    marginTop: 32,
  },
  empty: {
    color: colors.muted,
    fontSize: 15,
    lineHeight: 21,
    textAlign: "center",
    marginTop: 32,
    marginHorizontal: 24,
  },
}));
