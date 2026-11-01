import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { useWordFade } from "@/features/sessions/wordReveal";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let renders = 0;
const nows: number[] = [];
function Reply({ count }: { count: number }) {
  const { now } = useWordFade(count, true);
  renders++;
  nows.push(now);
  return null;
}

// One 60 Hz frame per act, so each state update renders as it would on a device.
const frames = async (ms: number) => {
  for (let t = 0; t < ms; t += 16) await act(async () => void (await vi.advanceTimersByTimeAsync(16)));
};
let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame", "performance", "setTimeout", "clearTimeout"] });
  renders = 0;
  nows.length = 0;
  root = createRoot(document.createElement("div"));
});
afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
});

it("renders fading words about 30 times a second, not once per 60 Hz frame, and settles", async () => {
  await act(async () => root.render(<Reply count={1} />));
  await act(async () => root.render(<Reply count={5} />));
  renders = 0;
  // A fade lasts 480 ms at most; two seconds covers it and then some.
  await frames(2000);
  const fadeMs = 480;
  // At 60 Hz that fade was about 30 renders; throttled it is about half.
  expect(renders).toBeGreaterThan(5);
  expect(renders).toBeLessThanOrEqual(Math.ceil(fadeMs / 32) + 3);
  // Nothing renders once the newest word has settled.
  const settled = renders;
  await frames(2000);
  expect(renders).toBe(settled);
});
