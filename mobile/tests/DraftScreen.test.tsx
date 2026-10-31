import { act, forwardRef, useImperativeHandle, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DraftScreen } from "@/features/drafts/DraftScreen";

const page = vi.hoisted(() => ({
  props: null as null | Record<string, (...args: never[]) => unknown> & { source?: { html: string } },
  injected: [] as string[],
  touch: null as null | (() => void),
}));
const link = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  answer: (() => new Promise(() => {})) as (
    method: string,
    params: Record<string, unknown>,
  ) => Promise<unknown>,
}));

vi.mock("react-native", () => {
  const View = ({
    children,
    onTouchStart,
  }: {
    children?: ReactNode;
    onTouchStart?: () => void;
  }) => {
    if (onTouchStart) page.touch = onTouchStart;
    return <div>{children}</div>;
  };
  return {
    View,
    ScrollView: View,
    Text: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
    ActivityIndicator: () => <span>loading</span>,
    Pressable: ({
      children,
      onPress,
      accessibilityLabel,
    }: {
      children?: ReactNode;
      onPress?: () => void;
      accessibilityLabel?: string;
    }) => (
      <button aria-label={accessibilityLabel} onClick={onPress}>
        {children}
      </button>
    ),
    Image: ({
      source,
      onLoad,
      accessibilityLabel,
    }: {
      source: { uri: string };
      onLoad?: () => void;
      accessibilityLabel?: string;
    }) => <img src={source.uri} alt={accessibilityLabel} onLoad={onLoad} />,
    Alert: { alert: vi.fn() },
    AppState: { currentState: "active" },
    useWindowDimensions: () => ({ width: 390, height: 844, scale: 3 }),
  };
});
vi.mock("react-native-webview", () => {
  const WebView = forwardRef((props: NonNullable<typeof page.props>, ref) => {
    page.props = props;
    useImperativeHandle(ref, () => ({
      injectJavaScript: (script: string) => page.injected.push(script),
      reload: () => {},
    }));
    return <div data-web />;
  });
  WebView.displayName = "WebView";
  return { WebView };
});
vi.mock("expo-haptics", () => ({ impactAsync: vi.fn(), ImpactFeedbackStyle: { Light: "light" } }));
vi.mock("@/features/connection/ConnectionProvider", () => {
  // One function for the life of the connection, as the provider's is.
  const connection = {
    command: (method: string, params: Record<string, unknown>) => {
      link.calls.push({ method, params });
      return link.answer(method, params);
    },
  };
  return { useConnection: () => connection };
});
vi.mock("@/features/connection/nativeLink", () => ({ makeCommandId: () => "cmd" }));
vi.mock("@/ui/theme", () => {
  const colors = { sheet: "#141414", muted: "#8c8c8c", faint: "#5c5c5c", accent: "#4c9ef5", fill: "#222", content: "#eee" };
  return {
    radii: { pill: 999 },
    themedStyles: (build: (c: typeof colors) => unknown) => () => build(colors),
    useColors: () => colors,
    useScheme: () => "dark",
    withAlpha: () => "#333",
  };
});

const record = (over: Record<string, unknown> = {}) => ({
  live: true,
  title: "Pricing",
  rev: 4,
  generation: 1,
  kind: "html",
  state: { radius: 12 },
  stateRev: 2,
  pins: [],
  pending: [],
  control: false,
  waiting: false,
  ...over,
});

const flush = () => act(async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
});
const message = (data: Record<string, unknown>) =>
  page.props!.onMessage({ nativeEvent: { data: JSON.stringify({ __mc: 1, ...data }) } } as never);

let root: ReturnType<typeof createRoot> | null = null;
function mount(element: ReactNode) {
  const host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  act(() => root!.render(element));
  return host;
}

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  page.props = null;
  page.injected = [];
  page.touch = null;
  link.calls = [];
  document.body.innerHTML = "";
});

describe("DraftScreen", () => {
  it("draws the desktop's page, keeps what was chosen there, and sends the person's change", async () => {
    let reads = 0;
    link.answer = async (method) => {
      if (method === "draft.read")
        return reads++ === 0 ? record({ page: "<p>the page</p>" }) : new Promise(() => {});
      if (method === "draft.action") return record({ rev: 5, stateRev: 3, state: { radius: 20 } });
      throw new Error(method);
    };
    const host = mount(<DraftScreen conversationId="c1" draft="draft-1" />);
    await flush();
    expect(page.props?.source?.html).toBe("<p>the page</p>");
    act(() => page.props!.onLoadEnd());

    // The page's starting value gives way to the one chosen on the desktop.
    act(() => message({ k: "v", v: { radius: 4 } }));
    expect(page.injected.at(-1)).toContain('{"__mcState":{"radius":12}}');
    act(() => message({ k: "v", v: { radius: 12 } }));

    // Moved by the person's own touch, the change goes to the desktop.
    act(() => page.touch!());
    act(() => message({ k: "v", v: { radius: 20 } }));
    await flush();
    const sent = link.calls.find((call) => call.method === "draft.action");
    expect(sent?.params).toMatchObject({ action: "state", values: { radius: 20 }, generation: 1 });
    expect(host.textContent).toContain("Comment");
  });

  it("ignores what a page posts without the person's touch", async () => {
    let reads = 0;
    link.answer = async (method) =>
      method === "draft.read" && reads++ === 0 ? record({ page: "<p>x</p>" }) : new Promise(() => {});
    mount(<DraftScreen conversationId="c1" draft="draft-1" />);
    await flush();
    act(() => page.props!.onLoadEnd());
    act(() => message({ k: "n", t: "Use this one" }));
    act(() => message({ k: "f", f: { note: "x", tag: "b", name: "n", x: 1, y: 1 } }));
    await flush();
    expect(link.calls.filter((call) => call.method === "draft.action")).toEqual([]);
  });

  it("shows the notes waiting to be sent and sends them from the phone", async () => {
    let reads = 0;
    const note = { note: "Bigger", tag: "b", name: "Pro", x: 1, y: 2 };
    link.answer = async (method) => {
      if (method === "draft.read")
        return reads++ === 0
          ? record({ page: "<p>x</p>", pending: [note], pins: [{ x: 1, y: 2, side: "desktop" }], waiting: true })
          : new Promise(() => {});
      return record({ rev: 6 });
    };
    const host = mount(<DraftScreen conversationId="c1" draft="draft-1" />);
    await flush();
    expect(host.textContent).toContain("Waiting for you");
    const send = host.querySelector<HTMLButtonElement>('[aria-label="Send your notes to the agent"]');
    expect(send?.textContent).toBe("Send 1 note");
    // The desktop's pin is drawn in the phone's copy once it loads.
    act(() => page.props!.onLoadEnd());
    expect(page.injected.some((script) => script.includes('"__mcPin":{"x":1,"y":2}'))).toBe(true);
    act(() => send!.click());
    await flush();
    const actions = link.calls.filter((call) => call.method === "draft.action");
    expect(actions.map((call) => call.params.action)).toEqual(["send"]);
  });

  it("shows a dev server as pictures from the desktop", async () => {
    let reads = 0;
    link.answer = async (method) => {
      if (method === "draft.read")
        return reads++ === 0 ? record({ kind: "url", url: "http://localhost:5173/" }) : new Promise(() => {});
      if (method === "draft.frame")
        return { live: true, frame: "data:image/jpeg;base64,AAAA", width: 1170, height: 2400 };
      throw new Error(method);
    };
    const host = mount(<DraftScreen conversationId="c1" draft="draft-1" />);
    await flush();
    expect(page.props).toBeNull();
    expect(host.textContent).toContain("Live from your desktop · localhost:5173");
    const frame = link.calls.find((call) => call.method === "draft.frame");
    // At the phone's width in pixels.
    expect(frame?.params).toMatchObject({ conversationId: "c1", draft: "draft-1", width: 1170 });
    const picture = host.querySelector("img");
    expect(picture?.getAttribute("src")).toBe("data:image/jpeg;base64,AAAA");
  });
});
