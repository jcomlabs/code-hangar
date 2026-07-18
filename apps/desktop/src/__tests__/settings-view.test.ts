import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import {
  SCAN_ROOT_PREVIEW_LIMIT,
  SettingsAppearanceView,
  SettingsDiagnosticsExportCard,
  SettingsProtectionView,
  filterScanRoots,
  previewScanRoots,
  protectionVisibilityFlags,
  protectionVisibilityMode,
  scanRootListSummaryLabel,
  summarizeScanRoots
} from "../views/SettingsView";
import type { ScanRoot } from "../types";

const roots: ScanRoot[] = [
  { id: 1, path: "C:\\AI\\Codex\\CodeHangar", enabled: true },
  { id: 2, path: "C:\\AI\\Archive\\OldProject", enabled: false },
  { id: 3, path: "D:\\Models\\SharedCache", enabled: true }
];

describe("Settings scan-root filtering", () => {
  it("summarizes scan roots for the Settings health strip", () => {
    expect(summarizeScanRoots(roots)).toEqual({
      total: 3,
      enabled: 2,
      disabled: 1
    });
  });

  it("filters registered roots by path text", () => {
    expect(filterScanRoots(roots, "archive", "all").map((root) => root.id)).toEqual([2]);
    expect(filterScanRoots(roots, "AI", "all").map((root) => root.id)).toEqual([1, 2]);
  });

  it("combines path search with enabled-state filters", () => {
    expect(filterScanRoots(roots, "AI", "enabled").map((root) => root.id)).toEqual([1]);
    expect(filterScanRoots(roots, "AI", "disabled").map((root) => root.id)).toEqual([2]);
  });

  it("compacts long root lists until the user expands or searches", () => {
    const longRoots = Array.from({ length: SCAN_ROOT_PREVIEW_LIMIT + 4 }, (_, index) => ({
      id: index + 1,
      path: `C:\\Root\\${index + 1}`,
      enabled: true
    }));

    const compact = previewScanRoots(longRoots);
    const expanded = previewScanRoots(longRoots, { expanded: true });
    const searched = previewScanRoots(longRoots, { searchActive: true });

    expect(compact.compacted).toBe(true);
    expect(compact.roots).toHaveLength(SCAN_ROOT_PREVIEW_LIMIT);
    expect(compact.hiddenCount).toBe(4);
    expect(scanRootListSummaryLabel(compact, longRoots.length)).toBe(`${longRoots.length} match · ${SCAN_ROOT_PREVIEW_LIMIT} shown`);
    expect(expanded.roots).toHaveLength(longRoots.length);
    expect(scanRootListSummaryLabel(expanded, longRoots.length)).toBe(`${longRoots.length} shown`);
    expect(searched.roots).toHaveLength(longRoots.length);
  });
});

describe("Settings appearance choices", () => {
  it("exposes the selected text size and density as pressed controls", () => {
    const html = renderToStaticMarkup(createElement(SettingsAppearanceView, {
      fontSize: "comfortable",
      setFontSize: () => undefined,
      density: "comfortable",
      setDensity: () => undefined,
      contrast: "standard",
      setContrast: () => undefined,
      reduceMotion: false,
      setReduceMotion: () => undefined,
      showTopbarNav: true,
      setShowTopbarNav: () => undefined,
      showAllProjectPaths: true,
      setShowAllProjectPaths: () => undefined,
      demosVisible: false,
      demoVisibilityAutomatic: true,
      setDemosVisible: () => undefined,
      startupPreferences: {
        destination: "overview",
        leftPane: "open",
        rightPane: "remember"
      },
      setStartupPreferences: () => undefined,
      replayTour: () => undefined,
      resetLayout: () => undefined
    }));

    expect(html.match(/aria-pressed="true"/g)).toHaveLength(2);
    expect(html).toContain('role="group" aria-label="Text size"');
    expect(html).toContain('role="group" aria-label="Layout density"');
    expect(html).toContain("Navigation and project list");
    expect(html).toContain("On startup");
    expect(html).toContain('aria-label="Startup workspace"');
    expect(html).toContain('aria-label="Project sidebar on startup"');
    expect(html).toContain('aria-label="Details panel on startup"');
    expect(html).toContain('<option value="overview" selected="">Overview</option>');
    expect(html).toContain('<option value="open" selected="">Open</option>');
    expect(html).toContain('<option value="remember" selected="">Remember last state</option>');
    expect(html).toContain("Replay guided tour");
  });
});

describe("Settings diagnostics export", () => {
  it("states the privacy boundary before creating a support bundle", () => {
    const html = renderToStaticMarkup(createElement(SettingsDiagnosticsExportCard));

    expect(html).toContain("Redacted diagnostics");
    expect(html).toContain("Export diagnostics");
    expect(html).toContain("Project identity and evidence content are omitted by construction.");
    expect(html).toContain("without project names, file names, paths, sessions, prompts, source, diffs, logs, endpoints, credentials or model configuration");
  });
});

describe("Settings protection visibility", () => {
  it("maps the two policy flags to one clear visibility mode", () => {
    expect(protectionVisibilityMode(false, false)).toBe("locked");
    expect(protectionVisibilityMode(true, false)).toBe("reveal");
    expect(protectionVisibilityMode(true, true)).toBe("auto");
    expect(protectionVisibilityMode(false, true)).toBe("locked");

    expect(protectionVisibilityFlags("locked")).toEqual({
      allowSensitiveReveal: false,
      relaxNonStrongPreview: false
    });
    expect(protectionVisibilityFlags("reveal")).toEqual({
      allowSensitiveReveal: true,
      relaxNonStrongPreview: false
    });
    expect(protectionVisibilityFlags("auto")).toEqual({
      allowSensitiveReveal: true,
      relaxNonStrongPreview: true
    });
  });

  it("renders the three session modes as real pressed controls", () => {
    const html = renderToStaticMarkup(createElement(SettingsProtectionView, {
      zones: [],
      zoneAllowSensitiveReveal: true,
      setZoneAllowSensitiveReveal: () => undefined,
      zoneRelaxNonStrongPreview: false,
      setZoneRelaxNonStrongPreview: () => undefined,
      zoneShowProtectedMetadata: false,
      setZoneShowProtectedMetadata: () => undefined
    }));

    expect(html).toContain('role="group" aria-label="Temporary content visibility"');
    expect(html.match(/aria-pressed="true"/g)).toHaveLength(1);
    expect(html.match(/aria-pressed="false"/g)).toHaveLength(2);
    expect(html).toContain("Non-strong text can be revealed one file at a time after confirmation.");
    expect(html).not.toContain("Auto-preview while I browse");
  });
});
