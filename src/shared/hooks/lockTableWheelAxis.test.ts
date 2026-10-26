// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { lockTableWheelAxis } from "./lockTableWheelAxis";

function sized(el: HTMLElement, sizes: Record<string, number>) {
  for (const [name, value] of Object.entries(sizes)) {
    Object.defineProperty(el, name, { value, configurable: true });
  }
  return el;
}

function setup() {
  const outer = sized(document.createElement("div"), {
    scrollHeight: 2000,
    clientHeight: 500,
  });
  outer.style.overflowY = "auto";
  const root = document.createElement("div");
  const wrapper = document.createElement("div");
  wrapper.dataset.streamdown = "table-wrapper";
  const scroller = document.createElement("div");
  const table = document.createElement("table");
  table.dataset.streamdown = "table";
  const cell = document.createElement("td");
  table.append(cell);
  scroller.append(table);
  wrapper.append(scroller);
  root.append(wrapper);
  outer.append(root);
  document.body.append(outer);
  const detach = lockTableWheelAxis(root);
  const wheel = (init: WheelEventInit, target: Element = cell) => {
    const event = new WheelEvent("wheel", {
      bubbles: true,
      cancelable: true,
      ...init,
    });
    // happy-dom drops modifier flags from the init dictionary.
    if (init.shiftKey)
      Object.defineProperty(event, "shiftKey", { value: true });
    target.dispatchEvent(event);
    return event;
  };
  return { outer, root, wheel, detach };
}

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("lockTableWheelAxis", () => {
  it("drops the sideways drift of a vertical swipe and scrolls the outer pane", () => {
    const { outer, wheel, detach } = setup();
    const event = wheel({ deltaX: 3, deltaY: 40 });
    expect(event.defaultPrevented).toBe(true);
    expect(outer.scrollTop).toBe(40);
    detach();
  });

  it("keeps rejecting drift for the rest of the gesture", () => {
    const { outer, wheel, detach } = setup();
    wheel({ deltaX: 1, deltaY: 40 });
    // The later events of a vertical swipe can lean sideways more than the first.
    const event = wheel({ deltaX: 9, deltaY: 6 });
    expect(event.defaultPrevented).toBe(true);
    expect(outer.scrollTop).toBe(46);
    detach();
  });

  it("leaves a horizontal swipe to scroll the table", () => {
    const { outer, wheel, detach } = setup();
    const event = wheel({ deltaX: 40, deltaY: 2 });
    expect(event.defaultPrevented).toBe(false);
    expect(outer.scrollTop).toBe(0);
    detach();
  });

  it("leaves a plain mouse wheel alone", () => {
    const { outer, wheel, detach } = setup();
    const event = wheel({ deltaX: 0, deltaY: 100 });
    expect(event.defaultPrevented).toBe(false);
    expect(outer.scrollTop).toBe(0);
    detach();
  });

  it("leaves shift-wheel, which scrolls sideways on purpose", () => {
    const { wheel, detach } = setup();
    const event = wheel({ deltaX: 4, deltaY: 40, shiftKey: true });
    expect(event.defaultPrevented).toBe(false);
    detach();
  });

  it("ignores wheels outside a table", () => {
    const { root, wheel, detach } = setup();
    const event = wheel({ deltaX: 3, deltaY: 40 }, root);
    expect(event.defaultPrevented).toBe(false);
    detach();
  });

  it("starts a fresh gesture after the wheel goes idle", () => {
    const now = vi.spyOn(performance, "now");
    const { wheel, detach } = setup();
    now.mockReturnValue(1000);
    wheel({ deltaX: 1, deltaY: 40 });
    now.mockReturnValue(1500);
    const event = wheel({ deltaX: 40, deltaY: 1 });
    expect(event.defaultPrevented).toBe(false);
    detach();
  });
});
