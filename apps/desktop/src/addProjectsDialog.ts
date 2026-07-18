export const ADD_PROJECTS_DEEP_SCAN_ACTION = "Start scan";
export const ADD_PROJECTS_SHOW_PROGRESS_ACTION = "Show progress";

export type DeepScanPhase = "scanning" | "registering" | "building" | "done";

// The reserved id the backend's detect_installed_apps uses for the single
// WSL-presence SUMMARY row ("WSL detected: N distro(s) …"). It is not a host app.
export const WSL_SUMMARY_APP_ID = "wsl";

// Prefix the backend uses for per-app WSL-presence rows (`wsl:claude`,
// `wsl:codex`, `wsl:hermes`, …) — only emitted once the WSL scan gate is on.
export const WSL_APP_ID_PREFIX = "wsl:";

// Minimal shape of a backend InstalledApp entry the dialog needs. Mirrors
// `InstalledApp` in types.ts, kept local so these stay pure, node-testable helpers
// with no React/type-module imports.
export interface InstalledAppEntry {
  id: string;
  label: string;
  present: boolean;
}

// A per-app WSL-presence confirmation derived from a `wsl:<app>` entry.
export interface WslAppPresence {
  id: string; // full reserved id, e.g. "wsl:claude"
  appId: string; // suffix after "wsl:", e.g. "claude"
  label: string; // raw backend label, e.g. "Claude Code — in WSL (Ubuntu)"
  badge: string; // compact chip label, e.g. "Claude Code · WSL"
}

// Host apps (chips) split from the WSL offer / per-app confirmations, out of a raw
// detect_installed_apps result. Only PRESENT entries are surfaced.
export interface InstalledAppsPartition {
  hostApps: InstalledAppEntry[];
  wslOffer: InstalledAppEntry | null;
  wslApps: WslAppPresence[];
}

// True for the backend's reserved WSL ids (`wsl` summary + `wsl:<app>` rows).
// These are NOT host apps — they drive the WSL offer / confirmations instead, so
// callers exclude them from the detected-host-app list everywhere it renders.
export function isWslInstalledApp(id: string): boolean {
  return id === WSL_SUMMARY_APP_ID || id.startsWith(WSL_APP_ID_PREFIX);
}

// Compact badge label for a per-app WSL confirmation: take the app-name prefix of
// the backend label (before its " — in WSL (…)" tail) and mark it as WSL, e.g.
// "Claude Code — in WSL (Ubuntu)" -> "Claude Code · WSL". Faithful to backend copy
// (no hard-coded app names) while staying short enough for a badge.
export function wslAppBadgeLabel(label: string): string {
  const separatorIndex = label.indexOf(" — ");
  const name = (separatorIndex >= 0 ? label.slice(0, separatorIndex) : label).trim();
  return `${name} · WSL`;
}

// Partition a raw detect_installed_apps result into the host-app chips, the single
// WSL offer row, and the per-app WSL confirmations. Only present entries are kept,
// so unwiring is impossible: reserved `wsl*` ids can never leak into `hostApps`.
export function partitionInstalledApps(apps: InstalledAppEntry[]): InstalledAppsPartition {
  const hostApps: InstalledAppEntry[] = [];
  let wslOffer: InstalledAppEntry | null = null;
  const wslApps: WslAppPresence[] = [];
  for (const app of apps) {
    if (!app.present) continue;
    if (app.id === WSL_SUMMARY_APP_ID) {
      wslOffer = app;
    } else if (app.id.startsWith(WSL_APP_ID_PREFIX)) {
      wslApps.push({
        id: app.id,
        appId: app.id.slice(WSL_APP_ID_PREFIX.length),
        label: app.label,
        badge: wslAppBadgeLabel(app.label)
      });
    } else {
      hostApps.push(app);
    }
  }
  return { hostApps, wslOffer, wslApps };
}

// Progress sources must describe what discovery will really inspect. In
// particular, never invent a specific WSL tool before the backend has detected
// it; use a generic WSL source until per-app evidence exists.
export function deepScanSourceLabels(apps: InstalledAppEntry[], includeWsl: boolean): string[] {
  const { hostApps, wslApps } = partitionInstalledApps(apps);
  const labels = hostApps.map((app) => app.label);
  if (labels.length === 0) labels.push("Windows AI tools");
  if (includeWsl) {
    labels.push(...(wslApps.length > 0 ? wslApps.map((app) => app.badge) : ["WSL AI tools"]));
  }
  return Array.from(new Set(labels));
}

// Registry discovery and automatic registration expose no measured percentage,
// so their bar must stay indeterminate. Inventory building may use a real backend
// percentage once one is available.
export function deepScanUsesIndeterminateProgress(
  phase: DeepScanPhase,
  measuredPercent: number | null | undefined
): boolean {
  if (phase === "scanning" || phase === "registering") return true;
  return phase === "building" && measuredPercent == null;
}
