import { act, forwardRef, useRef, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Composer } from "@/features/sessions/Composer";

const runtime = vi.hoisted(() => ({
  os: "android",
  values: [] as { get: () => unknown; set: (value: unknown) => void }[],
  photos: vi.fn(async () => []),
  files: vi.fn(async () => []),
}));
vi.mock("react-native", () => {
  const View = ({
    children,
    style,
  }: {
    children?: ReactNode;
    style?: object;
  }) => <div style={style}>{children}</div>;
  return {
    Platform: {
      get OS() {
        return runtime.os;
      },
    },
    View,
    ScrollView: ({
      children,
      style,
      contentContainerStyle,
    }: {
      children?: ReactNode;
      style: object;
      contentContainerStyle: object;
    }) => (
      <div data-scroll style={style}>
        <div style={contentContainerStyle}>{children}</div>
      </div>
    ),
    Text: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
    Image: () => null,
    Alert: { alert: vi.fn() },
    StyleSheet: { absoluteFill: {} },
    Modal: ({
      visible,
      children,
    }: {
      visible: boolean;
      children?: ReactNode;
    }) => (visible ? <section>{children}</section> : null),
    Pressable: ({
      children,
      onPress,
      accessibilityLabel,
      disabled,
    }: {
      children?: ReactNode;
      onPress?: () => void;
      accessibilityLabel?: string;
      disabled?: boolean;
    }) => (
      <button
        aria-label={accessibilityLabel}
        onClick={onPress}
        disabled={disabled}
      >
        {children}
      </button>
    ),
    TextInput: forwardRef<
      HTMLTextAreaElement,
      {
        value: string;
        placeholder: string;
        editable: boolean;
        onChangeText: (text: string) => void;
        onFocus: () => void;
        onBlur: () => void;
      }
    >((props, ref) => (
      <textarea
        ref={ref}
        value={props.value}
        placeholder={props.placeholder}
        disabled={!props.editable}
        onFocus={props.onFocus}
        onBlur={props.onBlur}
        onInput={(event) => props.onChangeText(event.currentTarget.value)}
      />
    )),
  };
});
vi.mock("react-native-reanimated", () => {
  const transition = { duration: () => ({}) };
  return {
    default: {
      View: ({
        children,
        style,
      }: {
        children?: ReactNode;
        style: object | object[];
      }) => {
        const styles = (Array.isArray(style) ? style : [style]).map((item) =>
          typeof item === "function" ? item() : item,
        );
        return <div style={Object.assign({}, ...styles)}>{children}</div>;
      },
    },
    Easing: { out: () => null, cubic: null },
    FadeIn: transition,
    FadeOut: transition,
    interpolate: (
      value: number,
      [a, b]: number[],
      [c, d]: number[],
      clamp?: string,
    ) => {
      const fraction = (value - a) / (b - a);
      return (
        c + (d - c) * (clamp ? Math.min(1, Math.max(0, fraction)) : fraction)
      );
    },
    useAnimatedStyle: (fn: () => object) => fn,
    useSharedValue: (initial: unknown) => {
      const ref = useRef<{
        get: () => unknown;
        set: (value: unknown) => void;
      } | null>(null);
      if (!ref.current) {
        let value = initial;
        ref.current = {
          get: () => value,
          set: (next) => {
            value = next;
          },
        };
        runtime.values.push(ref.current);
      }
      return ref.current;
    },
    withTiming: (value: unknown) => value,
  };
});
vi.mock("@expo/ui/swift-ui", () => ({
  Button: () => null,
  Divider: () => null,
  Host: () => null,
  HStack: () => null,
  Menu: () => null,
  Spacer: () => null,
}));
vi.mock("@expo/ui/swift-ui/modifiers", () => ({
  accessibilityLabel: vi.fn(),
  contentShape: vi.fn(),
  frame: vi.fn(),
  shapes: { circle: vi.fn() },
}));
vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ bottom: 24 }),
}));
vi.mock("@/features/connection/ConnectionNotice", () => ({
  useOffline: () => false,
  PulsingDot: () => null,
}));
vi.mock("@/features/files/FileTypeIcon", () => ({ FileTypeIcon: () => null }));
vi.mock("@/features/sessions/SessionScreen", () => ({
  COMPOSER_INPUT_ID: "composer",
}));
vi.mock("@/features/sessions/ProviderLogo", () => ({
  ProviderLogo: () => null,
}));
vi.mock("@/features/sessions/DictationBar", () => ({
  DictationBar: () => null,
}));
vi.mock("@/features/sessions/dictation", () => ({
  canDictate: () => false,
  insertDictation: vi.fn(),
}));
vi.mock("@/features/sessions/attachments", () => ({
  MAX_ATTACHMENTS: 10,
  pickPhotos: runtime.photos,
  pickFiles: runtime.files,
  merge: vi.fn(),
  isImage: () => false,
}));
vi.mock("@/ui/Icon", () => ({ Icon: () => null }));
vi.mock("@/features/sessions/useComposerDraft", () => ({
  useComposerDraft: () => ({ typed: () => {}, flush: () => {}, sent: () => undefined }),
}));
vi.mock("@/features/skills/SkillsSheet", () => ({
  SkillsSheet: ({ isPresented, onPick }: { isPresented: boolean; onPick: (name: string) => void }) =>
    isPresented ? <button aria-label="Pick deploy" onClick={() => onPick("deploy")} /> : null,
}));
vi.mock("@/features/skills/useSlashCatalog", () => ({
  useSlashCatalog: (_scope: unknown, active: boolean) => ({
    skills: active
      ? [
          { name: "review", description: "Review the diff", scope: "project", source: "agents" },
          { name: "deploy", description: "Ship it", scope: "user", source: "agents" },
        ]
      : [],
    commands: active ? [{ name: "compact", description: "Shrink context" }] : [],
    loading: false,
    error: null,
    refresh: () => {},
  }),
}));
vi.mock("@/ui/LiquidView", () => ({ LiquidView: () => null }));
vi.mock("@/ui/theme", () => ({
  useColors: () => ({}),
  useScheme: () => "dark",
  themedStyles: (fn: (colors: object) => object) => () => fn({}),
}));

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
const model = vi.fn();
const send = vi.fn();
const stop = vi.fn(async () => {});
const render = () =>
  act(() =>
    root.render(
      <Composer
        context="Project"
        branch="main"
        model="GPT"
        onModelPress={model}
        onSend={send}
        busy
        onStop={stop}
      />,
    ),
  );
const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!;
const modelTouches = () =>
  button("Model: GPT").parentElement!.style.pointerEvents;
beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  runtime.os = "android";
  runtime.values = [];
  vi.clearAllMocks();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

it("excludes the hidden model from Android touches, then permits only its control after expansion", async () => {
  render();
  expect(modelTouches()).toBe("none");
  const input = container.querySelector("textarea")!;
  act(() => input.focus());
  expect(document.activeElement).toBe(input);
  render();
  expect(modelTouches()).toBe("box-none");
  act(() => input.blur());
  render();
  expect(modelTouches()).toBe("none");
  act(() => input.focus());
  act(() => {
    input.value = "Hello";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(input.value).toBe("Hello");
  expect(model).not.toHaveBeenCalled();
  act(() => input.blur());
  render();
  expect(modelTouches()).toBe("box-none");
  act(() => button("Model: GPT").click());
  expect(model).toHaveBeenCalledOnce();
  await act(async () => button("Send").click());
  expect(send).toHaveBeenCalledWith("Hello", [], undefined, undefined);
  expect(modelTouches()).toBe("none");
});

it("sends the mode picked from the add menu with the next message only", async () => {
  act(() =>
    root.render(
      <Composer context="Project" branch="main" model="GPT" onSend={send} modes />,
    ),
  );
  const menuItem = (label: string) =>
    [...container.querySelectorAll("button")].find(
      (item) => item.textContent?.replace("✓  ", "") === label,
    )!;
  act(() => button("Add files or pick a mode").click());
  act(() => menuItem("Operator").click());
  expect(button("Operator, on")).not.toBeNull();
  const input = container.querySelector("textarea")!;
  act(() => {
    input.value = "Open the logs";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => button("Send").click());
  expect(send).toHaveBeenCalledWith("Open the logs", [], "operator", undefined);
  expect(button("Operator, on")).toBeNull();
});

it("suggests skills and commands for a slash word and puts the pick in its place", async () => {
  (HTMLTextAreaElement.prototype as unknown as { setSelection: () => void }).setSelection = () => {};
  act(() =>
    root.render(
      <Composer context="Project" branch="main" model="GPT" onSend={send} slash={{ projectId: "p" }} />,
    ),
  );
  const input = container.querySelector("textarea")!;
  const type = (value: string) =>
    act(() => {
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  type("please /rev");
  expect(button("Skill review")).not.toBeNull();
  expect(container.querySelector('[aria-label="Command compact"]')).toBeNull();
  act(() => button("Skill review").click());
  expect(input.value).toBe("please /review ");
  expect(container.querySelector('[aria-label="Skill review"]')).toBeNull();

  // `$` finds skills only; a price stays text.
  type("$");
  expect(container.querySelector('[aria-label="Command compact"]')).toBeNull();
  type("costs $5");
  expect(container.querySelector('[aria-label="Skill deploy"]')).toBeNull();
  type("/");
  expect(button("Command compact")).not.toBeNull();
});

it("lists skills from the add menu and leads the draft with the pick", async () => {
  act(() =>
    root.render(
      <Composer context="Project" branch="main" model="GPT" onSend={send} slash={{ projectId: "p" }} />,
    ),
  );
  const input = container.querySelector("textarea")!;
  act(() => {
    input.value = "the main branch";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  act(() => button("Add photos or files").click());
  act(() =>
    [...container.querySelectorAll("button")]
      .find((item) => item.textContent === "Skills and commands")!
      .click(),
  );
  act(() => button("Pick deploy").click());
  expect(input.value).toBe("/deploy the main branch");
  await act(async () => button("Send").click());
  expect(send).toHaveBeenCalledWith("/deploy the main branch", [], undefined, undefined);
});

it("blocks model hits throughout a morph and dictation, and preserves the stop action", () => {
  render();
  // Shared values: focused, morph progress, dictation progress.
  for (const progress of [0, 0.2, 0.8, 0.999]) {
    runtime.values[1].set(progress);
    render();
    expect(modelTouches()).toBe("none");
  }
  runtime.values[1].set(1);
  runtime.values[2].set(0.2);
  render();
  expect(modelTouches()).toBe("none");
  runtime.values[2].set(0);
  render();
  act(() => button("Stop").click());
  expect(stop).toHaveBeenCalledOnce();
});

it("keeps the iOS hit-testing styles unchanged", () => {
  runtime.os = "ios";
  render();
  expect(modelTouches()).toBe("");
});

it("opens a content-sized attachment menu with safe-area padding and retains both actions", async () => {
  render();
  await act(async () => button("Add photos or files").click());
  const scroll = container.querySelector<HTMLElement>("[data-scroll]")!;
  expect(scroll.style.flexGrow).toBe("0");
  expect(scroll.style.maxHeight).toBe("75%");
  expect((scroll.firstElementChild as HTMLElement).style.paddingBottom).toBe(
    "44px",
  );
  await act(async () =>
    [...container.querySelectorAll("button")]
      .find((item) => item.textContent === "Photo Library")!
      .click(),
  );
  expect(runtime.photos).toHaveBeenCalledWith(10);
  expect(container.querySelector("section")).toBeNull();
  await act(async () => button("Add photos or files").click());
  await act(async () =>
    [...container.querySelectorAll("button")]
      .find((item) => item.textContent === "Files")!
      .click(),
  );
  expect(runtime.files).toHaveBeenCalledWith(10);
});
