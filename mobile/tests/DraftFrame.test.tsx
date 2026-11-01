import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const world = vi.hoisted(() => ({
  focused: true,
  appState: "active",
  listeners: new Set<(state: string) => void>(),
  command: vi.fn(),
  imageRenders: 0,
}));
vi.mock("react-native", () => ({
  AppState: {
    get currentState() {
      return world.appState;
    },
    addEventListener: (_: string, listener: (state: string) => void) => {
      world.listeners.add(listener);
      return { remove: () => world.listeners.delete(listener) };
    },
  },
  Image: ({ source }: { source: { uri: string } }) => {
    world.imageRenders++;
    return <img data-length={source.uri.length} />;
  },
  ScrollView: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Text: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  useWindowDimensions: () => ({ width: 430, scale: 3 }),
}));
vi.mock("expo-router", () => ({ useIsFocused: () => world.focused }));
vi.mock("@/features/connection/ConnectionProvider", () => ({ useCommand: () => world.command }));
vi.mock("@/ui/theme", () => ({
  themedStyles: () => () => ({}),
  useColors: () => ({ faint: "#555" }),
}));

import { DraftFrame, frameSignature } from "@/features/drafts/DraftFrame";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const picture = (seed: string) => ({
  frame: `data:image/jpeg;base64,${seed.repeat(4000)}`,
  width: 750,
  height: 1000,
});
let root: ReturnType<typeof createRoot>;
const run = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));
/** One second per act, so each state update is flushed on its own like a real frame. */
const seconds = async (count: number) => {
  for (let i = 0; i < count; i++) await run(1000);
};
const mount = () =>
  act(async () => root.render(<DraftFrame conversationId="c" draft="d" url="http://localhost:3000" />));

beforeEach(() => {
  vi.useFakeTimers();
  world.focused = true;
  world.appState = "active";
  world.listeners.clear();
  world.command.mockReset();
  world.imageRenders = 0;
  root = createRoot(document.createElement("div"));
});
afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
});

describe("DraftFrame", () => {
  it("asks for the phone's full width in pixels, as before", async () => {
    world.command.mockResolvedValue(picture("a"));
    await mount();
    // 430 points at scale 3.
    expect(world.command.mock.calls[0][1]).toMatchObject({ width: 1290 });
  });

  it("asks once a second while the page moves and draws every new picture", async () => {
    let n = 0;
    world.command.mockImplementation(async () => picture(String.fromCharCode(97 + (n++ % 20))));
    await mount();
    await seconds(10);
    expect(world.command.mock.calls.length).toBeGreaterThanOrEqual(10);
    // Each different picture is drawn: one render each.
    expect(world.imageRenders).toBe(world.command.mock.calls.length);
  });

  it("keeps asking once a second when the page is still, but never draws the same picture again", async () => {
    world.command.mockResolvedValue(picture("z"));
    await mount();
    await seconds(60);
    // The same cadence as when it moves: no slowdown that could delay a change.
    expect(world.command.mock.calls.length).toBeGreaterThanOrEqual(60);
    // The one picture was drawn once. Before, it was drawn again with every ask.
    expect(world.imageRenders).toBe(1);
  });

  it("shows a change the very next second after a long still stretch", async () => {
    world.command.mockResolvedValue(picture("z"));
    await mount();
    await seconds(30);
    expect(world.imageRenders).toBe(1);
    world.command.mockResolvedValue(picture("y"));
    await seconds(1);
    expect(world.imageRenders).toBe(2);
  });

  it("draws a change the samples miss once the full comparison runs", async () => {
    const still = picture("q");
    // A byte between two samples, so the signatures match.
    const changed = { ...still, frame: still.frame.slice(0, 200) + "X" + still.frame.slice(201) };
    expect(frameSignature(changed.frame)).toBe(frameSignature(still.frame));
    world.command.mockResolvedValue(still);
    await mount();
    expect(world.imageRenders).toBe(1);
    world.command.mockResolvedValue(changed);
    await seconds(10);
    expect(world.imageRenders).toBe(2);
  });

  it("does not ask while another screen covers it or the app is in the background", async () => {
    world.focused = false;
    world.command.mockResolvedValue(picture("a"));
    await mount();
    await run(30_000);
    expect(world.command).not.toHaveBeenCalled();
  });

  it("waits for the app to be shown rather than checking each second", async () => {
    world.appState = "background";
    world.command.mockResolvedValue(picture("a"));
    await mount();
    await run(60_000);
    expect(world.command).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
    world.appState = "active";
    await act(async () => {
      for (const listener of [...world.listeners]) listener("active");
    });
    await run(0);
    expect(world.command).toHaveBeenCalledTimes(1);
  });

  it("backs off after failures", async () => {
    world.command.mockRejectedValue(new Error("offline"));
    await mount();
    await run(300_000);
    // A fixed 3 s retry would be 100 asks.
    expect(world.command.mock.calls.length).toBeLessThan(20);
  });
});

describe("frameSignature", () => {
  it("notices a different length or sampled bytes, and stays small for a large picture", () => {
    const base = `data:image/jpeg;base64,${"abcd".repeat(100000)}`;
    expect(frameSignature(base)).toBe(frameSignature(base));
    expect(frameSignature(base)).not.toBe(frameSignature(base + "x"));
    // The first sample sits at the very start of the data.
    expect(frameSignature(base)).not.toBe(frameSignature("X" + base.slice(1)));
    expect(frameSignature(base).length).toBeLessThan(1000);
  });
});
