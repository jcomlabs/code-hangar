import { memo, type MouseEvent } from "react";

import { highlightSource } from "./syntaxHighlight";

export const SyntaxHighlightedSource = memo(function SyntaxHighlightedSource({
  source,
  path,
  readable = false,
  onContextMenu
}: {
  source: string;
  path: string;
  readable?: boolean;
  onContextMenu?: (event: MouseEvent<HTMLElement>) => void;
}) {
  const highlighted = highlightSource(source, path);
  return (
    <pre
      className={`source-preview syntax-source${readable ? " readable-preview" : ""}`}
      data-language={highlighted.language}
      data-highlight-limited={highlighted.limited ? "true" : undefined}
      onContextMenu={onContextMenu}
    >
      {highlighted.tokens.map((token, index) => (
        token.kind === "plain"
          ? token.text
          : <span className={`syntax-${token.kind}`} key={`${index}-${token.kind}`}>{token.text}</span>
      ))}
    </pre>
  );
});
