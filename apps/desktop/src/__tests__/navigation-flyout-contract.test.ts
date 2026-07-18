// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
const flyoutSource = appSource.slice(
  appSource.indexOf('className="brand-flyout"'),
  appSource.indexOf("<PrimaryNavButtons", appSource.indexOf('className="brand-flyout"'))
);

describe("brand navigation flyout", () => {
  it("releases focus after a destination is chosen so focus-within cannot cover the sidebar", () => {
    expect(flyoutSource).toContain("event.currentTarget.contains(active)");
    expect(flyoutSource).toContain("active.blur()");
  });

  it("keeps section labels visible when the compact topbar hides the wordmark", () => {
    expect(styles).toMatch(/\.brand-flyout button span\s*\{[^}]*display:\s*block;/s);
    expect(styles).toMatch(/@media \(max-width: 1600px\)[\s\S]*?\.brand span\s*\{[^}]*display:\s*none;/);
  });
});
