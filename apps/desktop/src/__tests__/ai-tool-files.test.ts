import { describe, expect, it } from "vitest";
import { aiToolFile } from "../ai-tool-files";

describe("aiToolFile", () => {
  it("recognizes Claude Code steering files (Windows or POSIX paths)", () => {
    expect(aiToolFile("C:\\proj\\CLAUDE.md")?.role).toBe("Project memory");
    expect(aiToolFile("/home/u/proj/CLAUDE.md")?.tool).toBe("Claude Code");
    expect(aiToolFile("C:\\proj\\.claude\\skills\\deploy\\SKILL.md")?.role).toBe("Skill");
    expect(aiToolFile("C:\\proj\\.claude\\agents\\reviewer.md")?.role).toBe("Subagent");
    expect(aiToolFile("C:\\proj\\.claude\\commands\\ship.md")?.role).toBe("Slash command");
    expect(aiToolFile("C:\\proj\\.claude\\settings.json")?.role).toBe("Settings & hooks");
    // Hooks run shell commands — the impact text must say so (it is the risky one).
    expect(aiToolFile("C:\\proj\\.claude\\settings.json")?.impact.toLowerCase()).toContain("shell");
  });

  it("recognizes other tools and cross-tool conventions", () => {
    expect(aiToolFile("C:\\proj\\AGENTS.md")?.tool).toBe("ChatGPT");
    expect(aiToolFile("C:\\Users\\u\\.codex\\config.toml")?.tool).toBe("ChatGPT");
    expect(aiToolFile("C:\\proj\\.cursorrules")?.tool).toBe("Cursor");
    expect(aiToolFile("C:\\proj\\.cursor\\rules\\style.mdc")?.role).toBe("Rule");
    expect(aiToolFile("C:\\proj\\GEMINI.md")?.tool).toBe("Gemini / Antigravity");
    expect(aiToolFile("C:\\proj\\.mcp.json")?.role).toBe("MCP servers");
    expect(aiToolFile("C:\\proj\\.windsurfrules")?.tool).toBe("Windsurf");
  });

  it("recognizes newer steering files, model assets and session logs", () => {
    // Claude local (git-ignored) memory — distinct from CLAUDE.md.
    expect(aiToolFile("C:\\proj\\CLAUDE.local.md")?.role).toBe("Project memory (local)");
    // Cursor's MCP config (project- or user-level).
    expect(aiToolFile("C:\\proj\\.cursor\\mcp.json")?.role).toBe("MCP servers");
    expect(aiToolFile("C:\\Users\\u\\.cursor\\mcp.json")?.tool).toBe("Cursor");
    // Local model weights are assets, not steering.
    expect(aiToolFile("C:\\models\\llama-3.1-8b.Q4_K_M.gguf")?.role).toBe("Model weights");
    expect(aiToolFile("D:\\hf\\model.safetensors")?.tool).toBe("Local models");
    // Codex session transcript (rollout).
    expect(aiToolFile("C:\\Users\\u\\.codex\\sessions\\rollout-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl")?.role).toBe("Session transcript");
    // Aider working files fall through the config check to the catch-all.
    expect(aiToolFile("C:\\proj\\.aider.conf.yml")?.role).toBe("Config");
    expect(aiToolFile("C:\\proj\\.aider.chat.history.md")?.role).toBe("Working file");
  });

  it("returns null for ordinary files and empty input", () => {
    expect(aiToolFile("C:\\proj\\src\\main.rs")).toBeNull();
    expect(aiToolFile("C:\\proj\\README.md")).toBeNull();
    expect(aiToolFile("")).toBeNull();
    expect(aiToolFile(null)).toBeNull();
  });
});
