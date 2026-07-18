import { describe, expect, it } from "vitest";
import { appMeta, displayAppText, projectAppMeta, projectAppMetas, sessionAppMeta } from "../app-meta";

describe("appMeta", () => {
  it("maps known apps to canonical slug + label regardless of casing/variants", () => {
    expect(appMeta("codex")).toEqual({ slug: "codex", label: "ChatGPT" });
    expect(appMeta("ChatGPT")).toEqual({ slug: "codex", label: "ChatGPT" });
    expect(appMeta("Claude Code")).toEqual({ slug: "claude", label: "Claude" });
    expect(appMeta("claude_code")).toEqual({ slug: "claude", label: "Claude" });
    expect(appMeta("Antigravity/Gemini")).toEqual({ slug: "antigravity", label: "Antigravity" });
    expect(appMeta("gemini")).toEqual({ slug: "antigravity", label: "Antigravity" });
    expect(appMeta("Hermes/NemoClaw")).toEqual({ slug: "hermes", label: "Hermes" });
    expect(appMeta("Cline")).toEqual({ slug: "cline", label: "Cline" });
    expect(appMeta("cline_tasks")).toEqual({ slug: "cline", label: "Cline" });
    expect(appMeta("Kilo Code")).toEqual({ slug: "kilo", label: "Kilo Code" });
    expect(appMeta("kilocode.kilo-code")).toEqual({ slug: "kilo", label: "Kilo Code" });
  });

  it("falls back to Other for unknown or empty values", () => {
    expect(appMeta(null)).toEqual({ slug: "other", label: "Other" });
    expect(appMeta(undefined)).toEqual({ slug: "other", label: "Other" });
    expect(appMeta("")).toEqual({ slug: "other", label: "Other" });
    expect(appMeta("some-future-ide")).toEqual({ slug: "other", label: "Other" });
  });

  it("normalizes only the complete legacy label without duplicating technical copy", () => {
    expect(displayAppText("Codex")).toBe("ChatGPT");
    expect(displayAppText(" codex ")).toBe("ChatGPT");
    expect(displayAppText("ChatGPT Codex CLI sessions")).toBe("ChatGPT Codex CLI sessions");
    expect(displayAppText("Codex archived sessions")).toBe("Codex archived sessions");
  });

  it("prefers an explicit project.app over the source", () => {
    expect(projectAppMeta({ app: "codex", source: "manual" }).slug).toBe("codex");
    expect(projectAppMeta({ app: null, source: "antigravity-workspace" }).slug).toBe("antigravity");
  });

  it("derives a session's app from its kind", () => {
    expect(sessionAppMeta({ sessionKind: "Codex", sourceKind: "codex_rollout" }).slug).toBe("codex");
    expect(sessionAppMeta({ sessionKind: "", sourceKind: "cursor_workspace" }).slug).toBe("cursor");
  });

  it("projectAppMetas returns every app for the filter (primary + others)", () => {
    const multi = projectAppMetas({ app: "antigravity", source: "registry", apps: ["antigravity", "claude", "codex"] });
    expect(multi.map((m) => m.slug).sort()).toEqual(["antigravity", "claude", "codex"]);
    expect(projectAppMetas({ app: "codex", source: "fixture", apps: ["codex", "claude"] }).map((m) => m.label)).toEqual(["ChatGPT", "Claude"]);
    // Falls back to the single primary when `apps` is absent/empty.
    expect(projectAppMetas({ app: "claude", source: "registry", apps: [] }).map((m) => m.slug)).toEqual(["claude"]);
    expect(projectAppMetas({ app: null, source: "cursor-workspace" }).map((m) => m.slug)).toEqual(["cursor"]);
  });
});
