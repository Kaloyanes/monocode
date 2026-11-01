import type { ReactNode } from "react";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { GradualBlur, layerTint } from "@/ui/GradualBlur";

const counts = vi.hoisted(() => ({ masks: 0, blurs: 0 }));
vi.mock("@react-native-masked-view/masked-view", () => ({
  default: ({ children }: { children?: ReactNode }) => {
    counts.masks++;
    return <div>{children}</div>;
  },
}));
vi.mock("expo-blur", () => ({
  BlurView: () => {
    counts.blurs++;
    return null;
  },
}));
vi.mock("expo-linear-gradient", () => ({ LinearGradient: () => null }));
vi.mock("react-native", () => ({
  StyleSheet: { absoluteFill: {}, create: <T,>(styles: T) => styles },
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));
vi.mock("@/ui/theme", () => ({
  useScheme: () => "dark",
  withAlpha: (color: string, alpha: number) => `${color}:${alpha}`,
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  counts.masks = 0;
  counts.blurs = 0;
  root = createRoot(document.createElement("div"));
});
afterEach(() => act(() => root.unmount()));

it("stacks two live blur layers by default, down from six", async () => {
  await act(async () =>
    root.render(<GradualBlur edge="bottom" height={100} color="#000000" />),
  );
  expect(counts.masks).toBe(2);
  expect(counts.blurs).toBe(2);
});

it("tints fewer layers to reach the same edge opacity", () => {
  const edge = (steps: number) => 1 - Math.pow(1 - layerTint(steps), steps);
  expect(layerTint(6)).toBeCloseTo(0.5);
  expect(edge(2)).toBeCloseTo(edge(6));
  expect(edge(1)).toBeCloseTo(edge(6));
});
