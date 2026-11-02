import { describe, expect, it } from "vitest";

import {
  insert,
  leadingCommand,
  prepend,
  rank,
  score,
  tokenAt,
  type ProviderCommand,
  type Skill,
} from "@/features/skills/slash";

const skill = (name: string, description = "", scope = "project"): Skill => ({
  name,
  description,
  scope,
  source: "agents",
});

describe("tokenAt", () => {
  it("reads a slash word at the cursor", () => {
    expect(tokenAt("/rev", 4)).toEqual({ trigger: "/", start: 0, end: 4, query: "rev" });
    expect(tokenAt("please /rev now", 11)).toEqual({ trigger: "/", start: 7, end: 11, query: "rev" });
  });

  it("covers the whole word when the cursor is inside it", () => {
    expect(tokenAt("/review", 3)).toEqual({ trigger: "/", start: 0, end: 7, query: "re" });
  });

  it("leaves paths, prices, variables and mid-word slashes as text", () => {
    expect(tokenAt("/usr/bin", 8)).toBeNull();
    expect(tokenAt("costs $5", 8)).toBeNull();
    expect(tokenAt("$HOME/x", 7)).toBeNull();
    expect(tokenAt("and/or", 6)).toBeNull();
    expect(tokenAt("plain words", 5)).toBeNull();
  });

  it("finds skills after a dollar", () => {
    expect(tokenAt("use $rev", 8)).toEqual({ trigger: "$", start: 4, end: 8, query: "rev" });
  });

  it("leaves quoted lines alone", () => {
    expect(tokenAt("> /quoted", 9)).toBeNull();
    expect(tokenAt("> quote\n/rev", 12)).toBeNull();
    expect(tokenAt("line\n/rev", 9)).toEqual({ trigger: "/", start: 5, end: 9, query: "rev" });
  });
});

describe("score", () => {
  it("ranks exact over prefix over substring over subsequence", () => {
    const exact = score("review", "review")!;
    const prefix = score("rev", "review")!;
    const inside = score("view", "review")!;
    const loose = score("rvw", "review")!;
    expect(exact).toBeGreaterThan(prefix);
    expect(prefix).toBeGreaterThan(inside);
    expect(inside).toBeGreaterThan(loose);
    expect(score("xyz", "review")).toBeNull();
  });
});

describe("rank", () => {
  const commands: ProviderCommand[] = [
    { name: "review", description: "Provider review" },
    { name: "compact", description: "Shrink context", hint: "[instructions]" },
  ];
  const skills = [skill("review", "Skill review"), skill("deploy", "Ship it")];

  it("offers commands and skills to slash, a same-named skill once as the command", () => {
    const entries = rank("", "/", skills, commands);
    expect(entries.map((entry) => `${entry.kind}:${entry.name}`).sort()).toEqual([
      "command:compact",
      "command:review",
      "skill:deploy",
    ]);
    expect(entries.find((entry) => entry.name === "compact")?.detail).toBe(
      "Shrink context · [instructions]",
    );
  });

  it("offers only skills to a dollar", () => {
    expect(rank("", "$", skills, commands).map((entry) => entry.kind)).toEqual(["skill", "skill"]);
  });

  it("puts the best match first", () => {
    expect(rank("dep", "/", skills, commands)[0]?.name).toBe("deploy");
  });

  it("describes a skill without one by where it lives", () => {
    expect(rank("", "$", [skill("bare", "", "user")], [])[0]?.detail).toBe("Personal · agents");
  });
});

describe("insert and prepend", () => {
  it("replaces the token and leaves the cursor after a space", () => {
    expect(insert("please /rev", { start: 7, end: 11 }, "review")).toEqual({
      text: "please /review ",
      cursor: 15,
    });
    expect(insert("/rev the code", { start: 0, end: 4 }, "review")).toEqual({
      text: "/review the code",
      cursor: 8,
    });
  });

  it("puts a picked name ahead of the draft", () => {
    expect(prepend("", "review")).toEqual({ text: "/review ", cursor: 8 });
    expect(prepend("  the code", "review")).toEqual({ text: "/review the code", cursor: 8 });
  });
});

describe("leadingCommand", () => {
  it("finds a name the message opens with, not a path", () => {
    expect(leadingCommand("/review the code")).toEqual({ name: "review", rest: "the code" });
    expect(leadingCommand("/claude:plan")).toEqual({ name: "claude:plan", rest: "" });
    expect(leadingCommand("/usr/bin is a path")).toBeNull();
    expect(leadingCommand("no command")).toBeNull();
  });
});
