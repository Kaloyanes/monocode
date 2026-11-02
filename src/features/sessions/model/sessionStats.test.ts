import { describe, expect, it } from "vitest";
import type { Block } from "./session";
import { cacheState, computeSessionStats, formatCountdown } from "./sessionStats";

const turn = (over: Partial<Block>): Block => ({
  id: "u",
  role: "user",
  text: "",
  ...over,
});

describe("computeSessionStats", () => {
  it("sums cost and marks it partial when a finished turn has none", () => {
    const stats = computeSessionStats([
      turn({ startedAt: 1000, durationMs: 500, turnMetrics: { costUsd: 0.5 } }),
      turn({ startedAt: 3000, durationMs: 500 }),
    ]);
    expect(stats.costUsd).toBe(0.5);
    expect(stats.costPartial).toBe(true);
    expect(stats.workingMs).toBe(1000);
  });

  it("hides cost when no turn reports one", () => {
    const stats = computeSessionStats([turn({ startedAt: 1, durationMs: 1 })]);
    expect(stats.costUsd).toBeUndefined();
    expect(stats.costPartial).toBe(false);
  });

  it("keeps the last known cache lifetime across read-only turns", () => {
    const stats = computeSessionStats([
      turn({ startedAt: 0, durationMs: 10, turnMetrics: { cacheTtlMs: 300_000 } }),
      turn({ startedAt: 100, durationMs: 10, turnMetrics: { cacheReadTokens: 5 } }),
    ]);
    expect(stats.cacheTtlMs).toBe(300_000);
    expect(stats.lastTurnEndedAt).toBe(110);
  });
});

describe("cacheState", () => {
  const stats = computeSessionStats([
    turn({ startedAt: 0, durationMs: 0, turnMetrics: { cacheTtlMs: 300_000 } }),
  ]);
  it("counts down, warns in the last 20%, then goes cold", () => {
    expect(cacheState(stats, 100_000, false)).toMatchObject({ kind: "warm", warning: false });
    expect(cacheState(stats, 250_000, false)).toMatchObject({ kind: "warm", warning: true });
    expect(cacheState(stats, 300_000, false)).toEqual({ kind: "cold" });
  });
  it("is live while busy and unknown without a lifetime", () => {
    expect(cacheState(stats, 0, true)).toEqual({ kind: "live" });
    expect(cacheState(computeSessionStats([]), 0, false)).toBeNull();
  });
  it("formats m:ss", () => expect(formatCountdown(252_000)).toBe("4:12"));
});
