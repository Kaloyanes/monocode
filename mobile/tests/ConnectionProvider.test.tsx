import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const world = vi.hoisted(() => ({
  appState: "active",
  appListeners: new Set<(state: string) => void>(),
  store: new Map<string, string>(),
  writes: [] as string[],
  requests: 0,
  catalog: undefined as unknown as Record<string, unknown>,
  addresses: [{ host: "10.0.0.2", port: 1 }],
  fail: false,
  rev: null as string | null,
  revs: [] as (string | undefined)[],
  // A desktop without `workspace.watch` unless a test opens one.
  watch: null as null | { frames: unknown[]; closed: number; opened: number; fail: string | null },
}));
vi.mock("react-native", () => ({
  Platform: { OS: "ios" },
  AppState: {
    get currentState() {
      return world.appState;
    },
    addEventListener: (_: string, listener: (state: string) => void) => {
      world.appListeners.add(listener);
      return { remove: () => world.appListeners.delete(listener) };
    },
  },
}));
vi.mock("expo-constants", () => ({ default: { deviceName: "Phone" } }));
vi.mock("expo-secure-store", () => ({
  WHEN_UNLOCKED_THIS_DEVICE_ONLY: 1,
  getItemAsync: async () => JSON.stringify({ certificate: "c", privateKey: "k" }),
  setItemAsync: async () => {},
}));
vi.mock("expo-sqlite/kv-store", () => ({
  default: {
    getItem: async (key: string) => world.store.get(key) ?? null,
    setItem: async (key: string, value: string) => {
      world.writes.push(key);
      world.store.set(key, value);
    },
    removeItem: async (key: string) => void world.store.delete(key),
  },
}));
vi.mock("@/features/connection/nativeLink", () => ({
  nativeCall: async () => ({}),
  parseInvitation: () => ({}),
  openWatch: async () => {
    if (!world.watch) throw new Error("Unsupported mobile operation");
    if (world.watch.fail) throw new Error(world.watch.fail);
    world.watch.opened++;
    return world.watch.opened;
  },
  nextUpdates: async () => {
    const watch = world.watch!;
    await new Promise((resolve) => setTimeout(resolve, 100));
    if (watch.fail) throw new Error(watch.fail);
    return watch.frames.splice(0);
  },
  closeWatch: async () => void world.watch!.closed++,
  request: async (_: unknown, __: string, params: { rev?: string } = {}) => {
    world.requests++;
    world.revs.push(params.rev);
    if (world.fail) throw new Error("offline");
    if (world.rev && params.rev === world.rev) return { unchanged: true, rev: world.rev };
    return structuredClone({
      ...world.catalog,
      addresses: world.addresses,
      ...(world.rev ? { rev: world.rev } : {}),
    });
  },
}));

import {
  ConnectionProvider,
  useCatalog,
  useCommand,
  useConnection,
} from "@/features/connection/ConnectionProvider";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const renders = { command: 0, monoTools: 0, everything: 0 };
const bump = (name: keyof typeof renders) => {
  renders[name]++;
};
function CommandOnly() {
  useCommand();
  bump("command");
  return null;
}
function ToolsOnly() {
  useCatalog((catalog) => catalog.monoTools === true);
  bump("monoTools");
  return null;
}
function Everything() {
  useConnection();
  bump("everything");
  return null;
}

const session = (id: string, busy = false) => ({ id, busy, title: id });
let root: ReturnType<typeof createRoot>;
let pushing = false;
const settle = () => act(async () => void (await vi.advanceTimersByTimeAsync(0)));
const advance = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));
const catalogWrites = () => world.writes.filter((key) => key.endsWith(".catalog")).length;
const flushedCatalog = () => {
  world.appListeners.forEach((listener) => listener("inactive"));
  return world.store.get("monocode.mobile.fp.catalog") ?? "{}";
};
const pairingWrites = () => world.writes.filter((key) => key === "monocode.mobile.pairing").length;

beforeEach(async () => {
  vi.useFakeTimers();
  world.appState = "active";
  world.appListeners.clear();
  world.store.clear();
  world.writes.length = 0;
  world.requests = 0;
  world.fail = false;
  world.rev = null;
  world.revs.length = 0;
  world.watch = pushing ? { frames: [], closed: 0, opened: 0, fail: null } : null;
  world.addresses = [{ host: "10.0.0.2", port: 1 }];
  world.catalog = { projects: [], monos: [], sessions: [session("one")] };
  world.store.set(
    "monocode.mobile.pairing",
    JSON.stringify({ fingerprint: "fp", addresses: world.addresses }),
  );
  Object.assign(renders, { command: 0, monoTools: 0, everything: 0 });
  root = createRoot(document.createElement("div"));
  await act(async () =>
    root.render(
      <ConnectionProvider>
        <CommandOnly />
        <ToolsOnly />
        <Everything />
      </ConnectionProvider>,
    ),
  );
  await settle();
  world.writes.length = 0;
});
afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
});

describe("ConnectionProvider following the desktop", () => {
  beforeAll(() => void (pushing = true));
  afterAll(() => void (pushing = false));

  const push = (frame: unknown) => world.watch!.frames.push(frame);

  it("reads nothing while the desktop has nothing to push", async () => {
    await advance(60_000);
    expect(world.requests).toBe(0);
    expect(world.watch!.opened).toBe(1);
  });

  it("takes what the desktop pushes", async () => {
    push({ projects: [], monos: [], sessions: [session("pushed")], rev: "a" });
    await advance(200);
    expect(JSON.parse(flushedCatalog()).sessions[0].id).toBe("pushed");
    expect(world.requests).toBe(0);
  });

  it("opens again on what it holds after the watch ends", async () => {
    push({ projects: [], monos: [], sessions: [session("pushed")], rev: "a" });
    await advance(200);
    world.watch!.fail = "Desktop stopped answering";
    await advance(200);
    expect(world.watch!.closed).toBe(1);
    world.watch!.fail = null;
    await advance(2_000);
    expect(world.watch!.opened).toBe(2);
  });

  it("closes the watch in the background and opens it again when shown", async () => {
    world.appState = "background";
    world.appListeners.forEach((listener) => listener("background"));
    await advance(200);
    expect(world.watch!.closed).toBe(1);
    await advance(60_000);
    expect(world.watch!.opened).toBe(1);
    world.appState = "active";
    world.appListeners.forEach((listener) => listener("active"));
    await advance(200);
    expect(world.watch!.opened).toBe(2);
  });
});

describe("ConnectionProvider polling", () => {
  it("reads every five seconds while a session works", async () => {
    world.catalog = { projects: [], monos: [], sessions: [session("one", true)] };
    await advance(5000);
    world.requests = 0;
    await advance(5000 * 12 + 100);
    expect(world.requests).toBe(12);
  });

  it("reads every five seconds while a Mono works", async () => {
    world.catalog = { projects: [], monos: [{ id: "m", status: "working" }], sessions: [] };
    await advance(5000);
    world.requests = 0;
    await advance(5000 * 12 + 100);
    expect(world.requests).toBe(12);
  });

  it("slows down once nothing is moving, and picks up when something does", async () => {
    await advance(5000 * 4);
    world.requests = 0;
    await advance(15000 * 4 + 100);
    expect(world.requests).toBe(4);
    world.catalog = { projects: [], monos: [], sessions: [session("one"), session("two")] };
    await advance(15000);
    world.requests = 0;
    await advance(5000 * 2 + 100);
    expect(world.requests).toBe(2);
  });

  it("asks with the desktop's rev, and keeps the workspace it holds", async () => {
    world.rev = "a";
    await advance(5000);
    world.revs.length = 0;
    const seen = world.requests;
    await advance(5000 + 100);
    expect(world.requests).toBe(seen + 1);
    expect(world.revs).toEqual(["a"]);
    const before = renders.everything;
    await advance(5000);
    expect(renders.everything).toBe(before);
    expect(world.fail).toBe(false);
    world.rev = "b";
    world.catalog = { projects: [], monos: [], sessions: [session("one"), session("two")] };
    await advance(15000);
    expect(renders.everything).toBeGreaterThan(before);
    world.revs.length = 0;
    await advance(5000 + 100);
    expect(world.revs).toEqual(["b"]);
  });

  it("reads an older desktop in full, without a rev", async () => {
    await advance(5000 * 3);
    expect(world.revs.every((rev) => rev === undefined)).toBe(true);
  });

  it("shows a session started on the desktop within one poll", async () => {
    const seen = renders.everything;
    world.catalog = { projects: [], monos: [], sessions: [session("one"), session("new")] };
    await advance(5000);
    expect(renders.everything).toBeGreaterThan(seen);
  });

  it("backs off after failures", async () => {
    world.fail = true;
    await advance(5000 * 3);
    world.requests = 0;
    await advance(300_000);
    // A fixed 5 s retry would be 60 reads.
    expect(world.requests).toBeLessThan(15);
  });

  it("does not read in the background", async () => {
    world.appState = "background";
    await advance(5000 * 3);
    world.requests = 0;
    await advance(5000 * 30);
    expect(world.requests).toBe(0);
    world.appState = "active";
    await act(async () => {
      for (const listener of [...world.appListeners]) listener("active");
    });
    await settle();
    expect(world.requests).toBeGreaterThan(0);
  });
});

describe("ConnectionProvider writes and renders", () => {
  it("writes neither the pairing nor the catalog when nothing changed", async () => {
    await advance(30000 * 4);
    expect(pairingWrites()).toBe(0);
    expect(catalogWrites()).toBeLessThanOrEqual(1);
    const before = catalogWrites();
    await advance(30000 * 4);
    expect(catalogWrites()).toBe(before);
  });

  it("writes the pairing when the addresses change", async () => {
    world.addresses = [{ host: "10.0.0.3", port: 1 }];
    await advance(5000 + 100);
    expect(pairingWrites()).toBe(1);
  });

  it("saves a changing catalog at most once per interval, and when backgrounded", async () => {
    let n = 0;
    const change = () => {
      world.catalog = { projects: [], monos: [], sessions: [session("one"), session(`s${++n}`)] };
    };
    // A catalog that changes at every 5 s read, for two minutes.
    world.catalog = { projects: [], monos: [], sessions: [session("one", true)] };
    for (let i = 0; i < 24; i++) {
      change();
      (world.catalog.sessions as { busy: boolean }[])[0].busy = true;
      await advance(5000);
    }
    expect(catalogWrites()).toBeLessThanOrEqual(5);
    expect(catalogWrites()).toBeGreaterThanOrEqual(2);
    change();
    await advance(5000);
    const before = catalogWrites();
    await act(async () => {
      world.appState = "background";
      for (const listener of [...world.appListeners]) listener("background");
    });
    await settle();
    expect(catalogWrites()).toBeGreaterThan(before - 1);
    const saved = JSON.parse(world.store.get("monocode.mobile.fp.catalog") ?? "{}");
    expect(saved.sessions.length).toBe(2);
  });

  it("a poll that finds nothing new updates no state and renders nothing", async () => {
    const before = { ...renders };
    const commits = vi.fn();
    const { Profiler } = await import("react");
    await act(async () =>
      root.render(
        <Profiler id="p" onRender={commits}>
          <ConnectionProvider>
            <CommandOnly />
            <ToolsOnly />
            <Everything />
          </ConnectionProvider>
        </Profiler>,
      ),
    );
    await settle();
    // Let the first read land, then count only the polls after it.
    await advance(5000);
    const quiet = { ...renders };
    commits.mockClear();
    world.requests = 0;
    await advance(15000 * 4);
    expect(world.requests).toBeGreaterThanOrEqual(4);
    // Reads of the same workspace: no render of any reader, no commit.
    expect(renders).toEqual(quiet);
    expect(commits).not.toHaveBeenCalled();
    expect(before.everything).toBeGreaterThan(0);
  });

  it("re-renders command-only and flag-only readers never, whatever the catalog does", async () => {
    const command = renders.command;
    const tools = renders.monoTools;
    const everything = renders.everything;
    world.catalog = { projects: [], monos: [], sessions: [session("one"), session("two")] };
    await advance(15000 + 100);
    expect(renders.everything).toBeGreaterThan(everything);
    expect(renders.command).toBe(command);
    expect(renders.monoTools).toBe(tools);
  });
});
