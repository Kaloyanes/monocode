import { useEffect, useRef, useState } from "react";
import { formatTokens, type ContextUsage } from "../model/contextUsage";
import {
  cacheState,
  formatCost,
  formatCountdown,
  formatSpan,
  type CacheState,
  type SessionStats,
} from "../model/sessionStats";
import { Popover } from "../../../shared/ui/Popover";

function cacheLabel(state: CacheState): string {
  if (state.kind === "live") return "cache live";
  if (state.kind === "cold") return "cache cold";
  return `cache ${formatCountdown(state.remainingMs)}`;
}

function cacheClass(state: CacheState | null): string {
  if (state?.kind === "warm" && state.warning) return "text-amber-400";
  if (state?.kind === "cold") return "text-content/30";
  return "";
}

function percent(value: number | undefined): string {
  return value == null ? "n/a" : `${Math.round(value)}%`;
}

/**
 * What the next message will cost: session spend, how long the prompt cache
 * stays warm, and how long the session has run. Each part hides when the
 * provider does not report what it needs; nothing renders with no data at all.
 */
export function SessionStatsChip({
  stats,
  busy,
  context,
}: {
  stats?: SessionStats;
  busy: boolean;
  context?: ContextUsage;
}) {
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const ticking =
    stats?.startedAt != null &&
    (busy || stats.cacheTtlMs != null || stats.lastTurnEndedAt != null);

  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);

  if (!stats || stats.startedAt == null) return null;
  const cache = cacheState(stats, now, busy);
  const parts: { text: string; className?: string }[] = [];
  if (stats.costUsd != null) {
    parts.push({
      text: `${stats.costPartial ? "≥" : ""}${formatCost(stats.costUsd)}`,
    });
  }
  if (cache) parts.push({ text: cacheLabel(cache), className: cacheClass(cache) });
  parts.push({ text: formatSpan(now - stats.startedAt) });

  const rewrite =
    cache?.kind === "cold" && context?.used
      ? `Next message re-writes ~${formatTokens(context.used)} tokens to cache`
      : null;

  return (
    <div ref={root} className="relative shrink-0">
      <button
        type="button"
        aria-label="Session cost, cache, and time"
        aria-expanded={open}
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        className="flex items-center gap-1 rounded-sm text-[11px] leading-4 tabular-nums text-content/45 outline-none focus-visible:ring-1 focus-visible:ring-accent"
      >
        {parts.map((part, index) => (
          <span key={index} className="flex items-center gap-1">
            {index > 0 ? <span aria-hidden>·</span> : null}
            <span className={part.className}>{part.text}</span>
          </span>
        ))}
      </button>
      {open ? (
        <Popover
          anchor={root}
          side="top"
          align="end"
          className="pointer-events-none w-max px-2.5 py-1.5"
        >
          <dl className="grid grid-cols-[auto_auto] gap-x-4 text-[11px] leading-4">
            {stats.costUsd != null ? (
              <Row
                label="Cost (API list price)"
                value={`${stats.costPartial ? "≥" : ""}${formatCost(stats.costUsd)}`}
              />
            ) : null}
            {cache ? <Row label="Cache" value={cacheLabel(cache)} /> : null}
            <Row
              label="Cache hit"
              value={`${percent(stats.sessionHitPercent)} session · ${percent(stats.lastHitPercent)} last turn`}
            />
            <Row
              label="Cache tokens"
              value={`${formatTokens(stats.cacheReadTokens)} read · ${formatTokens(stats.cacheWriteTokens)} written`}
            />
            <Row
              label="Started"
              value={new Date(stats.startedAt).toLocaleTimeString([], {
                hour: "numeric",
                minute: "2-digit",
              })}
            />
            <Row label="Agent worked" value={formatSpan(stats.workingMs)} />
          </dl>
          {rewrite ? (
            <div className="mt-1 text-[11px] leading-4 text-amber-400">
              {rewrite}
            </div>
          ) : null}
        </Popover>
      ) : null}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <>
      <dt className="text-content/50">{label}</dt>
      <dd className="text-content">{value}</dd>
    </>
  );
}
