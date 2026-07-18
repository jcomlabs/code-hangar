// Best-effort parsing of a local AI session transcript into readable turns, used
// by the session "Rendered" view. It never executes or fetches anything — it only
// reshapes already-loaded text. JSONL conversations (Codex/Claude — one JSON
// object per line) become turn-by-turn chat with role labels; anything else
// (e.g. Antigravity's protobuf-recovered text) falls back to blank-line blocks.

import type { SessionDiscoveryCandidate, SessionPreview } from "./types";

export type SessionTurn = { role?: string; label?: string; content: string };

const MIN_PROGRESSIVE_SESSION_BYTES = 256 * 1024;
export const SESSION_TRANSCRIPT_PAGE_SIZE = 200;

export function nextSessionPreviewLimit(currentLimitBytes: number, sizeBytes: number): number {
  const current = Math.max(MIN_PROGRESSIVE_SESSION_BYTES, Math.floor(currentLimitBytes));
  const doubled = Math.min(Number.MAX_SAFE_INTEGER, current * 2);
  return Math.min(Math.max(0, Math.floor(sizeBytes)), doubled);
}

export function sessionSupportsProgressiveLoading(
  association: SessionDiscoveryCandidate["association"],
  preview: Pick<SessionPreview, "truncated"> | null
): boolean {
  // Association is intentionally not a gate: project-linked and loose sessions
  // share the same reader and the same progressive-loading contract.
  void association;
  return Boolean(preview?.truncated);
}

export function sessionTranscriptPageCount(
  turnCount: number,
  pageSize = SESSION_TRANSCRIPT_PAGE_SIZE
): number {
  const safePageSize = Math.max(1, Math.floor(pageSize));
  return Math.max(1, Math.ceil(Math.max(0, Math.floor(turnCount)) / safePageSize));
}

export function clampSessionTranscriptPage(
  page: number,
  turnCount: number,
  pageSize = SESSION_TRANSCRIPT_PAGE_SIZE
): number {
  const lastPage = sessionTranscriptPageCount(turnCount, pageSize) - 1;
  return Math.min(lastPage, Math.max(0, Math.floor(page)));
}

export function initialSessionTranscriptPage(
  turnCount: number,
  truncated: boolean,
  pageSize = SESSION_TRANSCRIPT_PAGE_SIZE
): number {
  // Tail previews should still open on the newest turns. Once the user loads the
  // complete conversation, start at its beginning and let page navigation keep
  // the DOM bounded even for multi-hundred-megabyte source files.
  return truncated ? sessionTranscriptPageCount(turnCount, pageSize) - 1 : 0;
}

export function sessionTranscriptPageSlice<T>(
  items: T[],
  page: number,
  pageSize = SESSION_TRANSCRIPT_PAGE_SIZE
): T[] {
  const safePageSize = Math.max(1, Math.floor(pageSize));
  const safePage = clampSessionTranscriptPage(page, items.length, safePageSize);
  const start = safePage * safePageSize;
  return items.slice(start, start + safePageSize);
}

function toolActivityNames(turn: SessionTurn): string[] | null {
  if (turn.role !== "assistant") return null;
  const lines = turn.content.split(/\r?\n+/).map((line) => line.trim()).filter(Boolean);
  if (lines.length === 0) return null;
  const names: string[] = [];
  for (const line of lines) {
    const match = /^↳ used (.+)$/.exec(line);
    if (!match) return null;
    names.push(match[1]);
  }
  return names;
}

export function compactSessionToolActivity(turns: SessionTurn[]): SessionTurn[] {
  const compacted: SessionTurn[] = [];
  for (let index = 0; index < turns.length;) {
    const firstNames = toolActivityNames(turns[index]);
    if (!firstNames) {
      compacted.push(turns[index]);
      index += 1;
      continue;
    }

    const names = [...firstNames];
    let nextIndex = index + 1;
    while (nextIndex < turns.length) {
      const nextNames = toolActivityNames(turns[nextIndex]);
      if (!nextNames) break;
      names.push(...nextNames);
      nextIndex += 1;
    }
    compacted.push({
      role: "tool",
      label: "Tool activity",
      content: names.length === 1 ? `Tool used: ${names[0]}` : `${names.length} tool calls: ${names.join(", ")}`
    });
    index = nextIndex;
  }
  return compacted;
}

export interface SessionMetadataSummary {
  title?: string;
  initialMessage?: string;
  projectPath?: string;
  model?: string;
  createdMs?: number;
  lastActivityMs?: number;
  permissionMode?: string;
  archived?: boolean;
  enabledToolCount?: number;
  mcpServerCount?: number;
}

export function firstSessionString(...values: unknown[]): string | undefined {
  for (const value of values) {
    if (typeof value === "string" && value.trim().length > 0) return value;
  }
  return undefined;
}

export function sessionRoleKey(role: string): string {
  const r = role.toLowerCase();
  if (r.includes("user") || r.includes("human")) return "user";
  if (r.includes("assistant") || r.includes("model") || r.includes("gemini") || r.includes("claude") || r.includes("gpt") || r === "ai") return "assistant";
  if (r.includes("system")) return "system";
  if (r.includes("tool") || r.includes("function")) return "tool";
  return "other";
}

export function sessionRoleLabel(role: string): string {
  const key = sessionRoleKey(role);
  if (key === "user") return "You";
  if (key === "assistant") return "Assistant";
  if (key === "system") return "System";
  if (key === "tool") return "Tool";
  return role;
}

// Flatten a `content` value into readable text. It is either a plain string or an
// array of typed parts (the Anthropic/Claude shape: {type:"text",text} plus
// tool_use / tool_result / thinking parts). Human text is kept; a tool call is
// annotated in one line so the flow stays legible; tool results and internal
// reasoning are dropped so the transcript reads as a conversation, not a log.
function contentToText(content: unknown): string {
  if (typeof content === "string") return content.trim();
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const part of content) {
    if (typeof part === "string") {
      if (part.trim()) parts.push(part.trim());
      continue;
    }
    if (!isPlainObject(part)) continue;
    const type = typeof part.type === "string" ? part.type : "";
    if (type === "tool_use" || type === "server_tool_use") {
      const name = firstSessionString(part.name);
      parts.push(name ? `↳ used ${name}` : "↳ used a tool");
    } else if (type === "tool_result" || type === "thinking" || type === "reasoning" || type === "redacted_thinking") {
      // internal or high-volume — omit from the readable view
    } else {
      const text = firstSessionString(part.text, part.content);
      if (text && text.trim()) parts.push(text.trim());
    }
  }
  return parts.join("\n\n").trim();
}

export function sessionTurnContent(record: Record<string, unknown>): string {
  // Claude Code (and similar) nest the real content under a `message` object
  // ({ role, content }); content there is a string or an array of typed parts.
  if (isPlainObject(record.message)) {
    const nested = contentToText((record.message as Record<string, unknown>).content);
    if (nested) return nested;
  }
  const direct = firstSessionString(record.content, record.text, record.body, record.summary, record.value);
  if (direct) return direct;
  const fromArray = contentToText(record.content);
  if (fromArray) return fromArray;
  if (typeof record.message === "string" && record.message.trim()) return record.message.trim();
  // No recognizable text — drop the line rather than dumping raw JSON at the user.
  return "";
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value);
}

function sessionTimestamp(value: unknown): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return undefined;
  return value < 10_000_000_000 ? value * 1000 : value;
}

function jsonFieldValueStart(text: string, key: string): number | undefined {
  const needle = JSON.stringify(key);
  let searchFrom = 0;
  while (searchFrom < text.length) {
    const keyStart = text.indexOf(needle, searchFrom);
    if (keyStart < 0) return undefined;
    let cursor = keyStart + needle.length;
    while (/\s/.test(text[cursor] ?? "")) cursor += 1;
    if (text[cursor] !== ":") {
      searchFrom = cursor;
      continue;
    }
    cursor += 1;
    while (/\s/.test(text[cursor] ?? "")) cursor += 1;
    return cursor;
  }
  return undefined;
}

function jsonStringAt(text: string, start: number | undefined): string | undefined {
  if (start == null || text[start] !== '"') return undefined;
  let escaped = false;
  for (let cursor = start + 1; cursor < text.length; cursor += 1) {
    const character = text[cursor];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (character === "\\") {
      escaped = true;
      continue;
    }
    if (character !== '"') continue;
    try {
      const parsed = JSON.parse(text.slice(start, cursor + 1)) as unknown;
      return typeof parsed === "string" ? parsed : undefined;
    } catch {
      return undefined;
    }
  }
  return undefined;
}

function partialJsonString(text: string, key: string): string | undefined {
  return jsonStringAt(text, jsonFieldValueStart(text, key));
}

function partialJsonNumber(text: string, key: string): number | undefined {
  const start = jsonFieldValueStart(text, key);
  if (start == null) return undefined;
  const match = text.slice(start).match(/^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/);
  if (!match) return undefined;
  const value = Number(match[0]);
  return Number.isFinite(value) ? value : undefined;
}

function partialJsonBoolean(text: string, key: string): boolean | undefined {
  const start = jsonFieldValueStart(text, key);
  if (start == null) return undefined;
  if (text.startsWith("true", start)) return true;
  if (text.startsWith("false", start)) return false;
  return undefined;
}

function partialJsonFirstArrayString(text: string, key: string): string | undefined {
  const start = jsonFieldValueStart(text, key);
  if (start == null || text[start] !== "[") return undefined;
  let cursor = start + 1;
  while (/\s/.test(text[cursor] ?? "")) cursor += 1;
  return jsonStringAt(text, cursor);
}

function boundedMetadataText(value: string | undefined, maxLength: number): string | undefined {
  if (!value) return undefined;
  const compact = value.replace(/\s+/g, " ").trim();
  if (!compact) return undefined;
  return compact.length <= maxLength ? compact : `${compact.slice(0, maxLength - 1).trimEnd()}…`;
}

function hasMultipleJsonObjectLines(text: string): boolean {
  let parsedObjects = 0;
  let inspectedLines = 0;
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    inspectedLines += 1;
    if (trimmed.startsWith("{")) {
      try {
        const parsed = JSON.parse(trimmed) as unknown;
        if (isPlainObject(parsed)) parsedObjects += 1;
      } catch {
        // A pretty-printed or truncated single metadata object is handled below.
      }
    }
    if (parsedObjects >= 2) return true;
    if (inspectedLines >= 24) break;
  }
  return false;
}

export function parseSessionMetadata(text: string): SessionMetadataSummary | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{")) return null;
  let record: Record<string, unknown> | null = null;
  try {
    const parsed = JSON.parse(trimmed) as unknown;
    if (isPlainObject(parsed)) record = parsed;
  } catch {
    // Large local-agent records are intentionally previewed from a bounded head.
    // Read their early top-level scalar fields below without requiring a closing
    // brace, while the Source view continues to preserve the exact preview text.
  }

  // Claude Code transcripts are JSONL and often repeat sessionId on every turn.
  // Treating any occurrence as top-level metadata hides the entire conversation.
  if (!record && hasMultipleJsonObjectLines(trimmed)) return null;

  const looksLikeSessionMetadata = [
    "sessionId",
    "cliSessionId",
    "sessionSettings",
    "enabledMcpTools"
  ].some((key) => (record ? key in record : jsonFieldValueStart(trimmed, key) != null));
  if (!looksLikeSessionMetadata) return null;

  const settings = record && isPlainObject(record.sessionSettings) ? record.sessionSettings : null;
  const enabledTools = record && isPlainObject(record.enabledMcpTools) ? record.enabledMcpTools : null;
  const mcpServers = record && isPlainObject(record.mcpServers) ? record.mcpServers : null;
  const remoteMcpServers = record && Array.isArray(record.remoteMcpServersConfig) ? record.remoteMcpServersConfig : null;
  const selectedFolders = record && Array.isArray(record.userSelectedFolders)
    ? record.userSelectedFolders.filter((value): value is string => typeof value === "string" && value.trim().length > 0)
    : [];
  const rawTitle = firstSessionString(record?.title, partialJsonString(trimmed, "title"));
  const rawInitialMessage = firstSessionString(record?.initialMessage, partialJsonString(trimmed, "initialMessage"));
  return {
    title: boundedMetadataText(rawTitle, 160),
    initialMessage: boundedMetadataText(rawInitialMessage, 500),
    projectPath: firstSessionString(
      selectedFolders[0],
      partialJsonFirstArrayString(trimmed, "userSelectedFolders"),
      record?.originCwd,
      partialJsonString(trimmed, "originCwd"),
      record?.cwd,
      partialJsonString(trimmed, "cwd")
    ),
    model: firstSessionString(record?.model, partialJsonString(trimmed, "model")),
    createdMs: sessionTimestamp(record?.createdAt ?? partialJsonNumber(trimmed, "createdAt")),
    lastActivityMs: sessionTimestamp(record?.lastActivityAt ?? partialJsonNumber(trimmed, "lastActivityAt")),
    permissionMode: firstSessionString(record?.permissionMode, settings?.permissionMode, partialJsonString(trimmed, "permissionMode")),
    archived: typeof record?.isArchived === "boolean" ? record.isArchived : partialJsonBoolean(trimmed, "isArchived"),
    enabledToolCount: enabledTools
      ? Object.values(enabledTools).filter((enabled) => enabled === true).length
      : undefined,
    mcpServerCount: mcpServers ? Object.keys(mcpServers).length : remoteMcpServers?.length
  };
}

const UUID_SESSION_NAME = /^(?:local[_-])?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function sessionDisplayNameNeedsEnrichment(displayName: string): boolean {
  const withoutExtension = displayName.trim().replace(/\.(?:jsonl?|txt)$/i, "");
  return UUID_SESSION_NAME.test(withoutExtension);
}

export function enrichedSessionDisplayName(displayName: string, previewText: string): string {
  if (!sessionDisplayNameNeedsEnrichment(displayName)) return displayName;
  const metadataTitle = parseSessionMetadata(previewText)?.title;
  if (metadataTitle) return metadataTitle;
  const firstHumanTurn = parseSessionTranscript(previewText).find((turn) => turn.role === "user")?.content;
  const compact = boundedMetadataText(firstHumanTurn?.replace(/<[^>]+>/g, " "), 120);
  return compact ?? displayName;
}

// Codex rollout lines that carry no readable conversation (reasoning is an
// encrypted blob; the rest are tool/bookkeeping events). Skipping them is what
// stops the "encrypted content" dump.
const CODEX_SKIP_PAYLOAD_TYPES = new Set([
  "reasoning",
  "token_count",
  "function_call",
  "function_call_output",
  "custom_tool_call_output",
  "session_meta",
  "turn_context",
  "turn_diff"
]);

// Codex (OpenAI CLI) rollouts wrap everything under `payload` and duplicate the
// conversation across an `event_msg` stream (user_message/agent_message, clean
// strings) and a `response_item` stream (message with content[].text). Render the
// event_msg stream when present, else the response_item one; skip reasoning/tool
// lines entirely so encrypted blobs never surface.
function codexTurns(records: Record<string, unknown>[]): SessionTurn[] {
  const eventTurns: SessionTurn[] = [];
  const itemTurns: SessionTurn[] = [];
  for (const record of records) {
    const payload = isPlainObject(record.payload) ? record.payload : record;
    const type = typeof payload.type === "string" ? payload.type : undefined;
    if (type && CODEX_SKIP_PAYLOAD_TYPES.has(type)) continue;
    if (type === "user_message" || type === "agent_message") {
      const content = firstSessionString(payload.message, payload.text);
      if (content && content.trim()) {
        eventTurns.push(
          type === "user_message"
            ? { role: "user", label: "You", content }
            : { role: "assistant", label: "Assistant", content }
        );
      }
    } else if (type === "session_gap") {
      const content = firstSessionString(payload.message, payload.text);
      if (content && content.trim()) {
        eventTurns.push({ role: "system", label: "Timeline", content });
      }
    } else if (type === "message") {
      const roleRaw = firstSessionString(payload.role);
      const content = sessionTurnContent(payload);
      if (content.trim() && content !== JSON.stringify(payload)) {
        itemTurns.push({
          role: roleRaw ? sessionRoleKey(roleRaw) : undefined,
          label: roleRaw ? sessionRoleLabel(roleRaw) : undefined,
          content
        });
      }
    }
  }
  return eventTurns.length > 0 ? eventTurns : itemTurns;
}

// Non-conversation bookkeeping line types (seen in real Claude Code transcripts) —
// queued input echoes, auto-titles, attachments, mode/snapshot markers — that aren't
// turns and would just be noise in the readable view.
const GENERIC_SKIP_TYPES = new Set([
  "queue-operation",
  "summary",
  "file-history-snapshot",
  "snapshot",
  "attachment",
  "last-prompt",
  "ai-title",
  "mode"
]);

// Generic JSONL (Claude and similar). Role lives at the top level (`role`/`type`) or
// nested in a `message` object; content is resolved by `sessionTurnContent`.
function genericTurns(records: Record<string, unknown>[]): SessionTurn[] {
  const turns: SessionTurn[] = [];
  for (const record of records) {
    const typeRaw = typeof record.type === "string" ? record.type : "";
    if (typeRaw && GENERIC_SKIP_TYPES.has(typeRaw.toLowerCase())) continue;
    const message = isPlainObject(record.message) ? record.message : undefined;
    const roleRaw = firstSessionString(record.role, message?.role, record.type, record.sender, record.author, record.from);
    const content = sessionTurnContent(record);
    if (!content.trim()) continue;
    turns.push({
      role: roleRaw ? sessionRoleKey(roleRaw) : undefined,
      label: roleRaw ? sessionRoleLabel(roleRaw) : undefined,
      content
    });
  }
  return turns;
}

export function parseSessionTranscript(text: string): SessionTurn[] {
  const trimmed = text.trim();
  if (!trimmed) return [];
  if (parseSessionMetadata(trimmed)) return [];
  const lines = trimmed.split(/\r?\n/);
  let considered = 0;
  const records: Record<string, unknown>[] = [];
  for (const line of lines) {
    const t = line.trim();
    if (!t) continue;
    considered++;
    if (!t.startsWith("{") && !t.startsWith("[")) continue;
    try {
      const parsed = JSON.parse(t) as unknown;
      if (isPlainObject(parsed)) records.push(parsed);
    } catch {
      // Not a JSON line — ignored; if too few lines parse we fall back below.
    }
  }
  if (considered > 0 && records.length / considered >= 0.6 && records.length > 0) {
    // Codex rollouts nest content under `payload`; everything else reads top-level.
    const codexLike = records.filter((record) => isPlainObject(record.payload)).length >= records.length * 0.5;
    return codexLike ? codexTurns(records) : genericTurns(records);
  }
  return trimmed
    .split(/\n\s*\n+/)
    .map((block) => ({ content: block.trim() }))
    .filter((block) => block.content.length > 0);
}
