import type { Comment } from "../types";
import { formatCommentTimestamp } from "../comments";

export default function ConnectorCommentPresentation({
  mode,
  comment
}: {
  mode: "hint" | "meta";
  comment?: Comment;
}) {
  if (mode === "hint") {
    return (
      <p className="connector-comments-hint">
        Connected AI apps can add notes only when their permission and the global write switch are both enabled.
      </p>
    );
  }
  if (!comment) return null;
  const edited = comment.updatedAt !== comment.createdAt ? " · edited" : "";
  return (
    <>
      <span>{comment.author} · {formatCommentTimestamp(comment.createdAt)}{edited}</span>
      <span className="connector-comment-badge">AI</span>
    </>
  );
}
