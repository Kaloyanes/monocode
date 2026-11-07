import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const app = vi.hoisted(() => ({
  push: vi.fn(),
  archive: vi.fn(async (_id: string) => {}),
  unarchive: vi.fn(async (_id: string) => {}),
  canUnarchive: false,
  archivedTotal: 0,
  alert: vi.fn(),
  newMono: vi.fn(),
  store: new Map<string, string>(),
  workspace: {} as Record<string, unknown>,
}));

vi.mock("expo-sqlite/kv-store", () => ({
  default: {
    getItemSync: (key: string) => app.store.get(key) ?? null,
    setItem: async (key: string, value: string) => void app.store.set(key, value),
  },
}));
vi.mock("expo-router", () => ({ router: { push: app.push } }));
vi.mock("expo-haptics", () => ({
  impactAsync: vi.fn(),
  selectionAsync: vi.fn(),
  notificationAsync: vi.fn(),
  ImpactFeedbackStyle: { Medium: "medium" },
  NotificationFeedbackType: { Success: "success" },
}));
vi.mock("react-native", () => ({
  ActivityIndicator: () => null,
  ActionSheetIOS: { showActionSheetWithOptions: vi.fn() },
  Alert: { alert: app.alert, prompt: vi.fn() },
  StyleSheet: { create: (styles: unknown) => styles, hairlineWidth: 1 },
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Text: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  Pressable: ({ children, onPress, accessibilityLabel }: { children?: ReactNode; onPress?: () => void; accessibilityLabel?: string }) => (
    <button aria-label={accessibilityLabel} onClick={onPress}>{children}</button>
  ),
}));
function animation() {
  const chain: Record<string, () => unknown> = {};
  for (const step of ["duration", "delay", "reduceMotion", "easing"]) chain[step] = () => chain;
  return chain;
}
vi.mock("react-native-reanimated", () => ({
  default: {
    FlatList: ({ data, renderItem, ListHeaderComponent, ListEmptyComponent, ListFooterComponent }: {
      data: unknown[];
      renderItem: (info: { item: unknown }) => ReactNode;
      ListHeaderComponent?: ReactNode;
      ListEmptyComponent?: ReactNode;
      ListFooterComponent?: ReactNode;
    }) => (
      <section>
        {ListHeaderComponent}
        {data.length ? data.map((item, index) => <div key={index}>{renderItem({ item })}</div>) : ListEmptyComponent}
        {ListFooterComponent}
      </section>
    ),
    View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  },
  // Layout animations chain builders (`.duration(…).reduceMotion(…)`).
  FadeIn: animation(),
  FadeOut: animation(),
  FadeInDown: animation(),
  FadeOutDown: animation(),
  ReduceMotion: { System: "system" },
  LinearTransition: animation(),
  Easing: { bezier: () => () => 0, out: () => () => 0, cubic: () => 0 },
  useSharedValue: (value: number) => ({ value, get: () => value, set: () => {} }),
  useAnimatedStyle: () => ({}),
  withTiming: (value: number) => value,
}));
vi.mock("@/features/connection/ConnectionProvider", () => ({
  useConnection: () => ({ pairing: { desktop: { fingerprint: "desk" } } }),
}));
vi.mock("@/features/files/FileSearchBar", () => ({
  SEARCH_BAR: 48,
  FileSearchBar: ({ accessory }: { accessory?: ReactNode }) => <nav>{accessory}</nav>,
}));
vi.mock("@/features/workspace/monoSeen", () => ({
  useUnreadMonos: (_desktop: string, monos: { id: string }[]) =>
    new Set(monos.filter((mono) => mono.id === "unread").map((mono) => mono.id)),
}));
vi.mock("@/features/workspace/useCreateMono", () => ({ useCreateMono: () => app.newMono }));
vi.mock("@/features/workspace/WorkspaceProvider", () => ({ useWorkspace: () => app.workspace }));
vi.mock("@/features/mascots/PixelMascot", () => ({
  PixelMascot: ({ name }: { name: string }) => <i data-mascot={name} />,
}));
vi.mock("@/ui/Icon", () => ({ Icon: ({ name }: { name: string }) => <i data-icon={name} /> }));
vi.mock("@/ui/GradualBlur", () => ({ GradualBlur: () => null }));
vi.mock("@/ui/LiquidView", async () => {
  const { createContext } = await import("react");
  return {
    GlassPresence: createContext(null),
    LiquidView: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    useGlassEntrance: (shown: boolean) => ({ mounted: shown, visible: shown, presence: null }),
    usePopStyle: () => ({}),
  };
});
vi.mock("@/ui/Collapsible", async () => {
  const { use } = await import("react");
  const { AnimationsPaused } = await import("@/ui/AnimationsPaused");
  const Folded = () => <i data-paused={String(use(AnimationsPaused))} />;
  return {
    Collapsible: ({ open, children }: { open: boolean; children: ReactNode }) =>
      open ? <>{children}</> : <Folded />,
  };
});
vi.mock("@/ui/theme", () => ({
  themedStyles: (create: (colors: object) => object) => () => create({}),
  useColors: () => ({ muted: "gray" }),
  useScheme: () => "light",
  withAlpha: () => "black",
  radii: { row: 12 },
}));
vi.mock("@/features/sessions/SessionScreen", () => ({
  useSessionLayout: () => ({ pageTop: 0, dockBottom: 0 }),
}));
vi.mock("@/features/sessions/ArchivedSessions", () => ({
  ArchivedSessions: ({ onBack }: { onBack: () => void }) => (
    <aside data-archive>
      <button aria-label="Back to sessions" onClick={onBack} />
    </aside>
  ),
}));
vi.mock("@/features/sessions/useSessionPages", () => ({
  useSessionPages: (_project: string, _query: string, active: boolean, archived?: boolean) =>
    archived && active ? { sessions: [], hasMore: false, total: app.archivedTotal, loadMore: () => {} } : null,
  visibleSessions: (_pages: unknown, recent: unknown[]) => recent,
}));
vi.mock("@/features/sessions/useArrivals", () => ({
  ARRIVE_MS: 280,
  useArrivals: (sessions: unknown[]) => ({ sessions, arriving: new Set(), moving: false }),
}));
vi.mock("@/features/sessions/SessionFilterMenu", () => ({
  NO_FILTER: { working: false, harness: null },
  filterSessions: (sessions: unknown[]) => sessions,
  SessionFilterMenu: ({ monos }: { monos?: { shown: boolean; onChange: (shown: boolean) => void } }) =>
    monos ? (
      <button aria-label="Monos filter" data-shown={String(monos.shown)} onClick={() => monos.onChange(!monos.shown)} />
    ) : (
      <span data-no-monos-filter />
    ),
}));
vi.mock("@/features/sessions/SessionCard", () => ({
  SessionCard: ({ session, origin, onOpenOrigin, onArchive }: {
    session: { id: string; title: string };
    origin?: { kind: string; mono?: { name: string }; openable?: boolean } | null;
    onOpenOrigin?: () => void;
    onArchive?: () => void;
  }) => (
    <article data-session={session.id}>
      {session.title}
      {origin ? (
        <button aria-label={`chip ${session.id}`} data-tappable={String(!!onOpenOrigin)} onClick={onOpenOrigin}>
          {origin.kind === "removed" ? "Mono removed" : `via ${origin.mono?.name}`}
        </button>
      ) : null}
      <button aria-label={`archive ${session.id}`} onClick={onArchive} />
    </article>
  ),
}));

import { resetMonoGroupPrefs } from "@/features/sessions/monoGroupPrefs";
import { SessionsPage } from "@/features/sessions/SessionsPage";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mono = (id: string, projectIds: string[], extra = {}) => ({
  id, name: id, mascot: `mascot-${id}`, color: "red", projectIds, unread: 0, status: "idle",
  lastMessage: `${id} last line`, lastActiveAt: 0, ...extra,
});
const row = (id: string, extra = {}) => ({
  id, projectId: "p1", title: id, harness: "claude", model: "m", branch: "main", updatedAt: 1, ...extra,
});

let root: ReturnType<typeof createRoot>;
let page: HTMLElement;
function render(projectId = "p1") {
  page = document.createElement("div");
  root = createRoot(page);
  act(() => root.render(<SessionsPage projectId={projectId} active onOpen={vi.fn()} />));
}
const byLabel = (label: string) => page.querySelector(`[aria-label='${label}']`) as HTMLElement;
const click = (element: HTMLElement) => act(() => element.click());

beforeEach(() => {
  vi.clearAllMocks();
  app.store.clear();
  resetMonoGroupPrefs();
  app.canUnarchive = false;
  app.archivedTotal = 0;
  const taskGiver = mono("TaskGiver", ["p1"]);
  const away = mono("Away", ["p9"]);
  app.workspace = {
    projects: [{ id: "p1" }, { id: "p2" }],
    monos: [taskGiver, mono("Both", ["p1", "p2"]), mono("Loose", []), mono("unread", ["p1"])],
    allMonos: [taskGiver, away, mono("Both", ["p1", "p2"]), mono("Loose", []), mono("unread", ["p1"])],
    sessions: [
      row("by-tg", { monoId: "TaskGiver" }),
      row("by-away", { monoId: "Away" }),
      row("by-gone", { monoId: "Deleted" }),
      row("by-hand"),
      row("other-project", { projectId: "p2" }),
    ],
    get polled() {
      return new Set((this.sessions as { id: string }[]).map((session) => session.id));
    },
    edited: (session: unknown) => session,
    canListArchived: false,
    rememberSession: vi.fn(),
    archiveSession: app.archive,
    unarchiveSession: app.unarchive,
    get canUnarchive() { return app.canUnarchive; },
    renameSession: vi.fn(),
    deleteSession: vi.fn(async () => {}),
  };
});
afterEach(() => act(() => root.unmount()));

describe("a project's Sessions page", () => {
  it("pins its Monos above the rows, with the unassigned ones on the first project's page", () => {
    render("p1");
    expect(page.textContent).toContain("Monos");
    const names = Array.from(page.querySelectorAll("[data-mascot]")).map((el) => (el as HTMLElement).dataset.mascot);
    expect(names).toEqual(["mascot-TaskGiver", "mascot-Both", "mascot-unread", "mascot-Loose"].sort((a, b) => names.indexOf(a) - names.indexOf(b)));
    expect(page.textContent).toContain("Unassigned");
    expect(page.textContent).toContain("Loose");
  });

  it("lists a Mono assigned to several projects on each, and leaves out ones with no project there", () => {
    render("p2");
    expect(page.textContent).toContain("Both");
    expect(page.textContent).not.toContain("TaskGiver");
    expect(page.textContent).not.toContain("Unassigned");
  });

  it("opens a Mono's chat from its row", () => {
    render("p1");
    click(byLabel("Unread, unread, unread last line"));
    expect(app.push).toHaveBeenCalledWith({ pathname: "/mono/[monoId]", params: { monoId: "unread" } });
  });

  it("marks unread Monos on their rows", () => {
    render("p1");
    expect(byLabel("Unread, unread, unread last line")).not.toBeNull();
    expect(byLabel("TaskGiver, TaskGiver last line")).not.toBeNull();
  });

  it("folds the group from its header and remembers it", () => {
    render("p1");
    click(byLabel("Monos, 4"));
    expect(page.textContent).not.toContain("TaskGiver last line");
    act(() => root.unmount());
    resetMonoGroupPrefs();
    render("p1");
    expect(page.textContent).not.toContain("TaskGiver last line");
    click(byLabel("Monos, 4"));
    expect(page.textContent).toContain("TaskGiver last line");
  });

  it("says a Mono is working on its own line and leaves the others alone", () => {
    app.workspace.monos = [mono("Busy", ["p1"], { status: "working" }), mono("Calm", ["p1"])];
    render("p1");
    expect(byLabel("Busy, Working…").textContent).toContain("Working…");
    expect(byLabel("Calm, Calm last line").textContent).not.toContain("Working…");
  });

  it("says a Mono is working on the folded header, and only there once folded", () => {
    app.workspace.monos = [mono("Busy", ["p1"], { status: "working" }), mono("Calm", ["p1"])];
    render("p1");
    click(byLabel("Monos, 2"));
    expect(byLabel("Monos, 2, working")).not.toBeNull();
    expect(byLabel("Monos, 2, working").textContent).toContain("Working…");
  });

  it("holds a folded group's mascots still", () => {
    app.workspace.monos = [mono("Busy", ["p1"], { status: "working" })];
    render("p1");
    click(byLabel("Monos, 1"));
    expect(page.querySelector<HTMLElement>("[data-paused]")?.dataset.paused).toBe("true");
  });

  it("keeps a folded header quiet when no Mono is working", () => {
    render("p1");
    click(byLabel("Monos, 4"));
    expect(byLabel("Monos, 4").textContent).not.toContain("Working…");
  });

  it("hides and shows the group from the filter, and remembers it per desktop", () => {
    render("p1");
    expect(byLabel("Monos filter").dataset.shown).toBe("true");
    click(byLabel("Monos filter"));
    expect(page.textContent).not.toContain("TaskGiver last line");
    // Still offered, so it can come back.
    expect(byLabel("Monos filter").dataset.shown).toBe("false");
    expect(app.store.get("monocode.monoGroup.desk")).toContain('"shown":false');
    click(byLabel("Monos filter"));
    expect(page.textContent).toContain("TaskGiver last line");
  });

  it("offers a plus for a new Mono in the group's header", () => {
    render("p1");
    click(byLabel("New Mono"));
    expect(app.newMono).toHaveBeenCalledOnce();
  });

  it("says who started each session, and nothing for one started by hand", () => {
    render("p1");
    expect(byLabel("chip by-tg").textContent).toBe("via TaskGiver");
    expect(byLabel("chip by-tg").dataset.tappable).toBe("true");
    // Its Mono isn't on this phone, so it is named but not opened.
    expect(byLabel("chip by-away").textContent).toBe("via Away");
    expect(byLabel("chip by-away").dataset.tappable).toBe("false");
    expect(byLabel("chip by-gone").textContent).toBe("Mono removed");
    expect(byLabel("chip by-hand")).toBeNull();
    // Another project's session isn't on this page.
    expect(page.querySelector("[data-session='other-project']")).toBeNull();
  });

  it("opens the Mono's chat from a session's chip", () => {
    render("p1");
    click(byLabel("chip by-tg"));
    expect(app.push).toHaveBeenCalledWith({ pathname: "/mono/[monoId]", params: { monoId: "TaskGiver" } });
  });

  it("archives from the swipe with an Undo toast where the desktop can unarchive", async () => {
    app.canUnarchive = true;
    render("p1");
    await act(async () => byLabel("archive by-hand").click());
    expect(app.archive).toHaveBeenCalledWith("by-hand");
    expect(app.alert).not.toHaveBeenCalled();
    expect(page.textContent).toContain("Archived “by-hand”");
    await act(async () => {
      byLabel("Undo").click();
      await new Promise((done) => setTimeout(done, 0));
    });
    expect(app.unarchive).toHaveBeenCalledWith("by-hand");
  });

  it("asks first and shows no Undo where it can't be taken back", async () => {
    render("p1");
    act(() => byLabel("archive by-hand").click());
    expect(app.archive).not.toHaveBeenCalled();
    expect(app.alert.mock.calls[0][0]).toBe("Archive “by-hand”?");
    await act(async () => app.alert.mock.calls[0][2][1].onPress());
    expect(app.archive).toHaveBeenCalledWith("by-hand");
    expect(page.textContent).toContain("Archived “by-hand”");
    expect(byLabel("Undo")).toBeNull();
  });
});

describe("the list", () => {
  it("heads each day's run as the desktop does, below the working ones", () => {
    const today = Date.now();
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);
    app.workspace.sessions = [
      row("busy", { busy: true, updatedAt: today }),
      row("today", { updatedAt: today }),
      row("yesterday", { updatedAt: yesterday.getTime() }),
      row("old", { updatedAt: 1 }),
    ];
    render("p1");
    const shown = Array.from(page.querySelectorAll("[data-session], span"))
      .map((el) => (el as HTMLElement).dataset.session ?? el.textContent)
      .filter((text) => /^(Working|Today|Yesterday|Older|Sessions|old|yesterday|today|busy)$/.test(text ?? ""));
    expect(shown).toEqual(["Working", "busy", "Today", "today", "Yesterday", "yesterday", "Older", "old"]);
  });
});

describe("the sessions working now", () => {
  it("lead the list under their own label and leave the rows below", () => {
    (app.workspace.sessions as unknown[]).push(row("busy-one", { busy: true }), row("busy-two", { busy: true }));
    render("p1");
    const labels = Array.from(page.querySelectorAll("span")).map((el) => el.textContent);
    expect(labels).toContain("Working");
    expect(labels).toContain("2");
    expect(page.querySelectorAll("[data-session='busy-one']")).toHaveLength(1);
    expect(page.querySelectorAll("[data-session='busy-two']")).toHaveLength(1);
    const order = Array.from(page.querySelectorAll("[data-session]")).map((el) => (el as HTMLElement).dataset.session);
    expect(order.slice(0, 2)).toEqual(["busy-one", "busy-two"]);
    expect(order).toContain("by-hand");
  });

  it("show no label when nothing is working", () => {
    render("p1");
    expect(Array.from(page.querySelectorAll("span")).map((el) => el.textContent)).not.toContain("Working");
  });

  it("stay in their own project", () => {
    (app.workspace.sessions as unknown[]).push(row("busy-away", { busy: true, projectId: "p2" }));
    render("p1");
    expect(page.querySelector("[data-session='busy-away']")).toBeNull();
  });
});

describe("the archive", () => {
  it("ends the list with a row that opens it, and Back returns to the list", () => {
    app.workspace.canListArchived = true;
    app.archivedTotal = 3;
    render("p1");
    expect(page.querySelector("[data-archive]")).toBeNull();
    click(byLabel("Archived, 3 sessions"));
    expect(page.querySelector("[data-archive]")).not.toBeNull();
    expect(page.querySelector("[data-session='by-hand']")).toBeNull();
    click(byLabel("Back to sessions"));
    expect(page.querySelector("[data-archive]")).toBeNull();
    expect(page.querySelector("[data-session='by-hand']")).not.toBeNull();
  });

  it("has no row when nothing is archived or the desktop can't list it", () => {
    app.workspace.canListArchived = true;
    app.archivedTotal = 0;
    render("p1");
    expect(page.querySelector("[aria-label^='Archived']")).toBeNull();
  });

  it("is left out for a desktop that can't list it", () => {
    app.archivedTotal = 5;
    render("p1");
    expect(page.querySelector("[aria-label^='Archived']")).toBeNull();
  });
});
