// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
const flyoutSource = appSource.slice(
  appSource.indexOf("function NavigationFlyout"),
  appSource.indexOf("type ShellOpenChoice")
);

describe("brand navigation flyout", () => {
  it("is a controlled button disclosure with explicit accessible state", () => {
    expect(flyoutSource).toContain('type="button"');
    expect(flyoutSource).toContain("aria-expanded={open}");
    expect(flyoutSource).toContain('aria-controls="primary-navigation-flyout"');
    expect(flyoutSource).toContain("hidden={!open}");
    expect(flyoutSource).toContain("onClick={() => setOpen((current) => !current)}");
  });

  it("supports keyboard entry, Escape with focus restoration and outside-click dismissal", () => {
    expect(flyoutSource).toContain('event.key === "ArrowDown"');
    expect(flyoutSource).toContain('event.key !== "Escape"');
    expect(flyoutSource).toContain('document.addEventListener("pointerdown", handleOutsidePointer)');
    expect(flyoutSource).toContain('document.addEventListener("keydown", handleEscape)');
    expect(flyoutSource).toContain("close(true)");
    expect(flyoutSource).toContain("triggerRef.current?.focus()");
  });

  it("keeps section labels visible when the compact topbar hides the wordmark", () => {
    expect(styles).toMatch(/\.brand-flyout button span\s*\{[^}]*display:\s*block;/s);
    expect(styles).toMatch(/@media \(max-width: 1600px\)[\s\S]*?\.brand span\s*\{[^}]*display:\s*none;/);
  });

  it("does not regress to hover or focus-within visibility", () => {
    expect(styles).toContain(".brand-flyout[hidden]");
    expect(styles).not.toMatch(/\.brand-mark:(?:hover|focus-within)\s+\.brand-flyout/);
  });
});

describe("removed success pulse", () => {
  const uiSource = readFileSync(new URL("../ui.tsx", import.meta.url), "utf8");

  it("has no unused component, timer constant or orphaned selectors", () => {
    expect(uiSource).not.toContain("SuccessPulse");
    expect(uiSource).not.toContain("SUCCESS_PULSE_MS");
    expect(styles).not.toContain("success-pulse");
  });
});
