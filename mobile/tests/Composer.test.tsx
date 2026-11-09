import {
  act,
  forwardRef,
  useRef,
  type ComponentProps,
  type ReactNode,
} from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Composer } from "@/features/sessions/Composer";

const runtime = vi.hoisted(() => ({
  os: "android",
  glass: false,
  offline: false,
  values: [] as { get: () => unknown; set: (value: unknown) => void }[],
  photos: vi.fn(async () => []),
  files: vi.fn(async () => []),
  paste: vi.fn(async () => [] as object[]),
  merge: vi.fn((existing: object[], added: object[]) => [...existing, ...added]),
  clipboard: null as ((hasImage: boolean) => void) | null,
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
      children?: ReactNode | ((state: { pressed: boolean }) => ReactNode);
      onPress?: () => void;
      accessibilityLabel?: string;
      disabled?: boolean;
    }) => (
      <button
        aria-label={accessibilityLabel}
        onClick={onPress}
        disabled={disabled}
      >
        {typeof children === "function" ? children({ pressed: false }) : children}
      </button>
    ),
    TextInput: forwardRef<
      HTMLTextAreaElement,
      {
        value: string;
        placeholder: string;
        editable: boolean;
        scrollEnabled?: boolean;
        onChangeText: (text: string) => void;
        onFocus: () => void;
        onBlur: () => void;
      }
    >((props, ref) => (
      <textarea
        ref={ref}
        data-scrolls={String(props.scrollEnabled ?? true)}
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
vi.mock("@/features/sessions/TypingPostureFocus", () => ({
  TypingPostureFocus: ({ enabled, onEnterTypingPosture }: {
    enabled: boolean; onEnterTypingPosture: () => void;
  }) => <button aria-label="Enter typing posture" disabled={!enabled} onClick={onEnterTypingPosture} />,
}));
vi.mock("react-native-reanimated", () => {
  const transition = { duration: () => ({}), delay: () => transition };
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
    Easing: { out: () => null, cubic: null, bezier: () => null },
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
    useDerivedValue: (fn: () => unknown) => ({ get: fn }),
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
    withSpring: (value: unknown) => value,
    withTiming: (value: unknown) => value,
  };
});
vi.mock("expo-glass-effect", () => ({
  isLiquidGlassAvailable: () => runtime.glass,
}));
vi.mock("@/ui/GlassStretchButton", () => ({
  GlassStretchButton: ({ label, onPress }: { label: string; onPress: () => void }) => (
    <button aria-label={label} onClick={onPress} />
  ),
}));
vi.mock("@/ui/GlassBacking", () => ({
  ActionDisc: () => null,
  GlassBacking: () => <i data-testid="glass" />,
}));
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
vi.mock("expo-router", () => ({
  useNavigation: () => ({ addListener: () => () => {} }),
}));
vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ bottom: 24 }),
}));
vi.mock("@/features/connection/ConnectionNotice", () => ({
  useOffline: () => runtime.offline,
  PulsingDot: () => <i data-testid="pulsing-dot" />,
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
  pickMedia: runtime.photos,
  pickFiles: runtime.files,
  pasteImage: runtime.paste,
  watchClipboardImage: (onChange: (hasImage: boolean) => void) => {
    runtime.clipboard = onChange;
    return () => {
      runtime.clipboard = null;
    };
  },
  merge: runtime.merge,
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
vi.mock("@/ui/LiquidView", () => ({
  LiquidView: ({
    onPress,
    accessibilityLabel,
  }: {
    onPress?: () => void;
    accessibilityLabel?: string;
  }) => (onPress ? <button aria-label={accessibilityLabel} onClick={onPress} /> : null),
}));
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
const render = (
  active = true,
  props: Partial<ComponentProps<typeof Composer>> = {},
) =>
  act(() =>
    root.render(
      <Composer
        active={active}
        context="Project"
        branch="main"
        model="GPT"
        onModelPress={model}
        onSend={send}
        busy
        onStop={stop}
        {...props}
      />,
    ),
  );
const type = (text: string) => {
  const input = container.querySelector("textarea")!;
  act(() => {
    input.value = text;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  return input;
};
const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!;
const modelTouches = () =>
  button("Model: GPT").parentElement!.style.pointerEvents;
beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  runtime.os = "android";
  runtime.glass = false;
  runtime.offline = false;
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

it("draws the glass model out with the card, and takes it off screen once it's back", () => {
  runtime.glass = true;
  // The model's glass is the last piece, drawn over the chip's.
  const display = () =>
    [...container.querySelectorAll<HTMLElement>("[data-testid=glass]")].at(-1)!.parentElement!.style
      .display;
  render();
  expect(display()).toBe("none");
  const input = container.querySelector("textarea")!;
  act(() => input.focus());
  render();
  expect(display()).toBe("flex");
  act(() => button("Model: GPT").click());
  expect(model).toHaveBeenCalledOnce();
  act(() => input.blur());
  render();
  expect(display()).toBe("none");
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
  act(() => menuItem("Plan mode").click());
  expect(button("Plan mode, on")).not.toBeNull();
  const input = container.querySelector("textarea")!;
  act(() => {
    input.value = "Open the logs";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => button("Send").click());
  expect(send).toHaveBeenCalledWith("Open the logs", [], "plan", undefined);
  expect(button("Plan mode, on")).toBeNull();
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
  expect(scroll.parentElement!.style.maxHeight).toBe("80%");
  expect((scroll.firstElementChild as HTMLElement).style.paddingBottom).toBe(
    "40px",
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

// The native observer handles posture transitions; the composer focuses its
// existing input, preserving its draft and leaving inactive routes alone.
it("focuses the existing draft on typing posture only for the active route", () => {
  render();
  const input = container.querySelector("textarea")!;
  act(() => {
    input.value = "Unsent draft";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  act(() => button("Enter typing posture").click());
  expect(document.activeElement).toBe(input);
  expect(input.value).toBe("Unsent draft");
  act(() => input.blur());
  render(false);
  act(() => button("Enter typing posture").click());
  expect(document.activeElement).not.toBe(input);
  expect(input.value).toBe("Unsent draft");
});

// A UITextView that can't scroll reports its own frame as its content size.
// The card grows the field to that plus padding, which grows the report again,
// until the cap lets it scroll and the height drops: the card bounces forever.
it("keeps the field scrollable below the height cap, so its height can settle", () => {
  render();
  const input = container.querySelector("textarea")!;
  expect(input.dataset.scrolls).toBe("true");
  type("a short draft");
  expect(input.dataset.scrolls).toBe("true");
});

it("keeps the field open while the desktop is away and holds the message until it is back", async () => {
  runtime.offline = true;
  render();
  const input = container.querySelector("textarea")!;
  expect(input.disabled).toBe(false);
  expect(input.placeholder).toBe("Reconnecting to desktop…");
  expect(container.querySelector('[data-testid="pulsing-dot"]')).toBeNull();

  type("Written on the train");
  expect(input.value).toBe("Written on the train");
  expect(button("Send").disabled).toBe(true);
  expect(container.querySelector('[data-testid="pulsing-dot"]')).not.toBeNull();
  act(() => button("Send").click());
  expect(send).not.toHaveBeenCalled();
  expect(input.value).toBe("Written on the train");

  runtime.offline = false;
  render();
  expect(button("Send").disabled).toBe(false);
  expect(container.querySelector('[data-testid="pulsing-dot"]')).toBeNull();
  await act(async () => button("Send").click());
  expect(send).toHaveBeenCalledWith(
    "Written on the train",
    [],
    undefined,
    undefined,
  );
});

it("can't stop a turn while the desktop is away, but dictation still works", () => {
  runtime.offline = true;
  render();
  expect(button("Stop").disabled).toBe(true);
  act(() => button("Stop").click());
  expect(stop).not.toHaveBeenCalled();

  render(true, { busy: false });
  expect(button("Dictate").disabled).toBe(false);
});

it("leaves a session's error for when the desktop is back, so it can't lock the field", () => {
  runtime.offline = true;
  const notice = { text: "Turn failed", dismiss: vi.fn() };
  render(true, { notice });
  expect(container.querySelector("textarea")!.disabled).toBe(false);
  expect(container.textContent).not.toContain("Turn failed");

  runtime.offline = false;
  render(true, { notice });
  expect(container.querySelector("textarea")!.disabled).toBe(true);
  expect(container.textContent).toContain("Turn failed");
});

it("can't stop a turn behind a session's error", () => {
  render(true, { notice: { text: "Turn failed", dismiss: vi.fn() } });
  expect(button("Stop").disabled).toBe(true);
});

it("keeps the mic round at rest, however wide the composer is", () => {
  render(true, { busy: false });
  const { width, minWidth } = button("Dictate").parentElement!.style;
  expect(width).toBe("0%");
  expect(minWidth).toBe("36px");
});

it("offers to paste a copied image and holds it as a picked photo", async () => {
  const pasted = { id: "p", name: "Pasted image.png", mimeType: "image/png", size: 3, uri: "file:///p.png", data: "AAA" };
  runtime.paste.mockResolvedValueOnce([pasted]);
  render();
  const menuItems = () => [...container.querySelectorAll("button")].map((item) => item.textContent);
  await act(async () => button("Add photos or files").click());
  expect(menuItems()).not.toContain("Paste Image");
  await act(async () => button("Add photos or files").click());

  act(() => runtime.clipboard!(true));
  await act(async () => button("Add photos or files").click());
  expect(menuItems()).toContain("Paste Image");
  await act(async () =>
    [...container.querySelectorAll("button")].find((item) => item.textContent === "Paste Image")!.click(),
  );
  expect(runtime.paste).toHaveBeenCalledWith(10);
  expect(runtime.merge).toHaveBeenCalledWith([], [pasted]);
  // The same strip as a picked photo, removable the same way, and sent with the message.
  expect(button("Remove Pasted image.png")).toBeTruthy();
  await act(async () => button("Send").click());
  expect(send).toHaveBeenCalledWith("", [pasted], undefined, undefined);

  act(() => runtime.clipboard!(false));
  await act(async () => button("Add photos or files").click());
  expect(menuItems()).not.toContain("Paste Image");
});
