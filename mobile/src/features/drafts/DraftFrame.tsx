import { useEffect, useRef, useState } from "react";
import { AppState, Image, ScrollView, Text, useWindowDimensions, View } from "react-native";

import { useConnection } from "@/features/connection/ConnectionProvider";
import { themedStyles, useColors } from "@/ui/theme";

/** How often the phone asks for a new picture while it shows one. */
const FRAME_MS = 1000;
/** After a failed ask, the phone tries again this much later. */
const RETRY_MS = 3000;

type Frame = { frame: string; width: number; height: number };

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const reason = (value: unknown) => (value instanceof Error ? value.message : String(value));

/** The address without its scheme, as the person typed it. */
export const shortUrl = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/$/, "");

/**
 * A dev-server draft, which the phone cannot load from the desktop's
 * localhost: pictures of the desktop's copy, about once a second. Nothing on
 * it can be touched; the person uses the page on the desktop.
 */
export function DraftFrame({
  conversationId,
  draft,
  url,
}: {
  conversationId: string;
  draft: string;
  url: string | undefined;
}) {
  const { command } = useConnection();
  const colors = useColors();
  const styles = useStyles();
  const { width, scale } = useWindowDimensions();
  // Two pictures take turns: the next one loads hidden in the other slot and
  // is shown once it is drawn, so the page never blinks between frames and
  // the picture on screen never changes under the person.
  const [slots, setSlots] = useState<[Frame | null, Frame | null]>([null, null]);
  const [front, setFront] = useState<0 | 1 | null>(null);
  const frontRef = useRef<0 | 1 | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pixels = Math.round(width * scale);

  useEffect(() => {
    let alive = true;
    void (async () => {
      while (alive) {
        if (AppState.currentState !== "active") {
          await sleep(FRAME_MS);
          continue;
        }
        try {
          const reply = await command<Frame | { live: false }>("draft.frame", {
            conversationId,
            draft,
            width: pixels,
          });
          if (!alive) return;
          if (!("frame" in reply)) return setError("This draft was closed.");
          setError(null);
          setSlots((current) => {
            const back = frontRef.current === 0 ? 1 : 0;
            const next: [Frame | null, Frame | null] = [...current];
            next[back] = reply;
            return next;
          });
          await sleep(FRAME_MS);
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
  }, [command, conversationId, draft, pixels]);

  const shown = front == null ? null : slots[front];
  const status = error ?? (url ? `Live from your desktop · ${shortUrl(url)}` : "Live from your desktop");
  return (
    <View style={styles.screen}>
      <View style={styles.bar}>
        <Text style={styles.status} numberOfLines={1}>
          {status}
        </Text>
      </View>
      <ScrollView contentContainerStyle={styles.page}>
        <View style={shown ? { aspectRatio: shown.width / shown.height } : styles.empty}>
          {shown ? null : (
            <Text style={[styles.status, { color: colors.faint }]}>Waiting for the desktop…</Text>
          )}
          {slots.map((slot, index) =>
            slot ? (
              <Image
                key={index}
                source={{ uri: slot.frame }}
                style={[styles.picture, index !== front && styles.hidden]}
                fadeDuration={0}
                accessible={index === front}
                accessibilityLabel={`The draft as the desktop shows it${url ? `, ${shortUrl(url)}` : ""}`}
                onLoad={() => {
                  if (index === frontRef.current) return;
                  frontRef.current = index as 0 | 1;
                  setFront(index as 0 | 1);
                }}
              />
            ) : null,
          )}
        </View>
      </ScrollView>
    </View>
  );
}

const useStyles = themedStyles((colors) => ({
  screen: {
    flex: 1,
    backgroundColor: colors.sheet,
  },
  bar: {
    paddingHorizontal: 16,
    paddingVertical: 8,
    minHeight: 48,
    justifyContent: "center",
  },
  status: {
    color: colors.muted,
    fontSize: 13,
  },
  page: {
    paddingBottom: 32,
  },
  empty: {
    height: 240,
    alignItems: "center",
    justifyContent: "center",
  },
  picture: {
    position: "absolute",
    top: 0,
    left: 0,
    right: 0,
    bottom: 0,
    resizeMode: "contain",
  },
  hidden: {
    opacity: 0,
  },
}));
