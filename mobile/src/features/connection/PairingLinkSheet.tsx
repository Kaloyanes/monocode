import { BottomSheet, RNHostView } from "@expo/ui";
import { useEffect, useRef, useState } from "react";
import { Pressable, Text, TextInput, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useSheetColor } from "@/ui/sheetColor";
import { SHEET_INSET, SHEET_TOP, SheetTitle, useSheetStyles } from "@/ui/SheetParts";
import { themedStyles, useColors, useScheme } from "@/ui/theme";

/** Takes a pairing link copied from the desktop, the keyboard up as it opens. */
export function PairingLinkSheet({
  isPresented,
  onDismiss,
  onConnect,
}: {
  isPresented: boolean;
  onDismiss: () => void;
  onConnect: (link: string) => void;
}) {
  const colors = useColors();
  const sheetColor = useSheetColor();
  const parts = useSheetStyles();
  const scheme = useScheme();
  const styles = useStyles();
  const { width } = useWindowDimensions();
  const { bottom } = useSafeAreaInsets();
  const input = useRef<TextInput>(null);
  const [link, setLink] = useState("");
  // Focused once the sheet is up, as focusing during its slide drops the keyboard.
  useEffect(() => {
    if (!isPresented) return;
    const timer = setTimeout(() => input.current?.focus(), 350);
    return () => clearTimeout(timer);
  }, [isPresented]);
  const dismiss = () => {
    setLink("");
    onDismiss();
  };
  const connect = () => {
    const value = link.trim();
    if (!value) return;
    setLink("");
    onConnect(value);
  };
  return (
    <BottomSheet
      isPresented={isPresented}
      onDismiss={dismiss}
      contentPadding={0}
      containerColor={sheetColor}
    >
      <RNHostView matchContents={{ vertical: true }}>
        <View style={[styles.sheet, { width, paddingBottom: Math.max(bottom, 16) + 8 }]}>
          <SheetTitle
            title="Paste a pairing link"
            detail="On your desktop, open Settings → Connections, choose Pair phone, and copy the link."
          />
          <TextInput
            ref={input}
            accessibilityLabel="Pairing link"
            placeholder="monocode://pair…"
            placeholderTextColor={colors.muted}
            value={link}
            onChangeText={setLink}
            onSubmitEditing={connect}
            autoCapitalize="none"
            autoCorrect={false}
            // The link carries the pairing secret.
            secureTextEntry
            returnKeyType="go"
            keyboardAppearance={scheme}
            cursorColor={colors.accent}
            selectionColor={colors.accent}
            style={parts.field}
          />
          <Pressable
            accessibilityRole="button"
            disabled={!link.trim()}
            onPress={connect}
            style={({ pressed }) => [
              parts.primary,
              !link.trim() && parts.dimmed,
              pressed && parts.pressed,
            ]}
          >
            <Text style={parts.primaryLabel}>Connect</Text>
          </Pressable>
        </View>
      </RNHostView>
    </BottomSheet>
  );
}

const useStyles = themedStyles(() => ({
  sheet: {
    paddingTop: SHEET_TOP,
    paddingHorizontal: SHEET_INSET,
    gap: 16,
  },
}));
