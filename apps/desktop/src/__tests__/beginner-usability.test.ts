// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { BEGINNER_HELP } from "../BeginnerHelp";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const contextMenuSource = readFileSync(new URL("../ContextMenu.tsx", import.meta.url), "utf8");
const projectHomeSource = readFileSync(new URL("../views/ProjectHomeViews.tsx", import.meta.url), "utf8");
const recapSource = readFileSync(new URL("../views/RecapView.tsx", import.meta.url), "utf8");
const settingsSource = readFileSync(new URL("../views/SettingsView.tsx", import.meta.url), "utf8");
const connectorSettingsSource = readFileSync(new URL("../views/ConnectorSettingsViews.tsx", import.meta.url), "utf8");
const stylesSource = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

describe("beginner help catalogue", () => {
  it("gives every supported concept a short two-part explanation", () => {
    expect(Object.keys(BEGINNER_HELP).length).toBeGreaterThanOrEqual(20);
    for (const entry of Object.values(BEGINNER_HELP)) {
      expect(entry.title.trim().length).toBeGreaterThan(0);
      expect(entry.paragraphs).toHaveLength(2);
      expect(entry.paragraphs.every((paragraph) => paragraph.trim().length >= 40)).toBe(true);
    }
  });

  it("defines Git and its risky verbs without implying Code Hangar can run them", () => {
    const gitHelp = BEGINNER_HELP.git.paragraphs.join(" ");
    expect(gitHelp).toContain("A commit is a named snapshot");
    expect(gitHelp).toContain("A branch is a separate line of work");
    expect(gitHelp).toContain("does not commit, push or change branches");
  });
});

describe("beginner review workflow", () => {
  it("separates overview, current project files, conversations and older reviews", () => {
    for (const label of ["Overview", "Project files now", "AI conversations", "Older saved reviews"]) {
      expect(recapSource).toContain(label);
    }
    expect(recapSource).toContain("How Code Hangar worked this out");
    expect(recapSource).toContain("Show technical source");
    expect(recapSource).toContain("Older saved evidence was left out.");
    expect(recapSource).toContain("projectReviewLedger(projectId, 100).catch(() => null)");
    expect(stylesSource).toContain("position: fixed");
    expect(stylesSource).toContain(".help-popover-panel");
    expect(stylesSource).toContain(".recap-ledger-warning");
  });

  it("keeps local selected-text copy without provider-backed tools", () => {
    expect(contextMenuSource).toContain("context-menu-title");
    expect(contextMenuSource).toContain("context-menu-section");
    for (const forbidden of [
      "AI tools for selected text",
      "Explain selected text with AI",
      "Check selected text for risks",
      "Suggest a change"
    ]) {
      expect(appSource).not.toContain(forbidden);
    }
    expect(appSource).toContain("Copy selected text");
    expect(appSource).toContain("navigator.clipboard.writeText(selectedText)");
    expect(appSource).toContain("without sending or changing anything");
  });

  it("keeps file discovery and safety tools available from contextual menus", () => {
    expect(appSource).toContain('label: "Show in File Explorer"');
    expect(appSource).toContain('section: "More tools"');
    expect(appSource).toContain('label: "Safe Manage"');
    expect(projectHomeSource).toContain("onContextMenu: FileContextMenuHandler");
    expect(projectHomeSource).toContain("Right-click for File Explorer, Safe Manage and more tools.");
  });

  it("requires an explicit project scope before connecting an AI app", () => {
    expect(connectorSettingsSource).toContain("Projects for the next connection");
    expect(connectorSettingsSource).toContain("projectIds.length === 0");
    expect(connectorSettingsSource).toMatch(/connectedAppRegister\(\s*host\.host,\s*projectIds,/);
    expect(connectorSettingsSource).toContain("item.host === updated.host ? updated : item");
    expect(connectorSettingsSource).not.toContain("connectedAppRegister(host.host, [])");
    expect(settingsSource).not.toContain("connectedAppRegister");
    expect(settingsSource).not.toContain("AI app integration");
    expect(appSource).toContain("projects={projects.filter((project) => !isDemoProject(project))}");
  });
});
