import { describe, expect, it } from "vitest";

import {
  HIGHLIGHT_FAST_MS,
  HIGHLIGHT_PUBLISH_MS,
  HIGHLIGHT_SLOW_MS,
  HighlightProgress,
  nextHighlightDelay,
} from "@/features/files/highlightProgress";

const reply = (count: number, palette = ["#fff"]) => ({
  palette,
  lines: Array.from({ length: count }, () => [["x", 0]] as [string, number][]),
});

describe("HighlightProgress", () => {
  it("accumulates replies and hands them over about once a second", () => {
    const progress = new HighlightProgress();
    // A reply every 250 ms for ten seconds: 40 replies, once 40 re-renders.
    let published = 0;
    for (let i = 0; i < 40; i++) {
      progress.add(reply(100));
      if (progress.take(i * 250)) published++;
    }
    expect(progress.collected).toBe(4000);
    expect(published).toBeLessThanOrEqual(11);
    expect(published).toBeGreaterThanOrEqual(9);
  });

  it("hands over the last of it at once, and only when there is something new", () => {
    const progress = new HighlightProgress();
    expect(progress.take(0, true)).toBeNull();
    progress.add(reply(3));
    expect(progress.take(0)?.lines.length).toBe(3);
    progress.add(reply(2));
    expect(progress.take(HIGHLIGHT_PUBLISH_MS / 2)).toBeNull();
    expect(progress.take(HIGHLIGHT_PUBLISH_MS / 2, true)?.lines.length).toBe(5);
    expect(progress.take(HIGHLIGHT_PUBLISH_MS * 5)).toBeNull();
  });

  it("gives a new array each time so the screen sees the change", () => {
    const progress = new HighlightProgress();
    progress.add(reply(1));
    const first = progress.take(0)!;
    progress.add(reply(1));
    const second = progress.take(5000)!;
    expect(second.lines).not.toBe(first.lines);
    expect(first.lines.length).toBe(1);
  });
});

describe("nextHighlightDelay", () => {
  it("is quick while lines arrive and doubles up to a ceiling while none do", () => {
    let delay = HIGHLIGHT_FAST_MS;
    const waits: number[] = [];
    for (let i = 0; i < 5; i++) waits.push((delay = nextHighlightDelay(delay, false)));
    expect(waits).toEqual([500, 1000, HIGHLIGHT_SLOW_MS, HIGHLIGHT_SLOW_MS, HIGHLIGHT_SLOW_MS]);
    expect(nextHighlightDelay(delay, true)).toBe(HIGHLIGHT_FAST_MS);
  });
});
