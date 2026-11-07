import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createFixtureClient } from "@/features/inbox/fixtures";
import type { InboxClient } from "@/features/inbox/protocol";
import { createReviewBook, type ReviewBook } from "@/features/inbox/review";
import { ReviewDiff } from "@/features/inbox/ReviewDiff";
import type { ChangedFile, DiffLine } from "@/features/workspace/types";

const PR = "github:monocode/monocode#1284";
const ADDED: DiffLine = { kind: "add", text: "  let cap = 3;", oldNumber: null, newNumber: 42 };
const REMOVED: DiffLine = { kind: "del", text: "  let cap = 9;", oldNumber: 40, newNumber: null };
const HUNK: DiffLine = { kind: "hunk", text: "@@ -40 +42 @@", oldNumber: null, newNumber: null };
const FILE = {
  relative: "src/net/retry.rs",
  status: "modified",
  additions: 1,
  deletions: 1,
  lines: [HUNK, REMOVED, ADDED],
} as ChangedFile;

const seen = vi.hoisted(() => ({
  review: null as unknown,
  alert: vi.fn<(title: string, detail?: string) => void>(),
}));

vi.mock("react-native", () => ({
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Text: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  Pressable: ({
    children,
    onPress,
    disabled,
    accessibilityLabel,
  }: {
    children?: ReactNode;
    onPress?: () => void;
    disabled?: boolean;
    accessibilityLabel?: string;
  }) => (
    <button aria-label={accessibilityLabel} disabled={disabled} onClick={onPress}>
      {children}
    </button>
  ),
  TextInput: ({
    value,
    onChangeText,
  }: {
    value: string;
    onChangeText: (text: string) => void;
  }) => <textarea value={value} onChange={(event) => onChangeText(event.target.value)} />,
  Modal: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  KeyboardAvoidingView: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  ActivityIndicator: () => <span>busy</span>,
  Alert: { alert: seen.alert },
  Platform: { OS: "ios" },
}));
vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ top: 0, bottom: 0, left: 0, right: 0 }),
}));
vi.mock("@/ui/theme", () => ({
  themedStyles: () => () => ({}),
  useColors: () => ({}),
  useScheme: () => "dark",
}));
vi.mock("@/features/changes/diffTheme", () => ({ MONO: "mono" }));
vi.mock("@/features/inbox/InboxProvider", () => ({ useInbox: () => ({ review: seen.review }) }));
// The real diff is native list rows; its contract here is onLine and isMarked.
vi.mock("@/features/changes/DiffScreen", () => ({
  DiffScreen: ({
    file,
    onLine,
    isMarked,
  }: {
    file: ChangedFile;
    onLine: (line: DiffLine) => void;
    isMarked: (line: DiffLine) => boolean;
  }) => (
    <div>
      {file.lines?.map((line) => (
        <button key={line.text} aria-label={`line ${line.text}`} onClick={() => onLine(line)}>
          {isMarked(line) ? "marked" : ""}
        </button>
      ))}
    </div>
  ),
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

let root: Root | null = null;
let host: HTMLElement;
let review: ReviewBook;

function fixture() {
  return createFixtureClient({ projectIds: ["p1"], now: () => 0, controls: { latencyMs: 0 } });
}

async function open(client: InboxClient) {
  review = createReviewBook(client);
  seen.review = review;
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ReviewDiff file={FILE} itemKey={PR} />));
}

const button = (label: string) => host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);

async function press(label: string) {
  const target = button(label);
  if (!target) throw new Error(`No button "${label}"`);
  await act(async () => {
    target.click();
    await new Promise((resolve) => setTimeout(resolve, 5));
  });
}

async function write(text: string) {
  const input = host.querySelector("textarea");
  if (!input) throw new Error("No composer");
  const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  await act(async () => {
    set?.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => seen.alert.mockClear());

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host.remove();
});

describe("a pull request's diff", () => {
  it("opens the composer on the tapped line, showing which", async () => {
    await open(fixture());
    expect(host.querySelector("textarea")).toBeNull();
    await press(`line ${ADDED.text}`);
    expect(host.textContent).toContain("retry.rs:42");
    expect(host.textContent).toContain("let cap = 3;");
  });

  it("opens nothing for a hunk header", async () => {
    await open(fixture());
    await press(`line ${HUNK.text}`);
    expect(host.querySelector("textarea")).toBeNull();
  });

  it("adds nothing blank", async () => {
    await open(fixture());
    await press(`line ${ADDED.text}`);
    expect(button("Add to review")?.disabled).toBe(true);
    await write("   ");
    expect(button("Add to review")?.disabled).toBe(true);
  });

  it("adds the comment to the line's side, marks it and closes", async () => {
    await open(fixture());
    await press(`line ${REMOVED.text}`);
    await write("  Why nine?  ");
    await press("Add to review");
    expect(review.comments(PR)).toMatchObject([
      { path: "src/net/retry.rs", line: 40, side: "left", text: "Why nine?" },
    ]);
    expect(host.querySelector("textarea")).toBeNull();
    expect(button(`line ${REMOVED.text}`)?.textContent).toBe("marked");
    expect(button(`line ${ADDED.text}`)?.textContent).toBe("");
    expect(host.textContent).toContain("1 comment pending");
  });

  it("keeps what was typed when the desktop refuses", async () => {
    await open({
      ...fixture(),
      reviewAdd: () => Promise.reject(new Error("The desktop is offline")),
    });
    await press(`line ${ADDED.text}`);
    await write("Cap this");
    await press("Add to review");
    expect(seen.alert).toHaveBeenCalledWith("Couldn’t add the comment", "The desktop is offline");
    expect(host.querySelector("textarea")?.value).toBe("Cap this");
    expect(button("Add to review")?.disabled).toBe(false);
  });
});
