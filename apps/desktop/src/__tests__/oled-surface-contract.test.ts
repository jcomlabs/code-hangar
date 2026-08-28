// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

describe("OLED surface parity", () => {
  it("keeps the expanded Lost Projects preset action dark", () => {
    expect(styles).toMatch(
      /\.app-shell\[data-theme="oled"\] \.preset-save-row button \{[\s\S]*?background: #0b0b0b;[\s\S]*?color: var\(--text\);/
    );
  });
});
