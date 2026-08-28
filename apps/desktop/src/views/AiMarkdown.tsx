import { memo, useMemo } from "react";
import { renderMarkdownSafe } from "../markdown";

export const AiMarkdown = memo(function AiMarkdown({
  text,
  className = ""
}: {
  text: string;
  className?: string;
}) {
  const html = useMemo(
    () => renderMarkdownSafe(text, { localLinks: "inert" }).html,
    [text]
  );
  return (
    <div
      className={["markdown-preview", className].filter(Boolean).join(" ")}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
});
