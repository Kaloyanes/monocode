import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { ScenePagerHost, useDrawerActions, useSceneSwitching } from "@/ui/ScenePager";

const app = vi.hoisted(() => ({
  pathname: "/project/one",
  withSpring: vi.fn((to: unknown) => to),
}));

vi.mock("react-native", () => {
  const View = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
  return {
    BackHandler: { addEventListener: () => ({ remove: () => {} }) },
    Keyboard: { dismiss: () => {} },
    Pressable: View,
    StyleSheet: { absoluteFill: {}, absoluteFillObject: {}, create: (styles: unknown) => styles },
    View,
  };
});
vi.mock("expo-haptics", () => ({
  impactAsync: async () => {},
  ImpactFeedbackStyle: { Light: "light" },
}));
vi.mock("expo-linear-gradient", () => ({ LinearGradient: () => null }));
vi.mock("expo-router", () => ({
  useIsFocused: () => true,
  usePathname: () => app.pathname,
}));
vi.mock("react-native-gesture-handler", () => {
  const chain: Record<string, () => unknown> = new Proxy({}, { get: () => () => chain });
  return {
    Gesture: { Pan: () => chain, Tap: () => chain, Native: () => chain },
    GestureDetector: ({ children }: { children: ReactNode }) => children,
  };
});
vi.mock("react-native-reanimated", async () => {
  const { useRef } = await import("react");
  const { View } = await import("react-native");
  return {
    default: { View },
    cancelAnimation: () => {},
    interpolate: () => 0,
    ReduceMotion: { Never: "never" },
    useAnimatedReaction: () => {},
    useAnimatedStyle: () => ({}),
    // At rest, so a switch takes the drawer-already-shut path.
    useSharedValue: (initial: unknown) => {
      const value = useRef(initial);
      return useRef({ get: () => value.current, set: (next: unknown) => (value.current = next) })
        .current;
    },
    withSpring: app.withSpring,
  };
});
vi.mock("react-native-worklets", () => ({ scheduleOnRN: (run: () => void) => run() }));
vi.mock("@/ui/ContentPane", () => ({ useContentWidth: () => 393 }));
vi.mock("@/ui/theme", () => ({
  useColors: () => ({ background: "#000" }),
  useScheme: () => "dark",
}));

let root: Root;
let host: HTMLElement;
let actions: ReturnType<typeof useDrawerActions>;
let covering = false;

function Probe() {
  actions = useDrawerActions();
  covering = useSceneSwitching();
  return null;
}

function render() {
  act(() =>
    root.render(
      <ScenePagerHost>
        <Probe />
      </ScenePagerHost>,
    ),
  );
}

/** Lets the frames and the arrival's cover timing pass. */
function frames(count = 4) {
  act(() => {
    vi.advanceTimersByTime(count * 16);
  });
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers({ toFake: ["setTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
  app.pathname = "/project/one";
  host = document.createElement("div");
  root = createRoot(host);
  render();
});

afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
});

it("covers the screen while a switch loads, until the next one arrives", () => {
  const change = vi.fn();
  act(() => actions.closeThen(change));
  expect(covering).toBe(true);
  frames();
  expect(change).toHaveBeenCalledOnce();
  expect(covering).toBe(true);
  app.pathname = "/project/two";
  render();
  frames();
  expect(covering).toBe(false);
});

it("pushes behind the drawer at once, without ever covering the screen", () => {
  const change = vi.fn();
  act(() => actions.closeBehind(change));
  expect(change).toHaveBeenCalledOnce();
  expect(covering).toBe(false);
  frames();
  expect(change).toHaveBeenCalledOnce();
  expect(covering).toBe(false);
});

it("holds the drawer until the screen behind it starts", () => {
  act(() => actions.open());
  app.withSpring.mockClear();
  act(() => actions.closeBehind(vi.fn()));
  frames();
  expect(app.withSpring).not.toHaveBeenCalled();
  act(() => actions.releaseDrawer());
  expect(app.withSpring).toHaveBeenCalledOnce();
  expect(app.withSpring.mock.calls[0][0]).toBe(0);
  // The screen starts again each time it is returned to.
  act(() => actions.releaseDrawer());
  expect(app.withSpring).toHaveBeenCalledOnce();
});

it("leaves a drawer that isn't held alone", () => {
  act(() => actions.open());
  app.withSpring.mockClear();
  act(() => actions.releaseDrawer());
  expect(app.withSpring).not.toHaveBeenCalled();
});

it("shuts the drawer anyway when the screen never starts", () => {
  act(() => actions.open());
  app.withSpring.mockClear();
  act(() => actions.closeBehind(vi.fn()));
  act(() => {
    vi.advanceTimersByTime(999);
  });
  expect(app.withSpring).not.toHaveBeenCalled();
  act(() => {
    vi.advanceTimersByTime(1);
  });
  expect(app.withSpring).toHaveBeenCalledOnce();
});

it("brings the cover down when a push behind the drawer replaces a switch", () => {
  const first = vi.fn();
  const second = vi.fn();
  act(() => {
    actions.closeThen(first);
    actions.closeBehind(second);
  });
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledOnce();
  expect(covering).toBe(true);
  frames();
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledOnce();
  app.pathname = "/inbox-list";
  render();
  frames();
  expect(covering).toBe(false);
});
