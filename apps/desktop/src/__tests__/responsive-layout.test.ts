// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

describe("responsive shell layout", () => {
  it("keeps the optional banner, workspace and status rows aligned at narrow widths", () => {
    expect(styles).toMatch(
      /@media \(max-width: 760px\) \{\s*\.app-shell \{\s*grid-template-rows: auto auto minmax\(0, 1fr\) minmax\(32px, auto\);/
    );
  });

  it("does not float the expanded-sidebar collapse control over navigation rows", () => {
    expect(styles).toMatch(
      /\.left-pane:not\(\.pane-collapsed\) > \.pane-collapse-button\.left \{\s*position: relative;\s*top: auto;/
    );
  });

  it("stacks tool filters from the content pane width before select labels clip", () => {
    expect(styles).toMatch(/\.workspace-tool \.right-pane \{\s*container: toolpane \/ inline-size;/);
    expect(styles).toMatch(
      /@container toolpane \(max-width: 620px\) \{\s*\.workspace-tool \.filter-grid \{\s*grid-template-columns: minmax\(0, 1fr\);/
    );
  });
});
