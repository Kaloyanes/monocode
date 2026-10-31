/** A skill the desktop finds for a working copy (`project.skills`). */
export type Skill = {
  name: string;
  description: string;
  /** "project" for the repo's own, "user" for personal ones. */
  scope: string;
  /** The tool folder it came from, such as "agents" or "claude". */
  source: string;
};

/** A provider's own command a chat has heard of (`session.commands`). */
export type ProviderCommand = {
  name: string;
  description: string;
  /** What it takes after its name, when the provider says. */
  hint?: string | null;
};

/** One row the `/` picker or the skills sheet offers. */
export type SlashEntry = {
  kind: "skill" | "command";
  name: string;
  detail: string;
};

/**
 * The `/name` or `$name` being typed at the cursor, as the desktop's
 * `composer::token_at` reads one: a word starting with `/` or `$`, where `$`
 * only finds skills and prices and shell variables stay text.
 */
export type SlashToken = {
  trigger: "/" | "$";
  start: number;
  end: number;
  query: string;
};

export function tokenAt(text: string, cursor: number): SlashToken | null {
  if (cursor < 0 || cursor > text.length) return null;
  let start = cursor;
  while (start > 0 && !/\s/.test(text[start - 1]!)) start--;
  const trigger = text[start];
  if (trigger !== "/" && trigger !== "$") return null;
  const query = text.slice(start + 1, cursor);
  if (/[@\n\r/.\\]/.test(query)) return null;
  if (trigger === "$" && (query.includes("$") || /^[^A-Za-z]/.test(query))) return null;
  // Quoted lines stay text. As the desktop reads it, the line checked is the
  // one the token starts on, or the line above when it opens its line.
  const lineStart = text.lastIndexOf("\n", start - 1) + 1;
  let checked = text.slice(lineStart, start);
  if (start > 0 && start === lineStart) {
    const above = text.slice(0, lineStart - 1);
    checked = above.slice(above.lastIndexOf("\n") + 1);
  }
  if (checked.trimStart().startsWith(">")) return null;
  let end = cursor;
  while (end < text.length && !/\s/.test(text[end]!)) end++;
  return { trigger, start, end, query };
}

/** The desktop's fuzzy score (`composer::score`): exact, then prefix, then substring, then subsequence. */
export function score(query: string, value: string): number | null {
  const q = query.toLowerCase();
  const v = value.toLowerCase();
  if (!q) return 0;
  if (v === q) return 10_000;
  const leaf = v.slice(v.lastIndexOf("/") + 1);
  if (leaf.startsWith(q)) return 5_000 - v.length;
  const at = v.indexOf(q);
  if (at >= 0) return 3_000 - at - v.length;
  let rest = v;
  let gaps = 0;
  for (const ch of q) {
    const found = rest.indexOf(ch);
    if (found < 0) return null;
    gaps += found;
    rest = rest.slice(found + ch.length);
  }
  return 1_000 - gaps - v.length;
}

export function skillEntry(skill: Skill): SlashEntry {
  return {
    kind: "skill",
    name: skill.name,
    detail: skill.description || `${scopeLabel(skill.scope)} · ${skill.source}`,
  };
}

export function commandEntry(command: ProviderCommand): SlashEntry {
  return {
    kind: "command",
    name: command.name,
    detail: [command.description, command.hint ?? ""].filter(Boolean).join(" · "),
  };
}

/**
 * What a token offers, best first: `/` finds the provider's commands and
 * skills, `$` skills alone. A command and a skill of one name show once, the
 * command's, since the provider takes `/name` before MonoCode would.
 */
export function rank(
  query: string,
  trigger: "/" | "$",
  skills: Skill[],
  commands: ProviderCommand[],
): SlashEntry[] {
  const offered = trigger === "/" ? commands.map(commandEntry) : [];
  const taken = new Set(offered.map((entry) => entry.name.toLowerCase()));
  for (const skill of skills) {
    if (!taken.has(skill.name.toLowerCase())) offered.push(skillEntry(skill));
  }
  return offered
    .map((entry) => ({ entry, score: score(query, `${entry.name} ${entry.detail}`) }))
    .filter((match): match is { entry: SlashEntry; score: number } => match.score !== null)
    .sort((a, b) => b.score - a.score || a.entry.name.localeCompare(b.entry.name))
    .map((match) => match.entry);
}

/** `text` with the token replaced by `/name `, and where the cursor goes. */
export function insert(
  text: string,
  token: Pick<SlashToken, "start" | "end">,
  name: string,
): { text: string; cursor: number } {
  const after = text.slice(token.end);
  const inserted = `/${name}${after.startsWith(" ") ? "" : " "}`;
  const next = text.slice(0, token.start) + inserted + after;
  const cursor = token.start + inserted.length + (after.startsWith(" ") ? 1 : 0);
  return { text: next, cursor };
}

/** `/name` added where nothing is being typed: at the start, ahead of any words. */
export function prepend(text: string, name: string): { text: string; cursor: number } {
  const rest = text.trimStart();
  const next = `/${name} ${rest}`;
  return { text: next, cursor: name.length + 2 };
}

/**
 * The `/name` a sent message opens with, shaped as skill and command names
 * are, so a path such as `/usr/bin` stays text.
 */
export function leadingCommand(text: string) {
  const match = /^\/([A-Za-z0-9][\w:.-]*)(?=\s|$)/.exec(text);
  if (!match) return null;
  return { name: match[1]!, rest: text.slice(match[0].length).trimStart() };
}

export function scopeLabel(scope: string) {
  return scope === "project" ? "Project" : scope === "user" ? "Personal" : scope;
}
