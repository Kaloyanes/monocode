import { UsageLimitCard } from "@/features/sessions/UsageLimitCard";
import {
  router,
  Stack,
  useFocusEffect,
  useIsFocused,
  useLocalSearchParams,
} from "expo-router";
import * as Haptics from "expo-haptics";
import { use, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Alert, Platform, Text } from "react-native";
import { ConnectionNotice } from "@/features/connection/ConnectionNotice";
import { hasNativeTranscript } from "@/features/connection/nativeLink";

import { PixelMascot } from "@/features/mascots/PixelMascot";
import { useMonoModel } from "@/features/models/useMonoModel";
import { androidBackHeader } from "@/ui/AndroidHeader";
import { Icon } from "@/ui/Icon";
import { Timed } from "@/ui/Timed";
import { useMonoTools, useConnectionActions, usePairing } from "@/features/connection/ConnectionProvider";
import { markMonoSeen, markMonoUnread } from "@/features/workspace/monoSeen";
import { Composer } from "@/features/sessions/Composer";
import { MonoChoose, MonoMessageActions } from "@/features/sessions/MonoCards";
import { MonoMenu } from "@/features/sessions/MonoMenu";
import { useMonoAccent } from "@/features/workspace/monoAccent";
import { AccentOverride, ACCENTS, mix, themedStyles, useAccent, useScheme } from "@/ui/theme";
import { GlassBlurTarget, LiquidView } from "@/ui/LiquidView";
import { useNativeHeaderHeight } from "@/ui/useNativeHeaderHeight";
import type { Mono } from "@/features/workspace/types";
import { QuestionCard } from "@/features/sessions/QuestionCard";
import { EmptySession } from "@/features/sessions/EmptySession";
import {
  HEADER_HEIGHT,
  SessionContent,
  SessionFrame,
  SessionScreen,
} from "@/features/sessions/SessionScreen";
import { useMonoOnlyLayout } from "@/features/settings/monoOnly";
import { Transcript } from "@/features/sessions/Transcript";
import { NativeTranscript } from "@/features/sessions/NativeTranscript";
import type { TranscriptAgent } from "@/features/sessions/TranscriptTurn";
import {
  monoConversationId,
  useConversation,
  useConversationTurns,
  useWorkspace,
} from "@/features/workspace/WorkspaceProvider";

// A mono has one long-lived conversation, so its screen is that conversation.
// It is pushed over the list it opened from as a session is over its project:
// under the stack's native header, with back on the left and its ⋯ on the
// right, and a frame of its own for its composer, so the whole screen slides
// in and out with the push and swipes back to that list.
export default function MonoRoute() {
  const headerHeight = useNativeHeaderHeight();
  return (
    <SessionFrame nativeHeaderHeight={headerHeight}>
      <MonoScreen />
    </SessionFrame>
  );
}

/** Off the stack, to the list under it, or the start with nothing there. */
function leave() {
  if (router.canGoBack()) router.back();
  else router.replace("/");
}

/** A Mono that's gone, say deleted: its screen leaves once it is in front,
 *  rather than replacing itself and stacking the list on the list. */
function MonoGone() {
  useFocusEffect(leave);
  return null;
}

function MonoScreen() {
  const [monoOnly] = useMonoOnlyLayout();
  const { monoId } = useLocalSearchParams<{ monoId: string }>();
  const { monos, sendMessage, stop } = useWorkspace();
  const blurTarget = use(GlassBlurTarget);
  const mono = monos.find((m) => m.id === monoId);
  const conversationId = mono ? monoConversationId(mono.id) : null;
  const conversation = useConversation(conversationId);
  const { command, refresh } = useConnectionActions();
  // The older desktop app has no profile, reset or delete for a Mono.
  const tools = useMonoTools();
  const desktop = usePairing()?.desktop.fingerprint;
  // Its replies are seen as they arrive while its chat is in front, unless
  // it was just marked unread: that holds until the chat is left.
  const focused = useIsFocused();
  const keptUnread = useRef(false);
  useEffect(() => {
    if (!focused) keptUnread.current = false;
    else if (mono && !keptUnread.current) markMonoSeen(desktop, mono);
  }, [focused, desktop, mono]);
  // A Mono's message being replied to, quoted over the next one sent.
  const [reply, setReply] = useState<string | null>(null);
  const messageActions = useMemo(
    () => ({
      reply: setReply,
      // As Messages: the chat reads as new again, and the list comes up.
      markUnread: () => {
        if (!mono) return;
        keptUnread.current = true;
        markMonoUnread(desktop, mono);
        leave();
      },
    }),
    [desktop, mono],
  );
  // Stable, so the memoized turns keep their rows across polls.
  const name = mono?.name ?? "";
  const mascot = mono?.mascot ?? "";
  const color = mono?.color ?? "";
  const agent = useMemo(() => ({ name, mascot, color }), [name, mascot, color]);
  const { model, harness, effort } = useMonoModel(mono);
  // The colour picked for this Mono's chat, over the app's accent.
  const monoAccent = useMonoAccent(mono?.id);
  const scheme = useScheme();
  // Monos only, a chat with no colour picked, here or for the app, is drawn
  // in the Mono's own: as is in the dark, deepened in the light to hold its
  // contrast on the pale background.
  const appAccent = useAccent();
  const ownAccent =
    monoOnly && appAccent === "theme" && /^#[0-9a-f]{6}$/i.test(color)
      ? scheme === "light"
        ? mix(color, "#000000", 0.22)
        : color
      : null;
  const chatAccent = monoAccent === "theme" ? ownAccent : ACCENTS[monoAccent][scheme];
  const choose = useCallback(
    (text: string) => (conversationId ? sendMessage(conversationId, text) : Promise.resolve()),
    [conversationId, sendMessage],
  );
  // A reset or delete, then the catalog read again: a deleted Mono's screen
  // leaves for the start once it's gone from there.
  const act = async (run: () => Promise<unknown>) => {
    try {
      await run();
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      await refresh();
    } catch (reason) {
      Alert.alert("Couldn’t do that", reason instanceof Error ? reason.message : String(reason));
    }
  };
  if (!mono || !conversationId) return <MonoGone />;

  const openDetails = () =>
    router.push({ pathname: "/mono-profile/[monoId]", params: { monoId: mono.id } });
  // Both clear what can't come back, so each asks first, as the desktop does.
  const confirmReset = () =>
    Alert.alert(
      "Reset conversation?",
      `The chat with ${mono.name} will be cleared. Its soul, memory and habits stay.`,
      [
        { text: "Cancel", style: "cancel" },
        {
          text: "Reset",
          style: "destructive",
          onPress: () => void act(() => command("mono.reset", { monoId: mono.id })),
        },
      ],
    );
  const confirmDelete = () =>
    Alert.alert(
      `Delete ${mono.name}?`,
      "Its scheduled habits stop. Sessions and notes it created stay saved.",
      [
        { text: "Cancel", style: "cancel" },
        {
          text: "Delete",
          style: "destructive",
          onPress: () => void act(() => command("mono.delete", { monoId: mono.id })),
        },
      ],
    );
  const openSettings = () => router.push("/settings");
  const title = <MonoTitle mono={mono} onPress={tools ? openDetails : undefined} />;
  // The native header's buttons render outside this tree.
  const headerSurface = (content: ReactNode) => (
    <GlassBlurTarget value={blurTarget}>{content}</GlassBlurTarget>
  );

  return (
    <Timed id="mono screen">
    <SessionScreen
      composer={
        <AccentOverride value={chatAccent}>
          {conversation.question ? (
            <QuestionCard conversationId={conversationId} prompt={conversation.question} />
          ) : null}
          {conversationId && conversation.usageLimit ? (
            <UsageLimitCard key={`usage:${conversationId}`} conversationId={conversationId} state={conversation.usageLimit} />
          ) : null}
          <Composer
            key={mono.id}
            // One line: the header names the Mono, and its model lives in settings.
            active={focused}
            compact
            context=""
            branch=""
            model={model}
            effort={effort}
            harness={harness}
            onModelPress={() =>
              router.push({
                pathname: "/models",
                params: { projectId: "", sessionId: conversationId },
              })
            }
            busy={conversation.busy}
            onStop={() => stop(conversationId)}
            reply={reply}
            onClearReply={() => setReply(null)}
            onSend={(text, attachments) => {
              // A reply opens with the message it answers, quoted on one line,
              // so the Mono reads it in context and the chat shows it above.
              const quoted = reply?.replace(/\s+/g, " ").trim();
              setReply(null);
              return sendMessage(
                conversationId,
                quoted ? `> ${quoted.length > 280 ? quoted.slice(0, 279) + "…" : quoted}\n\n${text}` : text,
                attachments,
              );
            }}
          />
        </AccentOverride>
      }
    >
      <ConnectionNotice error={conversation.error} />
      {/* The stack's native header: back from the stack, the Mono's pill for
          a title, and on the right its details, a fresh start or deleting it
          from ⋯, or without them settings, as on a project. */}
      <Stack.Screen
        options={
          Platform.OS === "android"
            ? {
                ...androidBackHeader(headerSurface(title)),
                headerTitleAlign: "left",
                headerRight: () =>
                  headerSurface(
                    tools ? (
                      <MonoMenu
                        onDetails={openDetails}
                        onReset={confirmReset}
                        onDelete={confirmDelete}
                      />
                    ) : (
                      <SettingsButton onPress={openSettings} />
                    ),
                  ),
              }
            : { headerTitle: () => title }
        }
      />
      {Platform.OS === "android" ? null : (
        <Stack.Toolbar placement="right">
          {tools ? (
            <Stack.Toolbar.Menu icon="ellipsis" accessibilityLabel="More">
              <Stack.Toolbar.MenuAction icon="info.circle" onPress={openDetails}>
                View details
              </Stack.Toolbar.MenuAction>
              <Stack.Toolbar.MenuAction icon="arrow.counterclockwise" onPress={confirmReset}>
                Reset conversation
              </Stack.Toolbar.MenuAction>
              <Stack.Toolbar.MenuAction icon="trash" destructive onPress={confirmDelete}>
                Delete Mono
              </Stack.Toolbar.MenuAction>
            </Stack.Toolbar.Menu>
          ) : (
            <Stack.Toolbar.Button
              icon="gearshape"
              accessibilityLabel="Settings"
              onPress={openSettings}
            />
          )}
        </Stack.Toolbar>
      )}
      <SessionContent>
        {/* A suggested reply it offers is sent as the user's message. */}
        <AccentOverride value={chatAccent}>
          <MonoChoose value={choose}>
            <MonoMessageActions value={messageActions}>
              {hasNativeTranscript
                ? <NativeTranscript key={mono.id} conversationId={conversationId} agent={agent} />
                : <MonoTranscript key={mono.id} conversationId={conversationId} agent={agent} />}
            </MonoMessageActions>
          </MonoChoose>
        </AccentOverride>
      </SessionContent>
    </SessionScreen>
    </Timed>
  );
}

/** A Mono's header mascot holds still while it works: its chat shows that,
 *  beside the "Thinking…" line. The bulb still asks for you. */
function headerStatus(mono: Mono) {
  return mono.status === "working" ? "idle" : mono.status;
}

/** Android's settings button in the header, for an older desktop's Mono. */
function SettingsButton({ onPress }: { onPress: () => void }) {
  const styles = useTitleStyles();
  return (
    <LiquidView
      borderRadius={HEADER_HEIGHT / 2}
      innerStyle={styles.button}
      onPress={onPress}
      accessibilityLabel="Settings"
    >
      <Icon name="settings" size={20} />
    </LiquidView>
  );
}

/** A pushed Mono's title: its mascot and name in a glass pill, which opens
 *  its profile. */
function MonoTitle({ mono, onPress }: { mono: Mono; onPress?: () => void }) {
  const styles = useTitleStyles();
  return (
    <LiquidView
      borderRadius={HEADER_HEIGHT / 2}
      style={styles.frame}
      innerStyle={styles.pill}
      onPress={onPress}
      accessibilityLabel={onPress ? mono.name : undefined}
      pressedScale={onPress ? 1.04 : undefined}
    >
      <PixelMascot name={mono.mascot} color={mono.color} status={headerStatus(mono)} size={28} />
      <Text style={styles.label} numberOfLines={1}>
        {mono.name}
      </Text>
    </LiquidView>
  );
}

const useTitleStyles = themedStyles((colors) => ({
  // Kept narrow so a long name truncates rather than spanning the header.
  frame: {
    maxWidth: 220,
  },
  pill: {
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    height: HEADER_HEIGHT,
    paddingLeft: 12,
    paddingRight: 18,
  },
  label: {
    flexShrink: 1,
    color: colors.content,
    fontSize: 17,
    fontWeight: "600",
  },
  button: {
    width: HEADER_HEIGHT,
    height: HEADER_HEIGHT,
    alignItems: "center",
    justifyContent: "center",
  },
}));

/** The mono's turns, read here so a streaming reply re-renders only the transcript. */
function MonoTranscript({
  conversationId,
  agent,
}: {
  conversationId: string;
  agent: TranscriptAgent;
}) {
  const { turns, hasOlder, loadOlder } = useConversationTurns(conversationId);
  return turns.length > 0 ? (
    <Transcript
      turns={turns}
      conversationId={conversationId}
      agent={agent}
      hasOlder={hasOlder}
      loadOlder={loadOlder}
    />
  ) : (
    <EmptySession mono={agent} />
  );
}
