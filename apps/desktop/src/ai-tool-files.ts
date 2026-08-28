// Recognizes the AI-tool files coding agents generate (Claude Code, ChatGPT, Cursor,
// Gemini/Antigravity, Hermes, openclaw and a few cross-tool conventions): the "steering"
// files that actually direct the agent, plus the AI assets a user is likely to find right
// beside them — local model weights and session transcripts. Explains, in plain language,
// what each one is and its impact. Pure, offline, path-only classification — no disk reads —
// so it can label a file in the tree and the File Details panel instantly. Surfacing this is
// the whole point: a user who didn't write these files can see which ones steer the agent
// (and which run commands), and which are just large assets or session logs.

export interface AiToolFileInfo {
  /** The tool that reads this file, e.g. "Claude Code", "ChatGPT". */
  tool: string;
  /** Short role label, e.g. "Project memory", "Skill", "Subagent". */
  role: string;
  /** One plain sentence: what it does and why it matters. */
  impact: string;
  /** When/where it applies, e.g. "Always in context", "On demand", "Project". */
  scope: string;
}

function normalize(path: string): string {
  return path.replace(/\\/g, "/").toLowerCase();
}

function baseName(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut >= 0 ? path.slice(cut + 1) : path;
}

/** Identify an AI-tool steering file from its path, or null for an ordinary file. */
export function aiToolFile(rawPath: string | null | undefined): AiToolFileInfo | null {
  if (!rawPath) return null;
  const p = normalize(rawPath);
  const name = baseName(p);
  const inClaude = p.includes("/.claude/");

  // --- Claude Code (paths first, most specific) ---
  if (inClaude && p.includes("/skills/") && name === "skill.md") {
    return { tool: "Claude Code", role: "Skill", impact: "A packaged capability (instructions, optional scripts) Claude loads on demand when the task matches its description.", scope: "Project · on demand" };
  }
  if (inClaude && p.includes("/agents/") && name.endsWith(".md")) {
    return { tool: "Claude Code", role: "Subagent", impact: "An isolated assistant Claude can delegate a side task to; it runs in its own context window and returns a summary.", scope: "Project" };
  }
  if (inClaude && p.includes("/commands/") && name.endsWith(".md")) {
    return { tool: "Claude Code", role: "Slash command", impact: "A reusable prompt invoked as /name to trigger a saved workflow.", scope: "Project" };
  }
  if (inClaude && p.includes("/rules/") && name.endsWith(".md")) {
    return { tool: "Claude Code", role: "Rule", impact: "A constraint or convention applied globally or to matching file paths, re-injected after context compaction.", scope: "Project / User" };
  }
  if (inClaude && p.includes("/output-styles/") && name.endsWith(".md")) {
    return { tool: "Claude Code", role: "Output style", impact: "Replaces Claude's system prompt to change its role and behavior — the strongest steering of all.", scope: "Project" };
  }
  if (inClaude && (name === "settings.json" || name === "settings.local.json")) {
    return { tool: "Claude Code", role: "Settings & hooks", impact: "Configuration plus hooks — hooks run shell commands deterministically on events (edits, tool calls), so this file can execute code.", scope: "Project / User" };
  }
  if (inClaude && p.includes("/hooks/")) {
    return { tool: "Claude Code", role: "Hook", impact: "A command that fires deterministically on a lifecycle event and can run shell commands outside Claude's reasoning.", scope: "Project / User" };
  }
  if (name === "claude.md") {
    return { tool: "Claude Code", role: "Project memory", impact: "Loaded as the project's rules, layout and conventions; it steers every response. At the repo root it is always in context; in a subfolder it loads on demand.", scope: "Always in context (root)" };
  }
  if (name === "claude.local.md") {
    return { tool: "Claude Code", role: "Project memory (local)", impact: "Your personal, git-ignored Claude memory for this project — the same steering as CLAUDE.md but private to you and not shared with the team.", scope: "Always in context (root) · local" };
  }
  if (name === ".claude.json") {
    return { tool: "Claude Code", role: "Project registry", impact: "Claude's per-user list of projects it has run in (with per-project settings). Not project steering — it is how the catalog knows a folder is a Claude project.", scope: "User" };
  }

  // --- ChatGPT (Codex CLI store and conventions) ---
  if (name === "agents.md" || name === "agents.override.md") {
    return { tool: "ChatGPT", role: "Agent instructions", impact: "ChatGPT's local CLI project instructions and directives (the AGENTS.md convention, also read by several other tools).", scope: "Project" };
  }
  if (p.endsWith("/.codex/config.toml")) {
    return { tool: "ChatGPT", role: "Config", impact: "ChatGPT's local CLI configuration — models, trusted folders and document-discovery settings.", scope: "User / Project" };
  }

  // --- Cursor ---
  if (name === ".cursorrules") {
    return { tool: "Cursor", role: "Rules (legacy)", impact: "Cursor steering rules for this project (the older single-file form).", scope: "Project" };
  }
  if (p.includes("/.cursor/rules/")) {
    return { tool: "Cursor", role: "Rule", impact: "A Cursor steering rule (.mdc), optionally scoped to matching files.", scope: "Project" };
  }
  if (p.endsWith("/.cursor/mcp.json")) {
    return { tool: "Cursor", role: "MCP servers", impact: "Declares Model Context Protocol servers Cursor loads (project- or user-level), exposing extra tools and data to the agent.", scope: "Project / User" };
  }

  // --- Gemini / Antigravity ---
  if (name === "gemini.md") {
    return { tool: "Gemini / Antigravity", role: "Project memory", impact: "Project instructions for the Gemini CLI and the Antigravity IDE.", scope: "Project" };
  }

  // --- Cross-tool conventions ---
  if (name === ".mcp.json") {
    return { tool: "MCP (shared)", role: "MCP servers", impact: "Declares Model Context Protocol servers that expose extra tools/data to whichever agent loads it.", scope: "Project" };
  }
  if (name === "copilot-instructions.md") {
    return { tool: "GitHub Copilot", role: "Instructions", impact: "Repository custom instructions Copilot prepends to its prompts.", scope: "Project" };
  }
  if (name === ".windsurfrules") {
    return { tool: "Windsurf", role: "Rules", impact: "Windsurf steering rules for this project.", scope: "Project" };
  }
  if (name === ".clinerules") {
    return { tool: "Cline", role: "Rules", impact: "Cline steering rules for this project.", scope: "Project" };
  }
  if (name === ".aider.conf.yml") {
    return { tool: "Aider", role: "Config", impact: "Aider configuration (model, settings) for this project.", scope: "Project" };
  }
  if (name.startsWith(".aider.")) {
    return { tool: "Aider", role: "Working file", impact: "Aider chat/input history or a local cache it writes as you use it — a working artifact, not steering (usually git-ignored).", scope: "Local" };
  }
  if (p.includes("/.ruler/")) {
    return { tool: "Ruler (multi-tool)", role: "Shared source", impact: "A single source of truth Ruler fans out into each tool's own steering files.", scope: "Project" };
  }

  // --- Hermes / openclaw (best-effort: the app already tracks their state) ---
  if (p.includes("/.hermes/")) {
    return { tool: "Hermes", role: "Agent state", impact: "Hermes agent state and chat sessions (runs in WSL).", scope: "User" };
  }
  if (p.includes("/.openclaw/") || p.includes("/openclaw/")) {
    return { tool: "openclaw", role: "Agent state", impact: "openclaw agent state and sessions.", scope: "User" };
  }

  // --- AI assets & session logs (large files a user finds beside the steering files) ---
  if (name.endsWith(".gguf") || name.endsWith(".safetensors") || name.endsWith(".ggml")) {
    return { tool: "Local models", role: "Model weights", impact: "A local model-weights file (often gigabytes) a runtime like Ollama, llama.cpp or LM Studio loads to run inference offline — an asset, not steering.", scope: "Asset" };
  }
  if (name.startsWith("rollout-") && name.endsWith(".jsonl")) {
    return { tool: "ChatGPT", role: "Session transcript", impact: "A recorded ChatGPT CLI conversation (rollout) — the full turn-by-turn log of one session, not a file that steers new work.", scope: "Session log" };
  }

  return null;
}
