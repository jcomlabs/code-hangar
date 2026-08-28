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

/** Help text for the self-contained Local comments feature. */
export function commentsPanelHint(): string {
  return "Notes you attach to this item. Stored locally and encrypted.";
}

/** Human-readable byline for a Local comment. */
export function formatCommentMeta(comment: Comment): string {
  const when = formatCommentTimestamp(comment.createdAt);
  const edited = comment.updatedAt !== comment.createdAt ? " · edited" : "";
  return `You · ${when}${edited}`;
}

export function formatCommentTimestamp(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
