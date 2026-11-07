import type { Button } from "@expo/ui/swift-ui";
import { isLiquidGlassAvailable } from "expo-glass-effect";
import { useEffect, type ComponentProps } from "react";
import { Pressable, Text, type LayoutChangeEvent, type StyleProp, type ViewStyle } from "react-native";
import Animated, {
  Easing,
  FadeIn,
  interpolate,
  useAnimatedStyle,
  useDerivedValue,
  useSharedValue,
  withTiming,
  type AnimatedStyle,
  type SharedValue,
} from "react-native-reanimated";

import type { ComposerMode, Harness } from "@/features/workspace/types";
import { GlassBacking } from "@/ui/GlassBacking";
import { Icon } from "@/ui/Icon";
import { themedStyles, useColors, useScheme, type Scheme } from "@/ui/theme";

import { ProviderLogo } from "./ProviderLogo";

/** The desktop's + menu modes, in its order and with its words and hues. */
export const MODES: {
  id: ComposerMode;
  label: string;
  hint: string;
  systemImage: ComponentProps<typeof Button>["systemImage"];
  tint: Record<Scheme, string>;
}[] = [
  {
    id: "plan",
    label: "Plan mode",
    hint: "Review a plan before building",
    systemImage: "lightbulb",
    tint: { dark: "#fde047", light: "#a16207" },
  },
];

/** The chip drawing out from under the model, or back under it. */
const CHIP_DRAW = { duration: 240, easing: Easing.out(Easing.cubic) };
const CHIP_TEXT_FADE = 160;
/** How far the card has opened when the glass is fully out, ahead of the words on it. */
const DRAWN_BY = 0.75;
const GAP = 8;

type Frame = { x: number; width: number };

/**
 * The model and, when one is on, the mode chip, along the composer's bottom
 * row past the add button.
 *
 * Where there's Liquid Glass they're glass, the add button's material. Glass
 * can't be faded (see LiquidView's GlassPresence), so it travels instead: each
 * piece draws out from behind what's beside it as the card opens, and back in
 * as it folds or a recording takes the row. What's written on them does fade,
 * and lays out on its own row, so the glass follows the frames that row gives
 * it, truncation included. A piece that's fully away is taken off screen,
 * because glass stacked under the add button lightens it. The touches are the
 * row's own, as they are without glass; the glass under it takes none.
 */
export function ComposerPills({
  progress,
  dictation,
  written,
  size,
  edge,
  model,
  effort,
  harness,
  mode,
  onModelPress,
  onClearMode,
}: {
  /** The card's morph, 0 as a pill to 1 open. */
  progress: SharedValue<number>;
  /** A recording taking over the row, 0 to 1. */
  dictation: SharedValue<number>;
  /** Fades what's written here along with the rest of the card. */
  written: StyleProp<AnimatedStyle<StyleProp<ViewStyle>>>;
  size: number;
  /** Gap from the composer's edge to its round buttons. */
  edge: number;
  model: string;
  effort?: string;
  harness?: Harness;
  mode: ComposerMode | null;
  onModelPress?: () => void;
  onClearMode: () => void;
}) {
  const styles = useStyles();
  const colors = useColors();
  const scheme = useScheme();
  const lane = edge + size + GAP;
  const glass = isLiquidGlassAvailable();

  const name = `Model: ${model || "Desktop default"}${effort ? `, ${effort} effort` : ""}`;
  const modelHint = "Choose the model, account, effort and permissions";
  const entry = MODES.find((candidate) => candidate.id === mode);
  const chipName = entry ? `${entry.label}, on` : "";
  const chipHint = "Turns it off for this message";

  const modelFrame = useSharedValue<Frame>({ x: 0, width: 0 });
  const chipFrame = useSharedValue<Frame>({ x: 0, width: 0 });
  const chipOn = useSharedValue(mode ? 1 : 0);
  useEffect(() => {
    chipOn.set(withTiming(mode ? 1 : 0, CHIP_DRAW));
  }, [mode, chipOn]);

  const out = useDerivedValue(
    () => interpolate(progress.get(), [0, DRAWN_BY], [0, 1], "clamp") * (1 - dictation.get()),
  );
  const modelGlass = useAnimatedStyle(() => {
    const t = out.get();
    const { x, width } = modelFrame.get();
    return {
      display: t === 0 ? "none" : "flex",
      left: edge + (lane + x - edge) * t,
      width: size + (width - size) * t,
    };
  });
  const chipGlass = useAnimatedStyle(() => {
    const t = out.get() * chipOn.get();
    const { x, width } = chipFrame.get();
    const behind = edge + (lane + modelFrame.get().x - edge) * out.get();
    return {
      display: t === 0 ? "none" : "flex",
      left: behind + (lane + x - behind) * t,
      width: size + (width - size) * t,
    };
  });

  const track = (frame: SharedValue<Frame>) => (event: LayoutChangeEvent) => {
    const { x, width } = event.nativeEvent.layout;
    frame.set({ x, width });
  };

  const modelText = (
    <>
      {harness ? <ProviderLogo harness={harness} size={16} color={colors.content} /> : null}
      <Text style={styles.model} numberOfLines={1}>
        {model || "Desktop default"}
        {effort ? <Text style={styles.effort}> {effort}</Text> : null}
      </Text>
      <Icon name="chevronDown" size={14} color={colors.muted} />
    </>
  );
  const chipText = entry ? (
    <>
      <Icon name={entry.id} size={16} color={entry.tint[scheme]} />
      <Text style={styles.modeLabel} numberOfLines={1}>
        {entry.label.replace(" mode", "")}
      </Text>
      <Icon name="close" size={12} color={colors.muted} />
    </>
  ) : null;

  return (
    <>
      {glass ? (
        <>
          {/* The chip first, so it comes out from under the model. */}
          <Animated.View style={[styles.glass, { bottom: edge, height: size }, chipGlass]}>
            <GlassBacking />
          </Animated.View>
          <Animated.View style={[styles.glass, { bottom: edge, height: size }, modelGlass]}>
            <GlassBacking />
          </Animated.View>
        </>
      ) : null}
      <Animated.View style={[styles.row, { left: lane, right: lane, bottom: edge }, written]}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={name}
          accessibilityHint={modelHint}
          onLayout={track(modelFrame)}
          onPress={onModelPress}
          style={({ pressed }) => [
            styles.modelPill,
            !glass && styles.filled,
            { height: size, borderRadius: size / 2 },
            pressed && styles.pressed,
          ]}
        >
          {modelText}
        </Pressable>
        {entry ? (
          <Animated.View
            entering={glass ? FadeIn.delay(CHIP_TEXT_FADE / 2).duration(CHIP_TEXT_FADE) : undefined}
            onLayout={track(chipFrame)}
            style={styles.modeSlot}
          >
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={chipName}
              accessibilityHint={chipHint}
              hitSlop={4}
              onPress={onClearMode}
              style={({ pressed }) => [
                styles.modePill,
                !glass && styles.filled,
                { height: size, borderRadius: size / 2 },
                pressed && styles.pressed,
              ]}
            >
              {chipText}
            </Pressable>
          </Animated.View>
        ) : null}
      </Animated.View>
    </>
  );
}

const useStyles = themedStyles((colors) => ({
  row: {
    position: "absolute",
    flexDirection: "row",
  },
  glass: {
    position: "absolute",
  },
  // Shaped like the round buttons beside it: their height and full rounding.
  modelPill: {
    flexShrink: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    paddingHorizontal: 12,
  },
  modeSlot: {
    flexShrink: 0,
    marginLeft: 6,
  },
  modePill: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    paddingHorizontal: 12,
  },
  filled: {
    backgroundColor: colors.fill,
  },
  modeLabel: {
    color: colors.content,
    fontSize: 15,
    fontWeight: "500",
  },
  model: {
    flexShrink: 1,
    color: colors.content,
    fontSize: 15,
    fontWeight: "500",
  },
  effort: {
    color: colors.muted,
    fontWeight: "400",
  },
  pressed: {
    opacity: 0.6,
  },
}));
