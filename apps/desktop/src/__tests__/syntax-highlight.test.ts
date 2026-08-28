import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { SyntaxHighlightedSource } from "../views/project-center/SyntaxHighlightedSource";
import { highlightSource, sourceLanguage } from "../views/project-center/syntaxHighlight";

describe("offline syntax highlighting", () => {
  it("detects common local project languages from file names", () => {
    expect(sourceLanguage("src/App.tsx")).toBe("typescript");
    expect(sourceLanguage("crates/api/src/lib.rs")).toBe("rust");
    expect(sourceLanguage("config/settings.toml")).toBe("toml");
    expect(sourceLanguage("scripts/check.ps1")).toBe("powershell");
  });

  it("preserves the source byte-for-byte while classifying useful tokens", () => {
    const source = "const enabled = true; // local only\nreturn enabled;";
    const result = highlightSource(source, "src/settings.ts");

    expect(result.tokens.map((token) => token.text).join("")).toBe(source);
    expect(result.tokens.some((token) => token.kind === "keyword" && token.text.includes("const"))).toBe(true);
    expect(result.tokens.some((token) => token.kind === "literal" && token.text.includes("true"))).toBe(true);
    expect(result.tokens.some((token) => token.kind === "comment")).toBe(true);
  });

  it("does not interpret markup or embed rendered HTML", () => {
    const source = "const tag = '<img src=x onerror=alert(1)>';";
    const result = highlightSource(source, "src/app.ts");

    expect(result.tokens.map((token) => token.text).join("")).toBe(source);
    expect(result.tokens.find((token) => token.kind === "string")?.text).toContain("<img");
  });

  it("renders classified spans while React escapes source markup", () => {
    const source = "const tag = '<img src=x onerror=alert(1)>'; // local";
    const html = renderToStaticMarkup(createElement(SyntaxHighlightedSource, {
      source,
      path: "src/app.ts"
    }));

    expect(html).toContain('data-language="typescript"');
    expect(html).toContain('class="syntax-keyword"');
    expect(html).toContain('class="syntax-string"');
    expect(html).toContain('class="syntax-comment"');
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(html).not.toContain("<img src=x");
  });

  it("bounds work for very large files and leaves the text intact", () => {
    const source = `const value = 1;\n${"x".repeat(250_000)}`;
    const result = highlightSource(source, "src/large.ts");

    expect(result.limited).toBe(true);
    expect(result.tokens).toEqual([{ kind: "plain", text: source }]);
  });
});
