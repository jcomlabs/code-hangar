import { describe, expect, it } from "vitest";
import { markdownToPlainText, renderMarkdownSafe } from "../markdown";

describe("safe markdown preview", () => {
  it("escapes script tags instead of executing or rendering them", () => {
    const rendered = renderMarkdownSafe("# Hello\n<script>alert('x')</script>");

    expect(rendered.html).not.toContain("<script>");
    expect(rendered.html).toContain("&lt;script&gt;");
  });

  it("blocks remote images", () => {
    const rendered = renderMarkdownSafe("![remote](https://example.invalid/a.png)");

    expect(rendered.html).toContain("Remote image blocked");
    expect(rendered.html).not.toContain("<img");
    expect(rendered.html).not.toContain("https://example.invalid/a.png");
  });

  it("keeps remote links inert", () => {
    const rendered = renderMarkdownSafe("[remote](https://example.invalid)");

    expect(rendered.html).toContain("preview-remote-link");
    expect(rendered.html).not.toContain("href=\"https://example.invalid\"");
  });

  it("can render local links as honest non-interactive transcript text", () => {
    const rendered = renderMarkdownSafe("See [the plan](docs/plan.md).", { localLinks: "inert" });

    expect(rendered.html).toContain('<span class="preview-local-link-inert">the plan</span>');
    expect(rendered.html).not.toContain("<a ");
    expect(rendered.links).toEqual([{ label: "the plan", target: "docs/plan.md", isRemote: false }]);
  });

  it("keeps local links interactive by default for file previews", () => {
    const rendered = renderMarkdownSafe("See [the plan](docs/plan.md).");

    expect(rendered.html).toContain('data-local-path="docs/plan.md"');
  });

  it("renders bold inline text without exposing literal markdown markers", () => {
    const rendered = renderMarkdownSafe("This is **important** and __safe__.");

    expect(rendered.html).toContain("<strong>important</strong>");
    expect(rendered.html).toContain("<strong>safe</strong>");
    expect(rendered.html).not.toContain("**important**");
  });

  it("renders inline code and strikethrough without source markers", () => {
    const rendered = renderMarkdownSafe("Use `npm test`; ~~skip this~~.");

    expect(rendered.html).toContain("<code>npm test</code>");
    expect(rendered.html).toContain("<del>skip this</del>");
    expect(rendered.html).not.toContain("`npm test`");
    expect(rendered.html).not.toContain("~~skip this~~");
  });

  it("groups consecutive list items into semantic unordered and ordered lists", () => {
    const rendered = renderMarkdownSafe("- one\n- **two**\n\n1. first\n2. second");

    expect(rendered.html).toContain("<ul><li>one</li><li><strong>two</strong></li></ul>");
    expect(rendered.html).toContain("<ol><li>first</li><li>second</li></ol>");
    expect(rendered.html.match(/<ul>/g)).toHaveLength(1);
    expect(rendered.html.match(/<ol>/g)).toHaveLength(1);
  });

  it("renders simple blockquotes without the source prefix", () => {
    const rendered = renderMarkdownSafe("> Local only.");

    expect(rendered.html).toBe("<blockquote><p>Local only.</p></blockquote>");
  });

  it("renders simple pipe tables as safe HTML tables", () => {
    const rendered = renderMarkdownSafe("| Name | Status |\n| --- | --- |\n| README | **clean** |");

    expect(rendered.html).toContain("<table>");
    expect(rendered.html).toContain("<th>Name</th>");
    expect(rendered.html).toContain("<td><strong>clean</strong></td>");
    expect(rendered.html).not.toContain("| Name | Status |");
  });

  it("produces clean plain text for compact search snippets", () => {
    expect(markdownToPlainText("# Packaging\n\nShips as **two editions** with `local` files.")).toBe(
      "Packaging Ships as two editions with local files."
    );
    expect(markdownToPlainText("| Name | Status |\n| --- | --- |\n| README | **clean** |")).toBe(
      "Name Status README clean"
    );
  });
});
