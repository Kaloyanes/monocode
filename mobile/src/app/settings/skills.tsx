import { useLocalSearchParams } from "expo-router";
import { Fragment } from "react";
import { ActivityIndicator, StyleSheet, Text, View } from "react-native";

import { SettingsPage } from "@/features/settings/SettingsUI";
import { scopeLabel, skillEntry } from "@/features/skills/slash";
import { SlashRow } from "@/features/skills/SlashRow";
import { useSlashCatalog } from "@/features/skills/useSlashCatalog";
import { projectForRoute } from "@/features/workspace/routing";
import { useWorkspace } from "@/features/workspace/WorkspaceProvider";
import { radii, themedStyles, useColors } from "@/ui/theme";

// The skills the project open under settings can use, as its desktop finds
// them. Read only: they're added, edited and turned off on the desktop.
export default function SkillsScreen() {
  const colors = useColors();
  const styles = useStyles();
  const { projectId } = useLocalSearchParams<{ projectId?: string }>();
  const { projects } = useWorkspace();
  const project = projectForRoute(projects, projectId) ?? projects[0];
  const catalog = useSlashCatalog(project ? { projectId: project.id } : undefined, true);
  const scopes = [...new Set(["project", "user", ...catalog.skills.map((skill) => skill.scope)])];
  const groups = scopes
    .map((scope) => ({
      scope,
      skills: catalog.skills.filter((skill) => skill.scope === scope),
    }))
    .filter((group) => group.skills.length > 0);

  return (
    <SettingsPage onRefresh={catalog.refresh}>
      {project ? <Text style={styles.project}>{project.name}</Text> : null}
      {catalog.loading ? (
        <ActivityIndicator style={styles.status} color={colors.muted} />
      ) : catalog.error ? (
        <Text style={styles.error}>{catalog.error}</Text>
      ) : groups.length === 0 ? (
        <Text style={styles.note}>No skills for this project yet.</Text>
      ) : (
        groups.map((group) => (
          <Fragment key={group.scope}>
            <Text style={styles.heading}>
              {scopeLabel(group.scope)} · {group.skills.length}
            </Text>
            <View style={styles.card}>
              {group.skills.map((skill, index) => (
                <Fragment key={skill.name}>
                  {index > 0 ? <View style={styles.divider} /> : null}
                  <SlashRow entry={skillEntry(skill)} lines={3} />
                </Fragment>
              ))}
            </View>
          </Fragment>
        ))
      )}
      <Text style={styles.note}>
        Type / or $ in a message, or tap + and choose Skills and commands, to use one.
        Add, edit or turn skills off on your desktop, in Settings → Skills.
      </Text>
    </SettingsPage>
  );
}

const useStyles = themedStyles((colors) => ({
  project: {
    color: colors.muted,
    fontSize: 15,
    marginLeft: 16,
  },
  heading: {
    color: colors.muted,
    fontSize: 15,
    marginTop: 24,
    marginBottom: 8,
    marginLeft: 16,
  },
  card: {
    backgroundColor: colors.surface,
    borderRadius: radii.card,
    borderCurve: "continuous",
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  divider: {
    height: StyleSheet.hairlineWidth,
    marginLeft: 48,
    backgroundColor: colors.stroke,
  },
  status: {
    marginTop: 32,
  },
  note: {
    color: colors.muted,
    fontSize: 13,
    lineHeight: 18,
    marginTop: 16,
    marginHorizontal: 16,
  },
  error: {
    color: colors.deletions,
    fontSize: 14,
    lineHeight: 21,
    marginTop: 16,
    marginHorizontal: 16,
  },
}));
