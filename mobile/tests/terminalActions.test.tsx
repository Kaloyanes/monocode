import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { useTerminalActions } from "@/features/terminal/terminalActions";

const desktop = vi.hoisted(() => ({
  request: vi.fn(),
  refresh: vi.fn(),
  actions: [{ id: "a", name: "Run Dev", command: "npm run dev", icon: "play" }],
}));
vi.mock("@/features/connection/nativeLink", () => ({ request: desktop.request }));
vi.mock("@/features/connection/ConnectionProvider", () => ({
  useConnection: () => ({ pairing: {}, refresh: desktop.refresh }),
}));
vi.mock("@/features/workspace/WorkspaceProvider", () => ({
  useWorkspace: () => ({ projects: [{ id: "p", actions: desktop.actions }] }),
}));

let seen: ReturnType<typeof useTerminalActions>;
function Probe() {
  seen = useTerminalActions("p");
  return null;
}

let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  desktop.request.mockReset().mockResolvedValue({});
  desktop.refresh.mockReset().mockResolvedValue(undefined);
  root = createRoot(document.createElement("div"));
  act(() => root.render(<Probe />));
});
afterEach(() => act(() => root.unmount()));

it("shows the desktop's actions and sends the whole list when one is removed", async () => {
  expect(seen[0]).toEqual(desktop.actions);
  await act(() => seen[1]([]));
  expect(desktop.request).toHaveBeenCalledWith({}, "terminal.actions.save", {
    projectId: "p",
    actions: [],
  });
  expect(desktop.refresh).toHaveBeenCalled();
});

it("shows a change at once, while it is on its way", async () => {
  let finish!: () => void;
  desktop.request.mockReturnValue(new Promise<void>((resolve) => (finish = resolve)));
  const added = [...desktop.actions, { name: "Test", command: "cargo test", icon: "play" }];
  let saving!: Promise<void>;
  act(() => {
    saving = seen[1](added);
  });
  expect(seen[0]).toEqual(added);
  await act(async () => {
    finish();
    await saving;
  });
  expect(seen[0]).toEqual(desktop.actions);
});
