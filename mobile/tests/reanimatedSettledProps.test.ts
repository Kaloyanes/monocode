import { describe, expect, it } from "vitest";

import { countOnlyRegisteredViews } from "@/ui/reanimatedSettledProps";

// The real collector needs Reanimated's native module, so this copies how 4.5.1
// counts views. Its unregisterView is the one the patch has to replace.
function collector4_5_1() {
  return {
    viewsCount: 0,
    viewsMap: new Map<number, unknown>(),
    running: false,
    registerView(viewTag: number, component: unknown) {
      if (this.viewsMap.has(viewTag)) return;
      this.viewsMap.set(viewTag, component);
      this.viewsCount++;
      if (this.viewsCount === 1) this.running = true;
    },
    unregisterView(viewTag: number) {
      this.viewsMap.delete(viewTag);
      this.viewsCount--;
      if (this.viewsCount === 0) this.unregisterInterval();
    },
    unregisterInterval() {
      this.running = false;
    },
  };
}

function patched() {
  const collector = collector4_5_1();
  countOnlyRegisteredViews(collector);
  return collector;
}

describe("settled props collector", () => {
  it("stops early unpatched, which is the bug", () => {
    const collector = collector4_5_1();
    collector.registerView(1, {});
    collector.registerView(1, {});
    collector.registerView(2, {});

    collector.unregisterView(1);
    collector.unregisterView(1);

    expect(collector.viewsMap.size).toBe(1);
    expect(collector.running).toBe(false);
  });

  it("keeps running when a tag that repeated is unmounted twice", () => {
    const collector = patched();
    collector.registerView(1, {});
    collector.registerView(1, {});
    collector.registerView(2, {});

    collector.unregisterView(1);
    collector.unregisterView(1);

    expect(collector.viewsCount).toBe(1);
    expect(collector.running).toBe(true);
  });

  it("stops once the last view is gone", () => {
    const collector = patched();
    collector.registerView(1, {});
    collector.registerView(2, {});

    collector.unregisterView(1);
    collector.unregisterView(2);

    expect(collector.viewsCount).toBe(0);
    expect(collector.running).toBe(false);
  });

  it("ignores a tag it never counted", () => {
    const collector = patched();
    collector.registerView(1, {});

    collector.unregisterView(9);

    expect(collector.viewsCount).toBe(1);
    expect(collector.running).toBe(true);
  });
});
