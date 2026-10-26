import { useLayoutEffect, useRef, useState } from "react";

/** Rows drawn above and below the viewport so fast scrolls don't show gaps. */
const OVERSCAN = 12;
/** Rows drawn before the scroller has a measured height (first paint, tests). */
const UNMEASURED_ROWS = 80;

type Range = { start: number; end: number };

/**
 * Windows a list of fixed-height rows that sits somewhere inside a scrolling
 * ancestor, possibly below other content. Attach `anchorRef` to an element at
 * the list's top edge and render rows `start`..`end` between spacers of
 * `start * rowHeight` and `(count - end) * rowHeight`.
 */
export function useRowWindow<T extends HTMLElement>(
  count: number,
  rowHeight: number,
) {
  const anchorRef = useRef<T>(null);
  const [range, setRange] = useState<Range>(() => ({
    start: 0,
    end: Math.min(count, UNMEASURED_ROWS),
  }));
  const measureRef = useRef<() => void>(() => {});

  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    const scroller = anchor ? scrollParent(anchor) : null;
    const measure = () => {
      const next = visibleRange(anchor, scroller, count, rowHeight);
      setRange((prev) =>
        prev.start === next.start && prev.end === next.end ? prev : next,
      );
    };
    measureRef.current = measure;
    measure();
    if (!scroller) return;
    scroller.addEventListener("scroll", measure, { passive: true });
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(measure);
    observer?.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", measure);
      observer?.disconnect();
    };
  }, [count, rowHeight]);

  // Content above the list (another section opening, a status line) moves it
  // without scrolling or resizing the scroller, so re-check after each render.
  useLayoutEffect(() => measureRef.current());

  const end = Math.min(range.end, count);
  const start = Math.min(range.start, end);
  return { anchorRef, start, end };
}

function visibleRange(
  anchor: HTMLElement | null,
  scroller: HTMLElement | null,
  count: number,
  rowHeight: number,
): Range {
  const viewport = scroller?.clientHeight ?? 0;
  if (!anchor || !scroller || viewport === 0) {
    return { start: 0, end: Math.min(count, UNMEASURED_ROWS) };
  }
  const top =
    anchor.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  const first = Math.floor(-top / rowHeight) - OVERSCAN;
  const last = Math.ceil((viewport - top) / rowHeight) + OVERSCAN;
  const start = Math.max(0, Math.min(count, first));
  return { start, end: Math.max(start, Math.min(count, last)) };
}

function scrollParent(element: HTMLElement): HTMLElement | null {
  for (let node = element.parentElement; node; node = node.parentElement) {
    const { overflowY } = getComputedStyle(node);
    if (overflowY === "auto" || overflowY === "scroll") return node;
  }
  return null;
}
