import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AiMarkdown } from "../views/AiMarkdown";

describe("AI response Markdown", () => {
  it("renders common model formatting through the safe local renderer", () => {
    const html = renderToStaticMarkup(createElement(AiMarkdown, {
      text: "## Result\n\nThis is **clear**.\n\n- one\n- two\n\n<script>alert('no')</script>"
    }));

    expect(html).toContain("<h2>Result</h2>");
    expect(html).toContain("<strong>clear</strong>");
    expect(html).toContain("<ul>");
    expect(html).not.toContain("**clear**");
    expect(html).not.toContain("<script>");
  });
});
