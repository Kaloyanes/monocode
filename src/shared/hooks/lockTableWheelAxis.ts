import { useCallback, useRef } from "react";

const GESTURE_IDLE_MS = 120;
const LINE_HEIGHT_PX = 16;

/** The horizontal scroller Streamdown puts around each table. */
function tableScroller(target: EventTarget | null): HTMLElement | null {
  if (!(target instanceof Element)) return null;
  const parent = target.closest('[data-streamdown="table"]')?.parentElement;
  return parent instanceof HTMLElement ? parent : null;
}

function verticalScroller(from: HTMLElement | null): HTMLElement | null {
  for (let node = from; node; node = node.parentElement) {
    if (
      node.scrollHeight > node.clientHeight + 1 &&
      /auto|scroll|overlay/.test(getComputedStyle(node).overflowY)
    ) {
      return node;
    }
  }
  return null;
}

/**
 * Keep a vertical wheel gesture from sliding a wide table sideways. Trackpads
 * add a small sideways delta to a mostly vertical swipe, and a scroller with
 * `overflow-x: auto` takes every bit of it. The first event of a gesture picks
 * its axis; a vertical gesture has its sideways delta dropped and its vertical
 * delta handed to the scroller around the table.
 */
export function lockTableWheelAxis(root: HTMLElement): () => void {
  let lastEvent = -Infinity;
  let axis: "pending" | "horizontal" | "vertical" = "pending";
  const onWheel = (event: WheelEvent) => {
    const scroller = tableScroller(event.target);
    if (!scroller) return;
    // Modified wheels are deliberate: shift scrolls sideways, ctrl zooms.
    if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) {
      return;
    }
    const now = performance.now();
    if (now - lastEvent > GESTURE_IDLE_MS) axis = "pending";
    lastEvent = now;
    if (axis === "pending") {
      if (event.deltaX === 0 && event.deltaY === 0) return;
      axis =
        Math.abs(event.deltaX) > Math.abs(event.deltaY)
          ? "horizontal"
          : "vertical";
    }
    // A plain mouse wheel has no sideways delta, so native scrolling is fine.
    if (axis !== "vertical" || event.deltaX === 0) return;
    event.preventDefault();
    const scale =
      event.deltaMode === 1
        ? LINE_HEIGHT_PX
        : event.deltaMode === 2
          ? root.clientHeight
          : 1;
    const outer = verticalScroller(scroller.parentElement);
    if (outer) outer.scrollTop += event.deltaY * scale;
  };
  root.addEventListener("wheel", onWheel, { passive: false });
  return () => root.removeEventListener("wheel", onWheel);
}

export function useLockTableWheelAxis<T extends HTMLElement>() {
  const cleanup = useRef<(() => void) | null>(null);

  return useCallback((el: T | null) => {
    cleanup.current?.();
    cleanup.current = el ? lockTableWheelAxis(el) : null;
  }, []);
}
