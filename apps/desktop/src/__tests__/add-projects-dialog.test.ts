import { describe, expect, it } from "vitest";

import {
  ADD_PROJECTS_DEEP_SCAN_ACTION,
  ADD_PROJECTS_SHOW_PROGRESS_ACTION,
  deepScanSourceLabels,
  deepScanUsesIndeterminateProgress,
  isWslInstalledApp,
  partitionInstalledApps,
  wslAppBadgeLabel,
  type InstalledAppEntry
} from "../addProjectsDialog";
import { DIALOG_FOCUSABLE_SELECTOR, DIALOG_INITIAL_FOCUS_SELECTOR, nextDialogFocusIndex } from "../dialogFocus";

describe("Add Projects dialog", () => {
  it("keeps optional source checkboxes in the keyboard focus loop", () => {
    expect(DIALOG_FOCUSABLE_SELECTOR).toContain("button:not(:disabled)");
    expect(DIALOG_FOCUSABLE_SELECTOR).toContain("input:not(:disabled)");
    expect(DIALOG_FOCUSABLE_SELECTOR.split(", ")).toContain("input:not(:disabled)");
    expect(DIALOG_FOCUSABLE_SELECTOR).not.toBe("button:not(:disabled)");
  });

  it("uses an explicit verb on the primary Deep Scan call to action", () => {
    expect(ADD_PROJECTS_DEEP_SCAN_ACTION).toBe("Start scan");
    expect(ADD_PROJECTS_SHOW_PROGRESS_ACTION).toBe("Show progress");
  });

  it("uses indeterminate progress until the backend exposes a measured inventory percentage", () => {
    expect(deepScanUsesIndeterminateProgress("scanning", undefined)).toBe(true);
    expect(deepScanUsesIndeterminateProgress("registering", 100)).toBe(true);
    expect(deepScanUsesIndeterminateProgress("building", null)).toBe(true);
    expect(deepScanUsesIndeterminateProgress("building", 42)).toBe(false);
    expect(deepScanUsesIndeterminateProgress("done", 100)).toBe(false);
  });

  it("provides a deliberate initial-focus marker and wraps Tab in both directions", () => {
    expect(DIALOG_INITIAL_FOCUS_SELECTOR).toBe("[data-dialog-initial-focus]");
    expect(nextDialogFocusIndex(4, 3, false)).toBe(0);
    expect(nextDialogFocusIndex(4, 0, true)).toBe(3);
    expect(nextDialogFocusIndex(4, -1, false)).toBe(0);
    expect(nextDialogFocusIndex(0, -1, false)).toBe(-1);
  });
});

describe("isWslInstalledApp", () => {
  it("flags the reserved WSL summary id and per-app WSL ids", () => {
    expect(isWslInstalledApp("wsl")).toBe(true);
    expect(isWslInstalledApp("wsl:claude")).toBe(true);
    expect(isWslInstalledApp("wsl:hermes")).toBe(true);
  });

  it("leaves the real host-app ids alone", () => {
    for (const id of ["claude", "codex", "cursor", "antigravity", "gemini", "windsurf", "openclaw", "hermes", "pinokio"]) {
      expect(isWslInstalledApp(id)).toBe(false);
    }
  });
});

describe("wslAppBadgeLabel", () => {
  it("shortens the backend '<app> — in WSL (<distro>)' label to a compact badge", () => {
    expect(wslAppBadgeLabel("Claude Code — in WSL (Ubuntu)")).toBe("Claude Code · WSL");
    expect(wslAppBadgeLabel("Hermes / NemoClaw — in WSL (Ubuntu)")).toBe("Hermes / NemoClaw · WSL");
  });

  it("falls back to the raw label when there is no ' — ' separator", () => {
    expect(wslAppBadgeLabel("ChatGPT")).toBe("ChatGPT · WSL");
  });
});

describe("partitionInstalledApps", () => {
  // Mirrors a real detect_installed_apps result once the WSL gate is on: the host
  // apps, the `wsl` summary offer, and per-app `wsl:<app>` confirmations, plus some
  // absent entries that must be dropped.
  const apps: InstalledAppEntry[] = [
    { id: "claude", label: "Claude Code", present: true },
    { id: "codex", label: "ChatGPT", present: true },
    { id: "antigravity", label: "Antigravity", present: false },
    { id: "wsl", label: "WSL detected: 2 distro(s) (Ubuntu, Debian). Enable WSL scanning to include AI tools installed inside them.", present: true },
    { id: "wsl:claude", label: "Claude Code — in WSL (Ubuntu)", present: true },
    { id: "wsl:hermes", label: "Hermes / NemoClaw — in WSL (Ubuntu)", present: true },
    { id: "wsl:codex", label: "ChatGPT — in WSL (Debian)", present: false }
  ];

  it("excludes every wsl* id from the host-app chips (present hosts only)", () => {
    const { hostApps } = partitionInstalledApps(apps);
    expect(hostApps.map((app) => app.id)).toEqual(["claude", "codex"]);
    expect(hostApps.every((app) => !isWslInstalledApp(app.id))).toBe(true);
    // The absent host app never surfaces as a chip.
    expect(hostApps.some((app) => app.id === "antigravity")).toBe(false);
  });

  it("surfaces the `wsl` summary entry as the WSL offer", () => {
    const { wslOffer } = partitionInstalledApps(apps);
    expect(wslOffer?.id).toBe("wsl");
    expect(wslOffer?.label).toContain("WSL detected: 2 distro(s)");
    expect(wslOffer?.label).toContain("Enable WSL scanning");
  });

  it("maps present `wsl:<app>` entries to per-app WSL confirmations", () => {
    const { wslApps } = partitionInstalledApps(apps);
    expect(wslApps).toEqual([
      { id: "wsl:claude", appId: "claude", label: "Claude Code — in WSL (Ubuntu)", badge: "Claude Code · WSL" },
      { id: "wsl:hermes", appId: "hermes", label: "Hermes / NemoClaw — in WSL (Ubuntu)", badge: "Hermes / NemoClaw · WSL" }
    ]);
    // The absent per-app WSL entry is dropped like any other non-present app.
    expect(wslApps.some((app) => app.id === "wsl:codex")).toBe(false);
  });

  it("returns no offer or confirmations when only host apps are present (gate off)", () => {
    const { hostApps, wslOffer, wslApps } = partitionInstalledApps([
      { id: "claude", label: "Claude Code", present: true },
      { id: "cursor", label: "Cursor", present: true }
    ]);
    expect(hostApps.map((app) => app.id)).toEqual(["claude", "cursor"]);
    expect(wslOffer).toBeNull();
    expect(wslApps).toEqual([]);
  });

  it("surfaces the offer with no per-app confirmations while the gate is off", () => {
    const { wslOffer, wslApps } = partitionInstalledApps([
      { id: "claude", label: "Claude Code", present: true },
      { id: "wsl", label: "WSL detected: 1 distro(s) (Ubuntu). Enable WSL scanning to include AI tools installed inside them.", present: true }
    ]);
    expect(wslOffer?.id).toBe("wsl");
    expect(wslApps).toEqual([]);
  });

  it("builds honest progress sources without inventing a WSL app", () => {
    expect(deepScanSourceLabels(apps, false)).toEqual(["Claude Code", "ChatGPT"]);
    expect(deepScanSourceLabels([
      { id: "claude", label: "Claude Code", present: true },
      { id: "wsl", label: "WSL detected: 1 distro(s) (Ubuntu).", present: true }
    ], true)).toEqual(["Claude Code", "WSL AI tools"]);
  });

  it("uses detected per-app WSL evidence when it exists", () => {
    expect(deepScanSourceLabels(apps, true)).toEqual([
      "Claude Code",
      "ChatGPT",
      "Claude Code · WSL",
      "Hermes / NemoClaw · WSL"
    ]);
    expect(deepScanSourceLabels([], false)).toEqual(["Windows AI tools"]);
  });
});
