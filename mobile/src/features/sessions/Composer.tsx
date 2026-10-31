import { ChoiceMenu } from "@/ui/ChoiceMenu";
import { Button, Divider, Host, HStack, Menu, Spacer } from "@expo/ui/swift-ui";
import { accessibilityLabel, contentShape, frame, shapes } from "@expo/ui/swift-ui/modifiers";
import { useCallback, useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { Alert, Platform, Image, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import Animated, {
  Easing,
  FadeIn,
  FadeOut,
  interpolate,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from "react-native-reanimated";

import { PulsingDot, useOffline } from "@/features/connection/ConnectionNotice";
import { SkillsSheet } from "@/features/skills/SkillsSheet";
import { SlashRow } from "@/features/skills/SlashRow";
import { insert, prepend, rank, tokenAt, type SlashToken } from "@/features/skills/slash";
import { useSlashCatalog, type SlashScope } from "@/features/skills/useSlashCatalog";
import type { ComposerMode, Harness } from "@/features/workspace/types";
import { FileTypeIcon } from "@/features/files/FileTypeIcon";
import { Icon, type IconName } from "@/ui/Icon";
import { LiquidView } from "@/ui/LiquidView";
import { themedStyles, useColors, useScheme, type Scheme } from "@/ui/theme";

import {
  isImage,
  MAX_ATTACHMENTS,
  merge,
  pickFiles,
  pickPhotos,
  type Attachment,
} from "./attachments";
import { DictationBar } from "./DictationBar";
import { cachedText } from "./composerDraft";
import { canDictate, insertDictation } from "./dictation";
import { ProviderLogo } from "./ProviderLogo";
import { COMPOSER_INPUT_ID } from "./SessionScreen";
import { useComposerDraft } from "./useComposerDraft";

export const COMPOSER_PLACEHOLDER = "Plan, ask, build…";
/** Mid-turn the desktop steers with the message or queues it, as its setting says. */
const BUSY_PLACEHOLDER = "Send a follow-up…";

const BUTTON = 36;
/** Gap between the round buttons and the composer's edge. */
const EDGE = 7;
const COLLAPSED = BUTTON + EDGE * 2;
const RADIUS = COLLAPSED / 2;
/** Expanded layout: context row on top, buttons and model along the bottom. */
const TOP_ROW = 46;
/** The attachments' strip, under the context row as on the desktop. */
const THUMB = 48;
const STRIP = THUMB + 10;
const BOTTOM_ROW = BUTTON + EDGE * 2;
const INPUT_MIN = 64;
const INPUT_MAX = 160;
/** Inset from the screen edges: roomier as a pill, wider as a card. */
const MARGIN_COLLAPSED = 20;
const MARGIN_EXPANDED = 12;
const MORPH = { duration: 280, easing: Easing.out(Easing.cubic) };

/** The desktop's + menu modes, in its order and with its words and hues. */
const MODES: {
  id: ComposerMode;
  label: string;
  hint: string;
  systemImage: ComponentProps<typeof Button>["systemImage"];
  tint: Record<Scheme, string>;
}[] = [
  {
    id: "plan",
    label: "Plan mode",
    hint: "Review a plan before building",
    systemImage: "lightbulb",
    tint: { dark: "#fde047", light: "#a16207" },
  },
  {
    id: "operator",
    label: "Operator",
    hint: "Give this thread access to MonoCode",
    systemImage: "cursorarrow.rays",
    tint: { dark: "#7dd3fc", light: "#0369a1" },
  },
  {
    id: "orchestrator",
    label: "Orchestrator",
    hint: "Plan and coordinate agent work",
    systemImage: "point.3.connected.trianglepath.dotted",
    tint: { dark: "#f0abfc", light: "#a21caf" },
  },
];
const DICTATION_FADE = { duration: 180, easing: Easing.out(Easing.cubic) };
/** A `/` suggestion's row, and how many show before the list scrolls. */
const SUGGESTION_ROW = 48;
const SUGGESTIONS_SHOWN = 3.5;

/**
 * The prompt field. At rest it's a single glass pill: add, the field and the
 * mic. Focused (or holding a draft) it grows into a card with the project and
 * branch on top and the model along the bottom.
 * One progress value drives the morph, and the TextInput stays the same
 * element throughout so focus and the keyboard are never interrupted. A React
 * Native TextInput, so keyboard-controller can track it (it matches `nativeID`).
 * While the desktop can't be reached, or the session's latest error stands,
 * the field gives way to a line saying so and nothing can be sent; a draft
 * waits under it.
 * With `modes`, the add menu also offers the desktop's Plan, Operator and
 * Orchestrator. The one picked shows as a chip by the model and goes with the
 * next message only, as on the desktop.
 * With `slash`, typing `/` or `$` suggests the chat's provider commands and
 * the project's skills above the field, and the add menu lists them all.
 * Either puts in `/name`, which the desktop reads as it does a typed one.
 * With `conversationId`, the text is the thread's: it follows the desktop
 * composer's text for the same thread, and typing here changes it there.
 */
export function Composer({
  conversationId,
  context,
  branch,
  model,
  effort,
  harness,
  onSend,
  busy = false,
  onStop,
  onModelPress,
  workspace,
  notice,
  modes = false,
  slash,
}: {
  /** The thread whose composer text this shares with the desktop. */
  conversationId?: string;
  /** Where this runs: the project, or the mono. */
  context: string;
  branch: string;
  model: string;
  /** The model's effort, shown dimmer after its name as on desktop. */
  effort?: string;
  harness?: Harness;
  onSend: (
    text: string,
    attachments: Attachment[],
    mode?: ComposerMode,
    /** The desktop composer revision the text was based on, which it clears. */
    draftRev?: number,
  ) => void | Promise<void>;
  busy?: boolean;
  onStop?: () => Promise<void>;
  /** Opens the model sheet. */
  onModelPress?: () => void;
  /** Where it works, shown in place of the context and branch. */
  workspace?: ReactNode;
  /** The session's latest error, in place of the field until tapped away. */
  notice?: { text: string; dismiss: () => void } | null;
  /** Offers the composer modes; a Mono's chat has none. */
  modes?: boolean;
  /** Where skills and commands come from; a Mono's chat has none. */
  slash?: SlashScope;
}) {
  const scheme = useScheme();
  const colors = useColors();
  const styles = useStyles();
  const offline = useOffline();
  const blocked = offline ? "offline" : notice ? "error" : null;
  const input = useRef<TextInput>(null);
  const [text, setText] = useState(() => cachedText(conversationId));
  const [sending, setSending] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [mode, setMode] = useState<ComposerMode | null>(null);
  const [inputHeight, setInputHeight] = useState(INPUT_MIN);
  const [dictating, setDictating] = useState(false);
  // Where dictated words go: the cursor, or the selection they replace.
  const selection = useRef({ start: 0, end: 0 });
  // The field's text as of its last change, ahead of the render that shows it,
  // since a selection event can arrive between the two.
  const latest = useRef(text);
  // The `/name` or `$name` at the cursor. Set only when it changes, so moving
  // the cursor, or focusing, doesn't commit alongside the keyboard's rise.
  const [token, setToken] = useState<SlashToken | null>(null);
  const [skillsOpen, setSkillsOpen] = useState(false);
  // The desktop's text for this thread, while the person isn't typing here.
  const showDesktopText = useCallback((next: string) => {
    latest.current = next;
    setText(next);
    setToken(null);
  }, []);
  const shared = useComposerDraft(conversationId, showDesktopText);
  const catalog = useSlashCatalog(slash, !!token || skillsOpen);
  const suggestions = token
    ? rank(token.query, token.trigger, catalog.skills, catalog.commands).slice(0, 30)
    : [];
  const canSend = (text.trim().length > 0 || attachments.length > 0) && !sending;
  // A draft, in words, files or a mode, keeps the card open.
  const hasText = text.length > 0 || attachments.length > 0 || mode !== null;

  // Focus lives on the UI thread rather than in React state. The keyboard
  // starts moving as the field focuses, and a React commit in that same frame
  // makes Reanimated skip the KeyboardStickyView animation on iOS (new arch),
  // so the composer jumps up before the keyboard arrives. Typing commits, but
  // by then the keyboard is up. See "Missing animations on iOS" in the
  // keyboard-controller KeyboardStickyView docs.
  const focused = useSharedValue(false);
  const progress = useSharedValue(0);
  const morph = (expand: boolean) => progress.set(withTiming(expand ? 1 : 0, MORPH));
  useEffect(() => {
    if (hasText) progress.set(withTiming(1, MORPH));
    else if (!focused.get()) progress.set(withTiming(0, MORPH));
  }, [hasText, focused, progress]);

  // Lets go of the keyboard as the field gives way.
  useEffect(() => {
    if (blocked) input.current?.blur();
  }, [blocked]);

  const strip = attachments.length > 0 ? STRIP : 0;
  const suggest = Math.min(suggestions.length, SUGGESTIONS_SHOWN) * SUGGESTION_ROW;
  const expandedHeight = TOP_ROW + strip + suggest + inputHeight + BOTTOM_ROW;

  const containerStyle = useAnimatedStyle(() => ({
    height: interpolate(progress.get(), [0, 1], [COLLAPSED, expandedHeight]),
    marginHorizontal: interpolate(progress.get(), [0, 1], [MARGIN_COLLAPSED, MARGIN_EXPANDED]),
  }));
  // The field slides from between the pill's buttons to its own band in the card.
  const inputStyle = useAnimatedStyle(() => {
    const p = progress.get();
    return {
      left: interpolate(p, [0, 1], [EDGE + BUTTON + 8, 18]),
      right: interpolate(p, [0, 1], [EDGE + BUTTON + 8, 18]),
      top: interpolate(p, [0, 1], [EDGE - 1, TOP_ROW + strip + suggest]),
      bottom: interpolate(p, [0, 1], [EDGE, BOTTOM_ROW]),
    };
  });
  // Dictation takes over the bottom row: its buttons, and the pill's field.
  const dictation = useSharedValue(0);
  // Android still hit-tests transparent views. These absolute rows overlap the
  // collapsed input, so only their children may receive touches once the card
  // has finished opening. Keep this on the UI thread with the morph (no focus
  // state/React commit that would interrupt the iOS keyboard animation).
  const expandedOnly = useAnimatedStyle(() => ({
    opacity:
      interpolate(progress.get(), [0.35, 1], [0, 1], "clamp") * (1 - dictation.get()),
    ...(Platform.OS === "android" ? {
      pointerEvents: progress.get() === 1 && dictation.get() === 0
        ? "box-none" as const
        : "none" as const,
    } : {}),
  }));
  useEffect(() => {
    dictation.set(withTiming(dictating ? 1 : 0, DICTATION_FADE));
  }, [dictating, dictation]);
  const undictated = useAnimatedStyle(() => ({
    opacity: 1 - dictation.get(),
    ...(Platform.OS === "android" ? {
      pointerEvents: dictation.get() === 0 && !blocked ? "auto" as const : "none" as const,
    } : {}),
  }));
  const fieldShown = useAnimatedStyle(() => ({
    opacity: 1 - dictation.get() * (1 - progress.get()),
  }));

  const dictate = () => {
    if (!canDictate()) {
      Alert.alert("Dictation unavailable", "Dictation needs iOS 26 or later.");
      return;
    }
    // The keyboard makes way; a draft keeps the card open above the recording.
    input.current?.blur();
    setDictating(true);
  };
  const endDictation = (words: string | null) => {
    setDictating(false);
    if (!words) return;
    setText((current) => {
      const next = insertDictation(current, words, selection.current);
      selection.current = { start: next.cursor, end: next.cursor };
      latest.current = next.text;
      shared.typed(next.text);
      return next.text;
    });
  };

  const attach = async (pick: (room: number) => Promise<Attachment[]>) => {
    const room = MAX_ATTACHMENTS - attachments.length;
    if (room <= 0) {
      Alert.alert("Attachment limit", `A message can carry up to ${MAX_ATTACHMENTS} files.`);
      return;
    }
    try {
      const picked = await pick(room);
      if (!picked.length) return;
      // Merged here, not in an updater, so its size error lands in the catch.
      setAttachments(merge(attachments, picked));
      // Ready to say what they're for.
      input.current?.focus();
    } catch (reason) {
      Alert.alert("Could not attach", reason instanceof Error ? reason.message : String(reason));
    }
  };

  // Picking the mode that's on turns it off, as the desktop's menu does.
  const toggleMode = (picked: ComposerMode) => {
    const next = mode === picked ? null : picked;
    setMode(next);
    // Ready to say what it's for.
    if (next) input.current?.focus();
  };

  const findToken = (value: string, cursor: number) => {
    const next = slash ? tokenAt(value, cursor) : null;
    setToken((current) =>
      current?.trigger === next?.trigger &&
      current?.start === next?.start &&
      current?.end === next?.end &&
      current?.query === next?.query
        ? current
        : next,
    );
  };
  const changeText = (value: string) => {
    // Where the cursor will be, until the selection event says.
    const cursor = Math.max(0, selection.current.end + value.length - latest.current.length);
    latest.current = value;
    setText(value);
    shared.typed(value);
    findToken(value, Math.min(cursor, value.length));
  };
  // Puts `/name` in the field and the cursor after it, ready for the request.
  const putCommand = (next: { text: string; cursor: number }) => {
    latest.current = next.text;
    selection.current = { start: next.cursor, end: next.cursor };
    setText(next.text);
    shared.typed(next.text);
    setToken(null);
    requestAnimationFrame(() => input.current?.setSelection(next.cursor, next.cursor));
  };
  const pickSuggestion = (name: string) => {
    if (token) putCommand(insert(text, token, name));
  };
  // From the sheet the name leads the message, where provider commands
  // must be; the field takes the keyboard back once the sheet is down.
  const pickFromSheet = (name: string) => {
    setSkillsOpen(false);
    putCommand(prepend(text, name));
    setTimeout(() => input.current?.focus(), 350);
  };
  const openSkills = () => {
    input.current?.blur();
    setSkillsOpen(true);
  };

  // Clears, lets go of the keyboard and folds back to the pill at once; the
  // draft comes back if the desktop never takes it.
  const send = async () => {
    // The draft stays put until the desktop can take it.
    if (!canSend || blocked) return;
    const message = text.trim();
    const files = attachments;
    const sentMode = mode;
    setSending(true);
    const draftRev = shared.sent();
    setText("");
    latest.current = "";
    setToken(null);
    setAttachments([]);
    setMode(null);
    focused.set(false);
    morph(false);
    input.current?.blur();
    try {
      await onSend(message, files, sentMode ?? undefined, draftRev);
    } catch (reason) {
      setText((current) => {
        latest.current = current || message;
        shared.typed(latest.current);
        return latest.current;
      });
      setAttachments((current) => (current.length ? current : files));
      setMode((current) => current ?? sentMode);
      Alert.alert("Message not sent", reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSending(false);
    }
  };

  return (
    <Animated.View style={containerStyle}>
      <LiquidView fill row={false} borderRadius={RADIUS} style={StyleSheet.absoluteFill}>
        <View />
      </LiquidView>

      <Animated.View style={[styles.contextRow, expandedOnly]}>
        <View style={styles.picker}>
          {workspace ?? (
            <Text style={styles.context} numberOfLines={1}>
              {context} <Text style={styles.branch}>{branch}</Text>
            </Text>
          )}
        </View>
      </Animated.View>

      {attachments.length > 0 ? (
        <Animated.View style={[styles.strip, expandedOnly]}>
          <ScrollView
            horizontal
            showsHorizontalScrollIndicator={false}
            keyboardShouldPersistTaps="handled"
            contentContainerStyle={styles.stripContent}
          >
            {attachments.map((attachment) => (
              <AttachmentChip
                key={attachment.id}
                attachment={attachment}
                onRemove={() =>
                  setAttachments((current) => current.filter((item) => item.id !== attachment.id))
                }
              />
            ))}
          </ScrollView>
        </Animated.View>
      ) : null}

      {suggestions.length > 0 ? (
        <Animated.View
          style={[styles.suggestions, { top: TOP_ROW + strip, height: suggest }, expandedOnly]}
        >
          <ScrollView
            keyboardShouldPersistTaps="always"
            showsVerticalScrollIndicator={false}
            accessibilityLabel={token?.trigger === "$" ? "Skills" : "Skills and commands"}
          >
            {suggestions.map((entry) => (
              <SlashRow
                key={`${entry.kind}:${entry.name}`}
                entry={entry}
                onPress={() => pickSuggestion(entry.name)}
              />
            ))}
          </ScrollView>
        </Animated.View>
      ) : null}

      <Animated.View style={[styles.inputSlot, inputStyle, fieldShown]}>
        <TextInput
          ref={input}
          nativeID={COMPOSER_INPUT_ID}
          value={text}
          editable={!dictating && !blocked}
          onChangeText={changeText}
          onSelectionChange={(event) => {
            const picked = event.nativeEvent.selection;
            selection.current = picked;
            findToken(latest.current, picked.start === picked.end ? picked.end : -1);
          }}
          onFocus={() => {
            focused.set(true);
            morph(true);
          }}
          onBlur={() => {
            focused.set(false);
            shared.flush();
            if (!hasText) morph(false);
          }}
          onContentSizeChange={(event) =>
            setInputHeight(
              Math.min(INPUT_MAX, Math.max(INPUT_MIN, event.nativeEvent.contentSize.height + 12)),
            )
          }
          placeholder={busy ? BUSY_PLACEHOLDER : COMPOSER_PLACEHOLDER}
          placeholderTextColor={colors.muted}
          multiline
          keyboardAppearance={scheme}
          cursorColor={colors.accent}
          selectionColor={colors.accent}
          style={[styles.input, blocked && styles.hidden]}
        />
        {blocked === "offline" ? (
          <Animated.View
            key="offline"
            entering={FadeIn.duration(DICTATION_FADE.duration)}
            exiting={FadeOut.duration(DICTATION_FADE.duration)}
            accessibilityRole="text"
            accessibilityLabel="Reconnecting to your desktop. Messages can't be sent until it's back."
            style={styles.status}
          >
            <PulsingDot />
            <Text style={styles.statusLabel} numberOfLines={1}>
              Reconnecting to desktop
            </Text>
          </Animated.View>
        ) : blocked === "error" && notice ? (
          <Animated.View
            key="error"
            entering={FadeIn.duration(DICTATION_FADE.duration)}
            exiting={FadeOut.duration(DICTATION_FADE.duration)}
            style={StyleSheet.absoluteFill}
          >
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={`Couldn't continue: ${notice.text}`}
              accessibilityHint="Dismisses the error so you can write again"
              onPress={notice.dismiss}
              style={({ pressed }) => [styles.status, pressed && styles.pressed]}
            >
              <Text style={[styles.statusLabel, styles.errorLabel]} numberOfLines={1}>
                {notice.text}
              </Text>
              <Icon name="close" size={14} color={colors.muted} />
            </Pressable>
          </Animated.View>
        ) : null}
      </Animated.View>

      {/* Bottom row, shared by both layouts: the buttons stay put as the card grows. */}
      <Animated.View
        pointerEvents={blocked ? "none" : "auto"}
        style={[styles.button, styles.leading, undictated, blocked && styles.disabled]}
      >
        <AddMenu
          onPhotos={() => void attach(pickPhotos)}
          onFiles={() => void attach(pickFiles)}
          mode={mode}
          onMode={modes ? toggleMode : undefined}
          onSkills={slash ? openSkills : undefined}
        />
      </Animated.View>

      <Animated.View
        style={[styles.modelChip, expandedOnly]}
      >
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`Model: ${model || "Desktop default"}${effort ? `, ${effort} effort` : ""}`}
          accessibilityHint="Choose the model, account, effort and permissions"
          onPress={onModelPress}
          style={({ pressed }) => [styles.modelPill, pressed && styles.pressed]}
        >
          {harness ? <ProviderLogo harness={harness} size={16} color={colors.content} /> : null}
          <Text style={styles.model} numberOfLines={1}>
            {model || "Desktop default"}
            {effort ? <Text style={styles.effort}> {effort}</Text> : null}
          </Text>
          <Icon name="chevronDown" size={14} color={colors.muted} />
        </Pressable>
        {mode ? <ModeChip mode={mode} onClear={() => setMode(null)} /> : null}
      </Animated.View>

      <Animated.View
        pointerEvents={blocked ? "none" : "auto"}
        style={[styles.button, styles.trailing, undictated, blocked && styles.disabled]}
      >
        {/* A draft mid-turn sends; without one the button stops the turn. */}
        {canSend ? (
          <RoundButton icon="send" label="Send" onPress={() => void send()} primary />
        ) : busy && onStop ? (
          <RoundButton label="Stop" onPress={() => { void onStop().catch(reason => Alert.alert("Could not stop", String(reason))); }} primary>
            <View style={styles.stop} />
          </RoundButton>
        ) : (
          <RoundButton icon="mic" label="Dictate" onPress={dictate} />
        )}
      </Animated.View>

      {dictating ? (
        <Animated.View
          entering={FadeIn.duration(DICTATION_FADE.duration)}
          exiting={FadeOut.duration(DICTATION_FADE.duration)}
          style={styles.dictation}
        >
          <DictationBar onClose={endDictation} />
        </Animated.View>
      ) : null}

      {slash ? (
        <SkillsSheet
          isPresented={skillsOpen}
          onDismiss={() => setSkillsOpen(false)}
          scope={slash}
          onPick={pickFromSheet}
        />
      ) : null}
    </Animated.View>
  );
}

/**
 * The add button: a native menu to attach photos or files, with `onSkills`
 * to list skills and commands and, with `onMode`, to pick a mode. The menu
 * lies over the button, its label an invisible box of the button's size, as
 * in SessionFilterMenu.
 */
function AddMenu({
  onPhotos,
  onFiles,
  mode,
  onMode,
  onSkills,
}: {
  onPhotos: () => void;
  onFiles: () => void;
  mode: ComposerMode | null;
  onMode?: (mode: ComposerMode) => void;
  onSkills?: () => void;
}) {
  const styles = useStyles();
  const colors = useColors();
  const label = onMode ? "Add files or pick a mode" : "Add photos or files";
  if (Platform.OS === "android") return (
    <ChoiceMenu label={label} actions={[
      { label: "Photo Library", onPress: onPhotos },
      { label: "Files", onPress: onFiles },
      ...(onSkills ? [{ label: "Skills and commands", onPress: onSkills }] : []),
      ...(onMode
        ? MODES.map((entry) => ({
            label: entry.label,
            selected: entry.id === mode,
            onPress: () => onMode(entry.id),
          }))
        : []),
    ]}>
      <View style={styles.round}><Icon name="add" size={18} color={colors.content} /></View>
    </ChoiceMenu>
  );
  return (
    <View>
      <View style={styles.round}>
        <Icon name="add" size={18} color={colors.content} />
      </View>
      <Host style={StyleSheet.absoluteFill}>
        <Menu
          modifiers={[accessibilityLabel(label)]}
          label={
            <HStack
              modifiers={[
                frame({ width: BUTTON, height: BUTTON }),
                contentShape(shapes.circle()),
              ]}
            >
              <Spacer />
            </HStack>
          }
        >
          <Button label="Photo Library" systemImage="photo.on.rectangle" onPress={onPhotos} />
          <Button label="Files" systemImage="folder" onPress={onFiles} />
          {onSkills ? (
            <Button label="Skills and commands" systemImage="sparkles" onPress={onSkills} />
          ) : null}
          {onMode ? <Divider /> : null}
          {onMode
            ? MODES.map((entry) => (
                <Button
                  key={entry.id}
                  label={entry.label}
                  // A tick marks the mode that's on, as a menu's checked item.
                  systemImage={entry.id === mode ? "checkmark" : entry.systemImage}
                  onPress={() => onMode(entry.id)}
                />
              ))
            : null}
        </Menu>
      </Host>
    </View>
  );
}

/** The mode the next message goes in, by the model; tapping it turns it off. */
function ModeChip({ mode, onClear }: { mode: ComposerMode; onClear: () => void }) {
  const styles = useStyles();
  const colors = useColors();
  const scheme = useScheme();
  const entry = MODES.find((candidate) => candidate.id === mode)!;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${entry.label}, on`}
      accessibilityHint="Turns it off for this message"
      hitSlop={4}
      onPress={onClear}
      style={({ pressed }) => [styles.modePill, pressed && styles.pressed]}
    >
      <Icon name={mode} size={16} color={entry.tint[scheme]} />
      <Text style={styles.modeLabel} numberOfLines={1}>
        {entry.label.replace(" mode", "")}
      </Text>
      <Icon name="close" size={12} color={colors.muted} />
    </Pressable>
  );
}

/** A held attachment: a photo's thumbnail, or a file's icon and name. */
function AttachmentChip({
  attachment,
  onRemove,
}: {
  attachment: Attachment;
  onRemove: () => void;
}) {
  const styles = useStyles();
  const colors = useColors();
  const image = isImage(attachment);
  return (
    <View style={image ? styles.thumb : styles.fileChip}>
      {image ? (
        <Image source={{ uri: attachment.uri }} style={styles.thumbImage} resizeMode="cover" />
      ) : (
        <>
          <FileTypeIcon name={attachment.name} isDir={false} size={20} />
          <Text style={styles.fileName} numberOfLines={1}>
            {attachment.name}
          </Text>
        </>
      )}
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Remove ${attachment.name}`}
        hitSlop={8}
        onPress={onRemove}
        style={({ pressed }) => [styles.remove, pressed && styles.pressed]}
      >
        <Icon name="close" size={10} color={colors.content} />
      </Pressable>
    </View>
  );
}

function RoundButton({
  icon,
  label,
  onPress,
  primary = false,
  children,
}: {
  icon?: IconName;
  label: string;
  onPress?: () => void;
  primary?: boolean;
  children?: ReactNode;
}) {
  const colors = useColors();
  const styles = useStyles();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      hitSlop={4}
      onPress={onPress}
      style={({ pressed }) => [
        styles.round,
        primary && styles.roundPrimary,
        pressed && styles.pressed,
      ]}
    >
      {children ??
        (icon ? (
          <Icon name={icon} size={18} color={primary ? colors.inverse : colors.content} />
        ) : null)}
    </Pressable>
  );
}

const useStyles = themedStyles((colors) => ({
  stop: {
    width: 12,
    height: 12,
    borderRadius: 3,
    backgroundColor: colors.inverse,
  },
  contextRow: {
    position: "absolute",
    top: 12,
    left: 14,
    right: 14,
    flexDirection: "row",
  },
  picker: {
    flexShrink: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    height: 30,
    paddingHorizontal: 6,
  },
  context: {
    flexShrink: 1,
    color: colors.content,
    fontSize: 15,
    fontWeight: "500",
  },
  branch: {
    color: colors.muted,
    fontWeight: "400",
  },
  inputSlot: {
    position: "absolute",
  },
  // Over the field, between the context row and the text being typed.
  suggestions: {
    position: "absolute",
    left: 8,
    right: 8,
  },
  strip: {
    position: "absolute",
    top: TOP_ROW,
    left: 0,
    right: 0,
    height: STRIP,
  },
  stripContent: {
    alignItems: "center",
    gap: 8,
    paddingHorizontal: 16,
  },
  thumb: {
    width: THUMB,
    height: THUMB,
  },
  thumbImage: {
    width: THUMB,
    height: THUMB,
    borderRadius: 10,
    backgroundColor: colors.fill,
  },
  fileChip: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    height: THUMB,
    maxWidth: 180,
    paddingLeft: 10,
    paddingRight: 14,
    borderRadius: 10,
    backgroundColor: colors.fill,
  },
  fileName: {
    flexShrink: 1,
    color: colors.content,
    fontSize: 13,
  },
  // Over the chip's top-right corner, as the desktop's.
  remove: {
    position: "absolute",
    top: -5,
    right: -5,
    width: 18,
    height: 18,
    borderRadius: 9,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: colors.surfaceRaised,
  },
  input: {
    flex: 1,
    paddingTop: 8,
    paddingBottom: 8,
    color: colors.content,
    fontSize: 17,
    lineHeight: 22,
    textAlignVertical: "top",
  },
  button: {
    position: "absolute",
    bottom: EDGE,
  },
  leading: {
    left: EDGE,
  },
  trailing: {
    right: EDGE,
  },
  // Over the bottom row, edge to edge between the pill's ends.
  dictation: {
    position: "absolute",
    left: EDGE,
    right: EDGE,
    bottom: EDGE,
    height: BUTTON,
  },
  modelChip: {
    position: "absolute",
    bottom: EDGE,
    left: EDGE + BUTTON + 8,
    right: EDGE + BUTTON + 8,
    flexDirection: "row",
  },
  // Shaped like the round buttons beside it: their height, tint and full rounding.
  modelPill: {
    flexShrink: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    height: BUTTON,
    paddingHorizontal: 12,
    borderRadius: BUTTON / 2,
    backgroundColor: colors.fill,
  },
  modePill: {
    flexShrink: 0,
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    height: BUTTON,
    marginLeft: 6,
    paddingHorizontal: 12,
    borderRadius: BUTTON / 2,
    backgroundColor: colors.fill,
  },
  modeLabel: {
    color: colors.content,
    fontSize: 15,
    fontWeight: "500",
  },
  model: {
    flexShrink: 1,
    color: colors.content,
    fontSize: 15,
    fontWeight: "500",
  },
  effort: {
    color: colors.muted,
    fontWeight: "400",
  },
  round: {
    width: BUTTON,
    height: BUTTON,
    borderRadius: BUTTON / 2,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: colors.fill,
  },
  roundPrimary: {
    backgroundColor: colors.inverseSurface,
  },
  pressed: {
    opacity: 0.6,
  },
  // In the field's place while nothing can be sent.
  status: {
    ...StyleSheet.absoluteFill,
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
  },
  statusLabel: {
    flexShrink: 1,
    color: colors.muted,
    fontSize: 15,
  },
  errorLabel: {
    flex: 1,
    color: colors.deletions,
  },
  hidden: {
    opacity: 0,
  },
  disabled: {
    opacity: 0.4,
  },
}));
