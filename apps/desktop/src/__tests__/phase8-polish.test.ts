// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

const newSurfaceFiles = [
  "../views/RecapView.tsx",
  "../views/project-center/ChangeAccessDialog.tsx",
  "../views/project-center/ChangeReviewDialog.tsx",
  "../views/project-center/CorrectionChecks.tsx",
  "../views/project-center/PreviousVersions.tsx",
  "../views/project-center/SyntaxHighlightedSource.tsx",
  "../views/project-center/ValueEditor.tsx"
] as const;

describe("Phase 8 visual and rendering polish", () => {
  it("memoizes every new retrospective, learning and correction surface", () => {
    for (const path of newSurfaceFiles) {
      const source = readFileSync(new URL(path, import.meta.url), "utf8");
      expect(source, path).toMatch(/\bmemo\(function\s+[A-Z]/);
    }
  });

  it("honours both OS and in-app reduced-motion settings", () => {
    expect(styles).toContain("@media (prefers-reduced-motion: reduce)");
    expect(styles).toMatch(/data-reduce-motion="true"[\s\S]*?\.scan-celebration[\s\S]*?animation: none !important;/);
    expect(styles).toMatch(/data-reduce-motion="true"[\s\S]*?\.deep-scan-bar-fill\.indeterminate[\s\S]*?width: 100% !important;/);
    expect(styles).toMatch(/data-reduce-motion="true"[\s\S]*?\.scan-progress-fill\.indeterminate[\s\S]*?width: 100% !important;/);
    expect(styles).toMatch(/prefers-reduced-motion: reduce[\s\S]*?\.statusbar-scan-track > span\.indeterminate[\s\S]*?width: 100% !important;/);
    expect(styles).toMatch(/prefers-reduced-motion: reduce[\s\S]*?\.scan-progress-fill\.indeterminate[\s\S]*?width: 100% !important;/);
  });

  it("neutralizes protection-card hover motion for both reduced-motion settings", () => {
    const osStart = styles.indexOf("@media (prefers-reduced-motion: reduce)");
    const inAppStart = styles.indexOf(".app-shell[data-reduce-motion=\"true\"] :where(", osStart);
    const osReducedMotion = styles.slice(osStart, inAppStart);
    const inAppReducedMotion = styles.slice(inAppStart, styles.indexOf(".status-banner", inAppStart));

    expect(osStart).toBeGreaterThanOrEqual(0);
    expect(inAppStart).toBeGreaterThan(osStart);
    expect(osReducedMotion).toMatch(/:where\([\s\S]*?\.protection-state-card[\s\S]*?\):hover,[\s\S]*?transform: none !important;/);
    expect(inAppReducedMotion).toMatch(/\.protection-state-card[\s\S]*?\):hover,[\s\S]*?transform: none !important;/);
  });

  it("uses the defined line token for every CSS border", () => {
    expect(styles).not.toContain("var(--border");
  });

  it("keeps scan feedback calm and avoids idle resource polling", () => {
    expect(appSource).toContain("{primaryRunningScan ? <ResourceMeter /> : null}");
    expect(appSource).toContain("appearance.reduceMotion ? 1400 : 2200");
    expect(styles).toMatch(/\.scan-celebration\s*\{[\s\S]*?pointer-events: none;/);
    expect(styles).toContain("scan-celebration-in 220ms");
    expect(styles).toContain("scan-celebration-out 220ms");
    expect(styles).toContain("scan-celebration-spark 240ms");
  });

  it("keeps file-tab actions at least 24 CSS pixels square", () => {
    expect(styles).toMatch(/\.tab\s*\{[\s\S]*?grid-template-columns: minmax\(0, 1fr\) 24px;/);
    expect(styles).toMatch(/\.tab-close\s*\{[\s\S]*?width: 24px;[\s\S]*?height: 24px;/);
    expect(styles).toMatch(/\.tab-close:focus-visible\s*\{[\s\S]*?outline: 2px solid var\(--accent\);/);
  });

  it("keeps sticky tool headers opaque over scrolling content", () => {
    expect(styles).toMatch(/\.tool-workspace-header\s*\{[\s\S]*?background: #fbfcfe;/);
  });

  it("gives the review inbox and recap explicit narrow-pane layouts", () => {
    const narrow = styles.slice(styles.indexOf("@media (max-width: 760px)"));
    expect(narrow).toContain(".overview-recap-heading");
    expect(narrow).toContain(".review-inbox-list");
    expect(narrow).toMatch(/\.welcome-card\s*\{\s*grid-template-columns: minmax\(0, 1fr\);/);
    expect(narrow).toMatch(
      /\.overview-recap-heading \.action-button,\s*\.overview-recap-heading \.secondary-button\s*\{\s*grid-column: 2;/
    );
    expect(narrow).toMatch(/\.recap-layout\s*\{\s*grid-template-columns: minmax\(0, 1fr\);/);
    expect(narrow).toContain(".recap-session-list");
    expect(narrow).toMatch(
      /\.workspace\.workspace-project > \.center-pane\s*\{\s*grid-column: 1;\s*grid-row: 1;/
    );
    expect(styles).toMatch(/\.center-pane\s*\{[\s\S]*?grid-template-columns: minmax\(0, 1fr\);/);
  });

  it("uses vector icons and CSS pixels instead of bitmap UI assets on the new surfaces", () => {
    for (const path of newSurfaceFiles) {
      const source = readFileSync(new URL(path, import.meta.url), "utf8");
      expect(source, path).not.toMatch(/<img\b|\.png["']|\.jpg["']/i);
    }
  });
});
