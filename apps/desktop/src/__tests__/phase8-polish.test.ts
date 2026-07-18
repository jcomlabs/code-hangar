// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

const newSurfaceFiles = [
  "../views/RecapView.tsx",
  "../views/RecapAiLayer.tsx",
  "../views/AiLearningTools.tsx",
  "../views/AiUsageMeter.tsx",
  "../views/RewriteDialog.tsx",
  "../views/ProjectAiSummary.tsx",
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
    expect(styles).toMatch(/data-reduce-motion="true"[\s\S]*?\.ai-spin[\s\S]*?animation: none !important;/);
    expect(styles).toMatch(/data-reduce-motion="true"[\s\S]*?\.scan-celebration[\s\S]*?animation: none !important;/);
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
