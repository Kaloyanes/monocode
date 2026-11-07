import { Button, Label, Picker, Toggle } from "@expo/ui/swift-ui";
import { pickerStyle, tag } from "@expo/ui/swift-ui/modifiers";
import { Platform } from "react-native";

import { SEARCH_BAR } from "@/features/files/FileSearchBar";
import type { Harness, SessionSummary } from "@/features/workspace/types";
import { ChoiceMenu } from "@/ui/ChoiceMenu";
import { GlassMenuButton } from "@/ui/GlassMenuButton";
import { Icon } from "@/ui/Icon";
import { LiquidView } from "@/ui/LiquidView";
import { useColors } from "@/ui/theme";

export type SessionFilter = {
  /** Only the sessions working now. */
  working: boolean;
  /** Only one agent's sessions. */
  harness: Harness | null;
};

export const NO_FILTER: SessionFilter = { working: false, harness: null };

const HARNESS_NAMES: Record<Harness, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  opencode: "OpenCode",
  pi: "Pi",
  omp: "OMP",
  fx: "FX",
  hermes: "Hermes",
  antigravity: "Antigravity",
  devin: "Devin",
};

export function filterSessions(sessions: SessionSummary[], filter: SessionFilter) {
  if (!filter.working && !filter.harness) return sessions;
  return sessions.filter(
    (session) =>
      (!filter.working || session.busy) &&
      (!filter.harness || session.harness === filter.harness),
  );
}

/**
 * The round button beside the Sessions page's search: a native menu to show
 * only the sessions working now, or one agent's, from those in the list, and
 * to show or hide the pinned Monos group.
 */
export function SessionFilterMenu({
  value,
  onChange,
  harnesses,
  monos,
  onOpenArchived,
}: {
  value: SessionFilter;
  onChange: (value: SessionFilter) => void;
  /** The agents the list has sessions from. */
  harnesses: Harness[];
  /** The pinned Monos group, offered only on a page that has Monos. */
  monos?: { shown: boolean; onChange: (shown: boolean) => void };
  /** Opens the archive; offered only where the desktop serves it. */
  onOpenArchived?: () => void;
}) {
  const colors = useColors();
  const on = value.working || !!value.harness || (!!monos && !monos.shown);
  // The SwiftUI menu has no Android view, so Android lists the groups in a sheet.
  if (Platform.OS === "android") return (
    <ChoiceMenu label={on ? "Filter sessions, on" : "Filter sessions"} title="Filter" sections={[
      [
        { label: "All sessions", icon: "inbox", selected: !value.working, onPress: () => onChange({ ...value, working: false }) },
        { label: "Working now", icon: "automation", selected: value.working, onPress: () => onChange({ ...value, working: true }) },
      ],
      harnesses.length > 1 || value.harness ? [
        { label: "Any agent", selected: !value.harness, onPress: () => onChange({ ...value, harness: null }) },
        ...harnesses.map((harness) => ({
          label: HARNESS_NAMES[harness],
          selected: value.harness === harness,
          onPress: () => onChange({ ...value, harness }),
        })),
      ] : [],
      monos ? [
        { label: "Monos", icon: "people", selected: monos.shown, onPress: () => monos.onChange(!monos.shown) },
      ] : [],
      onOpenArchived ? [
        { label: "Archived", icon: "archive", onPress: onOpenArchived },
      ] : [],
    ]}>
      <LiquidView borderRadius={SEARCH_BAR / 2} innerStyle={{ width: SEARCH_BAR, height: SEARCH_BAR, alignItems: "center", justifyContent: "center" }}>
        <Icon name="filter" size={20} color={colors.content} />
      </LiquidView>
    </ChoiceMenu>
  );
  return (
    <GlassMenuButton
      icon="filter"
      symbol="slider.horizontal.3"
      label="Filter sessions"
      size={SEARCH_BAR}
      on={on}
    >
      <Picker
        selection={value.working ? "working" : "all"}
        onSelectionChange={(status) => onChange({ ...value, working: status === "working" })}
        modifiers={[pickerStyle("inline")]}
      >
        <Label title="All sessions" systemImage="tray.full" modifiers={[tag("all")]} />
        <Label title="Working now" systemImage="bolt" modifiers={[tag("working")]} />
      </Picker>
      {harnesses.length > 1 || value.harness ? (
        <Picker
          selection={value.harness ?? "any"}
          onSelectionChange={(harness) =>
            onChange({
              ...value,
              harness: harness === "any" ? null : (harness as Harness),
            })
          }
          modifiers={[pickerStyle("inline")]}
        >
          <Label title="Any agent" modifiers={[tag("any")]} />
          {harnesses.map((harness) => (
            <Label key={harness} title={HARNESS_NAMES[harness]} modifiers={[tag(harness)]} />
          ))}
        </Picker>
      ) : null}
      {onOpenArchived ? (
        <Button label="Archived" systemImage="archivebox" onPress={onOpenArchived} />
      ) : null}
      {monos ? (
        <Toggle
          label="Monos"
          systemImage="person.2"
          isOn={monos.shown}
          onIsOnChange={monos.onChange}
        />
      ) : null}
    </GlassMenuButton>
  );
}

