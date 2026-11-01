import { useIsFocused } from "expo-router";
import { useEffect, useRef, useState } from "react";
import { Image, ScrollView, Text, useWindowDimensions, View } from "react-native";

import { backoff, pause, waitForActive } from "@/features/connection/appActivity";
import { useCommand } from "@/features/connection/ConnectionProvider";
import { themedStyles, useColors } from "@/ui/theme";

/** How often the phone asks for a new picture while it shows one. */
const FRAME_MS = 1000;
/** While the last two pictures were the same, it asks this seldom. */
const STILL_MS = 2000;
/** The widest picture asked for; a phone screen shows no more than this well. */
const MAX_PIXELS = 750;
/** A still picture is drawn again after this many, in case the sample missed a change. */
const REFRESH_EVERY = 5;
/** After a failed ask, the phone tries again this much later, doubling up to this. */
const RETRY_MS = 3000;
const RETRY_MAX_MS = 30000;

type Frame = { frame: string; width: number; height: number };

/**
 * A cheap stand-in for comparing two pictures: the length and a few dozen
 * small samples across the data. A change in a JPEG shifts the bytes after it,
 * so a sample nearly always shows it; the screen also draws a still picture
 * again now and then.
 */
export function frameSignature(frame: string): string {
  const samples = 48;
  const size = 16;
  let signature = String(frame.length);
  for (let i = 0; i < samples; i++) {
    const at = Math.floor(((frame.length - size) * i) / (samples - 1));
    signature += frame.slice(Math.max(0, at), Math.max(0, at) + size);
  }
  return signature;
}
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
  const command = useCommand();
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
  const pixels = Math.min(MAX_PIXELS, Math.round(width * scale));
  // Pictures of a page nobody sees (another screen on top) are not asked for.
  const focused = useIsFocused();

  useEffect(() => {
    if (!focused) return;
    let alive = true;
    const stop = new AbortController();
    void (async () => {
      let failures = 0;
      let last: string | null = null;
      // Pictures in a row that matched the one before.
      let still = 0;
      while (alive) {
        await waitForActive(stop.signal);
        if (!alive) return;
        try {
          const reply = await command<Frame | { live: false }>("draft.frame", {
            conversationId,
            draft,
            width: pixels,
          });
          if (!alive) return;
          if (!("frame" in reply)) return setError("This draft was closed.");
          failures = 0;
          setError(null);
          const signature = frameSignature(reply.frame);
          still = signature === last ? still + 1 : 0;
          last = signature;
          // The same picture again isn't decoded or drawn, bar a refresh now and then.
          if (still === 0 || still % REFRESH_EVERY === 0)
            setSlots((current) => {
              const back = frontRef.current === 0 ? 1 : 0;
              const next: [Frame | null, Frame | null] = [...current];
              next[back] = reply;
              return next;
            });
          await pause(still >= 1 ? STILL_MS : FRAME_MS, stop.signal);
        } catch (value) {
          if (!alive) return;
          setError(reason(value));
          await pause(backoff(RETRY_MS, failures++, RETRY_MAX_MS), stop.signal);
        }
      }
    })();
    return () => {
      alive = false;
      stop.abort();
    };
  }, [command, conversationId, draft, pixels, focused]);

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
