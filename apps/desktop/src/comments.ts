import type { Comment } from "./types";

// Pure, framework-free helpers for the comments feature. Kept out of the React
// component so they can be unit-tested under vitest's `node` environment (the
// project has no jsdom / testing-library set up).

export const MAX_COMMENT_LENGTH = 4000;

export interface CommentValidation {
  ok: boolean;
  trimmed: string;
  error?: string;
}

export function validateCommentBody(body: string): CommentValidation {
  const trimmed = body.trim();
  if (!trimmed) {
    return { ok: false, trimmed, error: "A comment cannot be empty." };
  }
  if (trimmed.length > MAX_COMMENT_LENGTH) {
    return {
      ok: false,
      trimmed,
      error: `Comments are limited to ${MAX_COMMENT_LENGTH} characters.`,
    };
  }
  return { ok: true, trimmed };
}

/** A comment authored by a connected AI app rather than by the local user. */
export function isAgentComment(comment: Comment): boolean {
  return comment.source !== "user";
}

/**
 * Help text shown under the Comments header. It must stay edition-aware: only the AI Connector
 * edition lets connected AI apps write comments, so the base (Local) edition must NOT mention them
 * — the owner's promise is that the base UI is completely clean of the app's own AI capabilities.
 * `connectorBuild` mirrors App's `security.activeFeatures.includes("agent_automation")` gate.
 */
export function commentsPanelHint(connectorBuild: boolean): string {
  const connectorFrontendBuild = import.meta.env.MODE === "test" || import.meta.env.MODE === "connector";
  const subject = connectorFrontendBuild && connectorBuild
    ? "Notes you (or, later, your connected AI apps) attach to this item"
    : "Notes you attach to this item";
  return `${subject}. Stored locally and encrypted; never sent anywhere.`;
}

/** Human-readable byline, e.g. "You · 2026-06-22 14:03" or "claude-code · … · edited". */
export function formatCommentMeta(comment: Comment): string {
  const who = isAgentComment(comment) ? comment.author : "You";
  const when = formatCommentTimestamp(comment.createdAt);
  const edited = comment.updatedAt !== comment.createdAt ? " · edited" : "";
  return `${who} · ${when}${edited}`;
}

function formatCommentTimestamp(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
