import type { Block } from "./session";

/** Share of the cache lifetime, at the end, where the countdown turns amber. */
const WARN_FRACTION = 0.2;

export type SessionStats = {
  /** Session cost so far; absent when no turn reported one. */
  costUsd?: number;
  /** Some completed turns reported no cost, so `costUsd` is a lower bound. */
  costPartial: boolean;
  /** Epoch ms of the first turn. */
  startedAt?: number;
  /** Time the agent spent working across turns. */
  workingMs: number;
  /** Cache lifetime last written by the provider, when known. */
  cacheTtlMs?: number;
  /** Epoch ms when the last turn ended. */
  lastTurnEndedAt?: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** Cache hit share over the session, as a percentage. */
  sessionHitPercent?: number;
  /** Cache hit share of the latest turn that reported one. */
  lastHitPercent?: number;
};

export type CacheState =
  | { kind: "live" }
  | { kind: "cold" }
  | { kind: "warm"; remainingMs: number; warning: boolean };

export function computeSessionStats(blocks: readonly Block[]): SessionStats {
  let costUsd: number | undefined;
  let costPartial = false;
  let startedAt: number | undefined;
  let workingMs = 0;
  let cacheTtlMs: number | undefined;
  let lastTurnEndedAt: number | undefined;
  let read = 0;
  let write = 0;
  let input = 0;
  let reported = false;
  let lastHitPercent: number | undefined;

  for (const block of blocks) {
    if (block.role !== "user" || block.startedAt == null) continue;
    startedAt ??= block.startedAt;
    const done = block.durationMs != null;
    if (done) {
      workingMs += block.durationMs ?? 0;
      lastTurnEndedAt = block.startedAt + (block.durationMs ?? 0);
    }
    const metrics = block.turnMetrics;
    if (metrics?.costUsd != null) costUsd = (costUsd ?? 0) + metrics.costUsd;
    else if (done) costPartial = true;
    if (metrics?.cacheTtlMs) cacheTtlMs = metrics.cacheTtlMs;
    if (metrics?.cacheHitPercent != null) {
      reported = true;
      lastHitPercent = metrics.cacheHitPercent;
    }
    read += metrics?.cacheReadTokens ?? 0;
    write += metrics?.cacheWriteTokens ?? 0;
    input += metrics?.inputTokens ?? 0;
  }

  const cacheable = input + read + write;
  return {
    ...(costUsd != null ? { costUsd } : {}),
    costPartial: costUsd != null && costPartial,
    ...(startedAt != null ? { startedAt } : {}),
    workingMs,
    ...(cacheTtlMs ? { cacheTtlMs } : {}),
    ...(lastTurnEndedAt != null ? { lastTurnEndedAt } : {}),
    cacheReadTokens: read,
    cacheWriteTokens: write,
    ...(reported && cacheable ? { sessionHitPercent: (read / cacheable) * 100 } : {}),
    ...(lastHitPercent != null ? { lastHitPercent } : {}),
  };
}

/** Cache state at `now`, or null when its lifetime or last turn is unknown. */
export function cacheState(
  stats: SessionStats,
  now: number,
  busy: boolean,
): CacheState | null {
  if (busy) return { kind: "live" };
  if (!stats.cacheTtlMs || stats.lastTurnEndedAt == null) return null;
  const remainingMs = stats.lastTurnEndedAt + stats.cacheTtlMs - now;
  if (remainingMs <= 0) return { kind: "cold" };
  return {
    kind: "warm",
    remainingMs,
    warning: remainingMs <= stats.cacheTtlMs * WARN_FRACTION,
  };
}

export function formatCost(usd: number): string {
  return usd < 0.01 && usd > 0 ? "<$0.01" : `$${usd.toFixed(2)}`;
}

/** m:ss countdown. */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const seconds = String(total % 60).padStart(2, "0");
  return `${Math.floor(total / 60)}:${seconds}`;
}

/** Elapsed time: 45s, 14m, 2h 5m. */
export function formatSpan(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return `${Math.floor(Math.max(0, ms) / 1000)}s`;
  if (minutes < 60) return `${minutes}m`;
  const rest = minutes % 60;
  return rest ? `${Math.floor(minutes / 60)}h ${rest}m` : `${Math.floor(minutes / 60)}h`;
}
