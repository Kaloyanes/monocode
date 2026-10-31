import * as Haptics from "expo-haptics";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, Alert, AppState, Pressable, Text, View } from "react-native";
import { WebView, type WebViewMessageEvent } from "react-native-webview";
import type { ShouldStartLoadRequest } from "react-native-webview/lib/WebViewTypes";

import { useConnection } from "@/features/connection/ConnectionProvider";
import { makeCommandId } from "@/features/connection/nativeLink";
import { radii, themedStyles, useColors, useScheme, withAlpha } from "@/ui/theme";

import { DraftFrame } from "./DraftFrame";
import {
  follow,
  freshSync,
  newer,
  parseMessage,
  reportState,
  sendScript,
  trusted,
  type DraftAction,
  type DraftRecord,
  type PageCommand,
} from "./sync";

/** After a failed read, the phone asks again this much later. */
const RETRY_MS = 2000;
/** Taps and keys are sent together, at most this often. */
const TRAIL_MS = 600;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const reason = (value: unknown) => (value instanceof Error ? value.message : String(value));

type Press = { action: "send" } | { action: "handBack" };

/**
 * A draft from the conversation, live with the desktop's card: the phone draws
 * its own copy of the page, at its own width, and shares the page's state, the
 * person's notes and who is driving with the desktop and the agent.
 */
export function DraftScreen({
  conversationId,
  draft,
}: {
  conversationId: string;
  draft: string;
}) {
  const { command } = useConnection();
  const light = useScheme() === "light";
  const colors = useColors();
  const styles = useStyles();
  const [record, setRecord] = useState<DraftRecord | null>(null);
  const [page, setPage] = useState<{ html: string; generation: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [commenting, setCommenting] = useState(false);
  const web = useRef<WebView>(null);
  const current = useRef<DraftRecord | null>(null);
  const sync = useRef(freshSync());
  /** The generation of the page the web view has finished loading. */
  const loaded = useRef<number | null>(null);
  /** The generation whose page the phone holds, so the desktop leaves it out. */
  const held = useRef<number | null>(null);
  const touchedAt = useRef(-Infinity);
  const commentingRef = useRef(false);
  const trail = useRef<string[]>([]);
  const trailTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const send = useCallback((message: PageCommand) => {
    web.current?.injectJavaScript(sendScript(message));
  }, []);

  const apply = useCallback(
    (next: DraftRecord) => {
      const merged = newer(current.current, next);
      current.current = merged;
      setRecord(merged);
      if (!merged.live) return;
      if (merged.page && held.current !== merged.generation) {
        held.current = merged.generation;
        loaded.current = null;
        setPage({ html: merged.page, generation: merged.generation });
        return;
      }
      if (loaded.current === merged.generation)
        for (const message of follow(sync.current, merged, Date.now())) send(message);
    },
    [send],
  );

  const act = useCallback(
    async (action: DraftAction | Press) => {
      const state = action.action === "state";
      if (state) sync.current.sending += 1;
      try {
        const reply = await command<DraftRecord>(
          "draft.action",
          { conversationId, draft, light, generation: held.current ?? undefined, ...action },
          makeCommandId(),
        );
        if (state) sync.current.sending -= 1;
        apply(reply);
      } catch (value) {
        if (state) sync.current.sending -= 1;
        // A tap or a moved control that did not arrive is redone by hand;
        // notes and the buttons say so.
        if (action.action !== "input" && action.action !== "state")
          Alert.alert("Could not reach the desktop", reason(value));
      }
    },
    [apply, command, conversationId, draft, light],
  );

  // Read at once, then follow the record: each read waits on the desktop
  // until the draft changes.
  useEffect(() => {
    let alive = true;
    held.current = null;
    void (async () => {
      while (alive) {
        if (AppState.currentState !== "active") {
          await sleep(1000);
          continue;
        }
        const known = current.current?.live ? current.current : null;
        try {
          const reply = await command<DraftRecord>("draft.read", {
            conversationId,
            draft,
            light,
            since: known && held.current != null ? known.rev : undefined,
            generation: held.current ?? undefined,
          });
          if (!alive) return;
          setError(null);
          apply(reply);
          if (!reply.live) return;
        } catch (value) {
          if (!alive) return;
          setError(reason(value));
          await sleep(RETRY_MS);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [apply, command, conversationId, draft, light]);

  useEffect(
    () => () => {
      if (trailTimer.current) clearTimeout(trailTimer.current);
    },
    [],
  );

  const setComment = (on: boolean) => {
    commentingRef.current = on;
    setCommenting(on);
    send({ __mcComment: on });
  };

  const onMessage = (event: WebViewMessageEvent) => {
    const message = parseMessage(event.nativeEvent.data);
    const live = current.current;
    if (!message || !live?.live || loaded.current !== live.generation) return;
    const now = Date.now();
    // The page's own script can post anything; only what follows the
    // person's touch speaks for them.
    if (!trusted(message, touchedAt.current, commentingRef.current, now)) return;
    switch (message.k) {
      case "v": {
        const { page: commands, desktop } = reportState(
          sync.current,
          live,
          message.v,
          touchedAt.current,
          now,
        );
        commands.forEach(send);
        if (desktop) void act(desktop);
        break;
      }
      case "f":
        void act({ action: "pin", notes: [message.f] });
        break;
      case "u":
        if (message.a) trail.current.push(message.a);
        trailTimer.current ??= setTimeout(() => {
          trailTimer.current = null;
          void act({ action: "input", actions: trail.current.splice(0) });
        }, TRAIL_MS);
        break;
      case "n":
        void act({ action: "message", text: message.t });
        break;
    }
  };

  // The page and its sandboxed frame load from memory; nothing navigates.
  const allow = (request: ShouldStartLoadRequest) => /^about:(blank|srcdoc)/.test(request.url);

  const touched = () => {
    touchedAt.current = Date.now();
  };

  if (!record)
    return (
      <View style={styles.center}>
        {error ? (
          <Text style={styles.muted}>{error}</Text>
        ) : (
          <ActivityIndicator color={colors.muted} />
        )}
      </View>
    );
  if (!record.live)
    return (
      <View style={styles.center}>
        <Text style={styles.muted}>
          This draft was closed. Ask the agent to open it again.
        </Text>
      </View>
    );
  // The phone cannot reach the desktop's localhost; it shows pictures of it.
  if (record.kind === "url")
    return <DraftFrame conversationId={conversationId} draft={draft} url={record.url} />;

  const notes = record.pending.length;
  const status = record.waiting
    ? "Waiting for you"
    : commenting
      ? "Tap the page to leave a note"
      : record.control
        ? "You're using the draft"
        : null;
  return (
    <View style={styles.screen}>
      <View style={styles.bar}>
        <View style={styles.status}>
          {record.waiting ? <View style={[styles.dot, { backgroundColor: colors.accent }]} /> : null}
          {status ? (
            <Text style={styles.statusText} numberOfLines={1}>
              {status}
            </Text>
          ) : null}
        </View>
        <Button
          label="Comment"
          selected={commenting}
          onPress={() => setComment(!commenting)}
          hint="Leave a note on the page"
        />
        {notes ? (
          <Button
            label={`Send ${notes} ${notes === 1 ? "note" : "notes"}`}
            primary
            hint="Send your notes to the agent"
            onPress={() => {
              void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
              if (commentingRef.current) setComment(false);
              void act({ action: "send" });
            }}
          />
        ) : null}
        {record.control || record.waiting ? (
          <Button
            label="Hand back"
            hint="Hand the draft back to the agent"
            onPress={() => {
              if (commentingRef.current) setComment(false);
              void act({ action: "handBack" });
            }}
          />
        ) : null}
      </View>
      <View style={styles.page} onTouchStart={touched} onTouchMove={touched}>
        {page ? (
          <WebView
            key={page.generation}
            ref={web}
            source={{ html: page.html }}
            originWhitelist={["about:*"]}
            onShouldStartLoadWithRequest={allow}
            onLoadStart={() => {
              loaded.current = null;
              sync.current = freshSync();
            }}
            onLoadEnd={() => {
              loaded.current = page.generation;
              if (commentingRef.current) send({ __mcComment: true });
              const live = current.current;
              if (live?.live && live.generation === page.generation)
                for (const message of follow(sync.current, live, Date.now())) send(message);
            }}
            onContentProcessDidTerminate={() => web.current?.reload()}
            onMessage={onMessage}
            setSupportMultipleWindows={false}
            javaScriptCanOpenWindowsAutomatically={false}
            allowsLinkPreview={false}
            allowFileAccess={false}
            dataDetectorTypes="none"
            incognito
            style={styles.web}
          />
        ) : (
          <ActivityIndicator style={styles.center} color={colors.muted} />
        )}
      </View>
    </View>
  );
}

function Button({
  label,
  hint,
  onPress,
  selected = false,
  primary = false,
}: {
  label: string;
  hint: string;
  onPress: () => void;
  selected?: boolean;
  primary?: boolean;
}) {
  const styles = useStyles();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={hint}
      accessibilityState={{ selected }}
      hitSlop={{ top: 6, bottom: 6 }}
      onPress={onPress}
      style={({ pressed }) => [
        styles.button,
        selected && styles.selected,
        primary && styles.primary,
        pressed && styles.pressed,
      ]}
    >
      <Text style={[styles.buttonText, primary && styles.primaryText]}>{label}</Text>
    </Pressable>
  );
}

const useStyles = themedStyles((colors) => ({
  screen: {
    flex: 1,
    backgroundColor: colors.sheet,
  },
  center: {
    flex: 1,
    padding: 32,
    alignItems: "center",
    justifyContent: "center",
  },
  muted: {
    color: colors.muted,
    fontSize: 15,
    textAlign: "center",
  },
  bar: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    paddingHorizontal: 16,
    paddingVertical: 8,
  },
  status: {
    flex: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  statusText: {
    flexShrink: 1,
    color: colors.muted,
    fontSize: 13,
  },
  button: {
    height: 32,
    paddingHorizontal: 12,
    justifyContent: "center",
    borderRadius: radii.pill,
    backgroundColor: colors.fill,
  },
  selected: {
    backgroundColor: withAlpha(colors.content, 0.16),
  },
  primary: {
    backgroundColor: colors.accent,
  },
  pressed: {
    opacity: 0.6,
  },
  buttonText: {
    color: colors.content,
    fontSize: 13,
    fontWeight: "600",
  },
  primaryText: {
    color: "#ffffff",
  },
  page: {
    flex: 1,
  },
  web: {
    flex: 1,
    backgroundColor: "transparent",
  },
}));
