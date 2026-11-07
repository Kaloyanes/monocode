import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { marksChanged, syncFor, typed } from "@/features/sessions/composerDraft";
import { SessionCard } from "@/features/sessions/SessionCard";
import type { SessionSummary } from "@/features/workspace/types";

vi.mock("react-native", () => ({
  Pressable: ({
    children,
    accessibilityLabel,
  }: {
    children?: ReactNode;
    accessibilityLabel?: string;
  }) => <button aria-label={accessibilityLabel}>{children}</button>,
  Text: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));
vi.mock("react-native-gesture-handler/ReanimatedSwipeable", () => ({
  default: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));
vi.mock("react-native-reanimated", () => ({
  default: { View: ({ children }: { children?: ReactNode }) => <div>{children}</div> },
}));
vi.mock("@/features/mascots/PixelMascot", () => ({ PixelMascot: () => null }));
vi.mock("@/features/workspace/relativeTime", () => ({ relativeTime: () => "2h" }));
vi.mock("@/ui/Icon", () => ({ Icon: ({ name }: { name: string }) => <i>{name}</i> }));
vi.mock("@/ui/SwipeAction", () => ({
  ACTION_SIZE: 64,
  SwipeAction: () => null,
  SwipeActions: () => null,
}));
vi.mock("@/ui/theme", () => ({
  radii: {},
  withAlpha: (color: string) => color,
  themedStyles: () => () => ({}),
  useColors: () => ({}),
}));
vi.mock("@/features/sessions/ProviderLogo", () => ({ ProviderLogo: () => null }));
vi.mock("@/features/sessions/TerminalSpinner", () => ({ TerminalSpinner: () => null }));

const session = (change: Partial<SessionSummary> = {}) =>
  ({
    id: "s1",
    projectId: "p",
    title: "Fix the login",
    harness: "claude",
    model: "opus",
    branch: "main",
    updatedAt: 1,
    busy: false,
    ...change,
  }) as SessionSummary;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  host = document.createElement("div");
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
});

function show(summary: SessionSummary) {
  act(() => root.render(<SessionCard session={summary} onPress={() => {}} detailed />));
  return host.textContent ?? "";
}

it("shows the time when a thread has no draft", () => {
  const text = show(session({ id: "plain" }));
  expect(text).toContain("2h");
  expect(text).not.toContain("Draft");
});

it("puts the Draft mark where the time goes, with the desktop's dashed icon", () => {
  const text = show(session({ id: "marked", draft: true }));
  expect(text).toContain("Draft");
  expect(host.querySelector("i")?.textContent).toBe("draft");
  expect(text).not.toContain("2h");
  expect(host.querySelector("button")?.getAttribute("aria-label")).toBe("Fix the login, draft");
});

it("lets Working take the place of the mark", () => {
  const text = show(session({ id: "busy", draft: true, busy: true }));
  expect(text).toContain("Working...");
  expect(text).not.toContain("Draft");
});

it("marks a row the moment text is typed here, before the desktop hears of it", () => {
  show(session({ id: "typing" }));
  expect(host.textContent).not.toContain("Draft");
  act(() => {
    typed(syncFor("typing"), "half a thought", Date.now());
    marksChanged();
  });
  expect(host.textContent).toContain("Draft");
});
