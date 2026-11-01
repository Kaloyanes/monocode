import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const world = vi.hoisted(() => ({
  focused: true,
  appState: "active",
  listeners: new Set<(state: string) => void>(),
  command: vi.fn(),
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
  Image: ({ source }: { source: { uri: string } }) => <img data-length={source.uri.length} />,
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
const mount = () =>
  act(async () => root.render(<DraftFrame conversationId="c" draft="d" url="http://localhost:3000" />));

beforeEach(() => {
  vi.useFakeTimers();
  world.focused = true;
  world.appState = "active";
  world.listeners.clear();
  world.command.mockReset();
  root = createRoot(document.createElement("div"));
});
afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
});

describe("DraftFrame", () => {
  it("asks for at most 750 px, not the whole retina width", async () => {
    world.command.mockResolvedValue(picture("a"));
    await mount();
    expect(world.command.mock.calls[0][1]).toMatchObject({ width: 750 });
  });

  it("asks once a second while the page moves and every two seconds once it is still", async () => {
    let n = 0;
    world.command.mockImplementation(async () => picture(String.fromCharCode(97 + (n++ % 20))));
    await mount();
    await run(10_000);
    const moving = world.command.mock.calls.length;
    expect(moving).toBeGreaterThanOrEqual(10);

    world.command.mockReset();
    world.command.mockResolvedValue(picture("z"));
    await run(60_000);
    // First one differs, the rest are the same: about every 2 s, not every second.
    expect(world.command.mock.calls.length).toBeLessThanOrEqual(32);
    expect(world.command.mock.calls.length).toBeGreaterThanOrEqual(25);
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
