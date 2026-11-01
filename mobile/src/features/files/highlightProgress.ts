/** A large file's colours as far as they are collected. */
export type Highlight = { palette: string[]; lines: [string, number][][] };

/** How soon to ask again while colours keep arriving. */
export const HIGHLIGHT_FAST_MS = 250;
/** The longest wait between asks while none arrive. */
export const HIGHLIGHT_SLOW_MS = 2000;
/** The screen is given what has been collected at most this often. */
export const HIGHLIGHT_PUBLISH_MS = 1000;

/** The wait before the next ask: short after a reply with lines, doubling after an empty one. */
export const nextHighlightDelay = (current: number, gotLines: boolean) =>
  gotLines ? HIGHLIGHT_FAST_MS : Math.min(HIGHLIGHT_SLOW_MS, current * 2);

/**
 * Collects colours as the desktop finishes them and hands them to the screen
 * only now and then. Each hand-over re-renders the whole file, so replies
 * pile up here, in place, between hand-overs.
 */
export class HighlightProgress {
  private palette: string[] = [];
  private lines: [string, number][][] = [];
  private dirty = false;
  private publishedAt = -Infinity;

  /** How many lines are collected, which is where the next ask picks up. */
  get collected() {
    return this.lines.length;
  }

  add(value: Highlight) {
    if (value.lines.length === 0 && value.palette.length === this.palette.length) return false;
    this.palette = value.palette;
    for (const line of value.lines) this.lines.push(line);
    this.dirty = true;
    return value.lines.length > 0;
  }

  /** What to show now, if something new is collected and it is time (or `force`). */
  take(now: number, force = false): Highlight | null {
    if (!this.dirty || (!force && now - this.publishedAt < HIGHLIGHT_PUBLISH_MS)) return null;
    this.dirty = false;
    this.publishedAt = now;
    return { palette: this.palette, lines: this.lines.slice() };
  }
}
