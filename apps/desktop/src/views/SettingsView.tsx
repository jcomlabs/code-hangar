import { AlertTriangle, Download, PlayCircle, RotateCcw, Shield, ShieldCheck } from "lucide-react";
import { useMemo, useState } from "react";
import { api } from "../api";
import { scanProgressParts } from "../scanProgress";
import type {
  ProtectedZone,
  ScanRoot,
  ScanStatus
} from "../types";
import type { StartupPreferences } from "../workspaceRoute";
import { SectionTitle, protectedZoneHelp } from "../ui";

export type ScanRootFilter = "all" | "enabled" | "disabled";
export const SCAN_ROOT_PREVIEW_LIMIT = 5;

export interface ScanRootPreview {
  roots: ScanRoot[];
  hiddenCount: number;
  compacted: boolean;
}

export function summarizeScanRoots(roots: ScanRoot[]) {
  const enabled = roots.filter((root) => root.enabled).length;
  return {
    total: roots.length,
    enabled,
    disabled: roots.length - enabled
  };
}

export function filterScanRoots(roots: ScanRoot[], query: string, filter: ScanRootFilter) {
  const needle = query.trim().toLocaleLowerCase();
  return roots.filter((root) => {
    if (filter === "enabled" && !root.enabled) return false;
    if (filter === "disabled" && root.enabled) return false;
    if (!needle) return true;
    return root.path.toLocaleLowerCase().includes(needle);
  });
}

export function previewScanRoots(
  roots: ScanRoot[],
  options: { expanded?: boolean; searchActive?: boolean; limit?: number } = {}
): ScanRootPreview {
  const limit = Math.max(0, Math.floor(options.limit ?? SCAN_ROOT_PREVIEW_LIMIT));
  if (options.expanded || options.searchActive || roots.length <= limit) {
    return { roots, hiddenCount: 0, compacted: false };
  }
  return {
    roots: roots.slice(0, limit),
    hiddenCount: roots.length - limit,
    compacted: true
  };
}

export function scanRootListSummaryLabel(preview: ScanRootPreview, matchedCount: number): string {
  const shown = preview.roots.length;
  return preview.compacted ? `${matchedCount} match · ${shown} shown` : `${shown} shown`;
}

export type ProtectionVisibilityMode = "locked" | "reveal" | "auto";

export function protectionVisibilityMode(
  allowSensitiveReveal: boolean,
  relaxNonStrongPreview: boolean
): ProtectionVisibilityMode {
  if (!allowSensitiveReveal) return "locked";
  return relaxNonStrongPreview ? "auto" : "reveal";
}

export function protectionVisibilityFlags(mode: ProtectionVisibilityMode) {
  return {
    allowSensitiveReveal: mode !== "locked",
    relaxNonStrongPreview: mode === "auto"
  };
}

export function SettingsAppearanceView({
  fontSize,
  setFontSize,
  density,
  setDensity,
  contrast,
  setContrast,
  reduceMotion,
  setReduceMotion,
  showTopbarNav,
  setShowTopbarNav,
  showAllProjectPaths,
  setShowAllProjectPaths,
  demosVisible,
  demoVisibilityAutomatic,
  setDemosVisible,
  startupPreferences,
  setStartupPreferences,
  replayTour,
  resetLayout
}: {
  fontSize: "compact" | "comfortable" | "large" | "xlarge";
  setFontSize: (value: "compact" | "comfortable" | "large" | "xlarge") => void;
  density: "compact" | "comfortable" | "spacious";
  setDensity: (value: "compact" | "comfortable" | "spacious") => void;
  contrast: "standard" | "high";
  setContrast: (value: "standard" | "high") => void;
  reduceMotion: boolean;
  setReduceMotion: (value: boolean) => void;
  showTopbarNav: boolean;
  setShowTopbarNav: (value: boolean) => void;
  showAllProjectPaths: boolean;
  setShowAllProjectPaths: (value: boolean) => void;
  demosVisible: boolean;
  demoVisibilityAutomatic: boolean;
  setDemosVisible: (value: boolean) => void;
  startupPreferences: StartupPreferences;
  setStartupPreferences: (value: StartupPreferences) => void;
  replayTour: () => void;
  resetLayout: () => void;
}) {
  return (
    <section className="pane-section compact">
      <div className="dashboard-card" data-help="Text size controls the UI scale for lists, buttons, inspector rows and Markdown previews.">
        <h3>Text size</h3>
        <div className="settings-choice-grid" role="group" aria-label="Text size">
          {(["compact", "comfortable", "large", "xlarge"] as const).map((value) => (
            <button key={value} type="button" className={fontSize === value ? "active" : ""} aria-pressed={fontSize === value} data-help={fontSizeHelp(value)} onClick={() => setFontSize(value)}>
              {fontSizeLabel(value)}
            </button>
          ))}
        </div>
      </div>
      <div className="dashboard-card" data-help="Density changes spacing. Compact shows more rows; spacious improves scanning and clicking.">
        <h3>Layout density</h3>
        <div className="settings-choice-grid" role="group" aria-label="Layout density">
          {(["compact", "comfortable", "spacious"] as const).map((value) => (
            <button key={value} type="button" className={density === value ? "active" : ""} aria-pressed={density === value} data-help={densityHelp(value)} onClick={() => setDensity(value)}>
              {densityLabel(value)}
            </button>
          ))}
        </div>
      </div>
      <div className="dashboard-card" data-help="Contrast adjusts borders and muted text. High contrast is useful in OLED dark mode or high DPI displays.">
        <h3>Contrast and motion</h3>
        <label className="toggle-row" data-help="Use stronger muted text, borders and card contrast. This is visual only.">
          <input type="checkbox" checked={contrast === "high"} onChange={(event) => setContrast(event.target.checked ? "high" : "standard")} />
          <span><strong>High contrast UI</strong><small>Make secondary labels and panel borders easier to read.</small></span>
        </label>
        <label className="toggle-row" data-help="Reduce animated transitions in the UI. This does not affect background jobs.">
          <input type="checkbox" checked={reduceMotion} onChange={(event) => setReduceMotion(event.target.checked)} />
          <span><strong>Reduce motion</strong><small>Minimize animated width/progress transitions.</small></span>
        </label>
      </div>
      <div className="dashboard-card" data-help="Choose the workspace and pane state used the next time Code Hangar starts.">
        <h3>On startup</h3>
        <div className="startup-preferences-grid">
          <label className="startup-preference-row">
            <span><strong>Open to</strong><small>Start predictably or continue from the previous workspace.</small></span>
            <select
              aria-label="Startup workspace"
              value={startupPreferences.destination}
              onChange={(event) => setStartupPreferences({ ...startupPreferences, destination: event.target.value as StartupPreferences["destination"] })}
            >
              <option value="overview">Overview</option>
              <option value="last-workspace">Last workspace</option>
            </select>
          </label>
          <label className="startup-preference-row">
            <span><strong>Project sidebar</strong><small>The project and session list on the left.</small></span>
            <select
              aria-label="Project sidebar on startup"
              value={startupPreferences.leftPane}
              onChange={(event) => setStartupPreferences({ ...startupPreferences, leftPane: event.target.value as StartupPreferences["leftPane"] })}
            >
              <option value="open">Open</option>
              <option value="remember">Remember last state</option>
              <option value="collapsed">Collapsed</option>
            </select>
          </label>
          <label className="startup-preference-row">
            <span><strong>Details panel</strong><small>The contextual panel on the right.</small></span>
            <select
              aria-label="Details panel on startup"
              value={startupPreferences.rightPane}
              onChange={(event) => setStartupPreferences({ ...startupPreferences, rightPane: event.target.value as StartupPreferences["rightPane"] })}
            >
              <option value="remember">Remember last state</option>
              <option value="open">Open</option>
              <option value="collapsed">Collapsed</option>
            </select>
          </label>
        </div>
      </div>
      <div className="dashboard-card" data-help="Choose how navigation and projects appear. These preferences change only the local interface.">
        <h3>Navigation and project list</h3>
        <label className="toggle-row" data-help="Show Overview, Discover, Safe Manage, Recover and Settings as icon buttons next to the Code Hangar title. Hover a button to read its name in the status bar. The CH logo also opens this menu on hover.">
          <input type="checkbox" checked={showTopbarNav} onChange={(event) => setShowTopbarNav(event.target.checked)} />
          <span><strong>Show navigation icons in the top bar</strong><small>Off by default to keep the header focused; the sidebar and CH menu remain available.</small></span>
        </label>
        <label className="toggle-row" data-help="Show each project's full local path beneath its name in the project list.">
          <input type="checkbox" checked={showAllProjectPaths} onChange={(event) => setShowAllProjectPaths(event.target.checked)} />
          <span><strong>Show project paths</strong><small>Keep full root paths visible beneath project names.</small></span>
        </label>
        <label className="toggle-row" data-help="Show or hide the built-in demo projects. This never changes real projects or scan folders.">
          <input type="checkbox" checked={demosVisible} onChange={(event) => setDemosVisible(event.target.checked)} />
          <span>
            <strong>Show demo projects</strong>
            <small>{demoVisibilityAutomatic ? "Automatic until you choose: demos appear only when there are no real projects." : "Using your saved local preference."}</small>
          </span>
        </label>
        <button type="button" className="tour-replay-button" onClick={replayTour} data-help="Replay the guided walkthrough for this installed edition using your current or first available project.">
          <PlayCircle size={14} /> Replay guided tour
        </button>
      </div>
      <div className="dashboard-card" data-help="Reset saved pane widths and collapse state if the layout becomes cramped.">
        <h3>Layout reset</h3>
        <button type="button" className="secondary-button" data-help="Restore default sidebar, details pane and file tree widths." onClick={resetLayout}>
          <RotateCcw size={14} /> Reset pane widths
        </button>
      </div>
    </section>
  );
}

export function SettingsFoldersView({
  roots,
  rootIsScanning,
  startRootScan,
  toggleRoot,
  unregisterRoot,
  latestScanStatus,
  scanStatusList,
  cancelScan,
  onRescanAll,
  onCompactDatabase,
  compactBusy,
  onResetAll
}: {
  roots: ScanRoot[];
  rootIsScanning: (rootId: number) => boolean;
  startRootScan: (rootId: number) => void;
  toggleRoot: (root: ScanRoot) => void;
  unregisterRoot: (rootId: number) => void;
  latestScanStatus: ScanStatus | null;
  scanStatusList: ScanStatus[];
  cancelScan: (jobId: string) => void;
  onRescanAll: () => void;
  onCompactDatabase: () => void;
  compactBusy: boolean;
  onResetAll: () => void;
}) {
  const anyScanRunning = roots.some((root) => rootIsScanning(root.id));
  const [rootQuery, setRootQuery] = useState("");
  const [rootFilter, setRootFilter] = useState<ScanRootFilter>("all");
  const [rootsExpanded, setRootsExpanded] = useState(false);
  const rootSummary = useMemo(() => summarizeScanRoots(roots), [roots]);
  const visibleRoots = useMemo(() => filterScanRoots(roots, rootQuery, rootFilter), [roots, rootFilter, rootQuery]);
  const rootSearchActive = rootQuery.trim().length > 0;
  const rootPreview = useMemo(
    () => previewScanRoots(visibleRoots, { expanded: rootsExpanded, searchActive: rootSearchActive }),
    [rootSearchActive, rootsExpanded, visibleRoots]
  );
  return (
    <section className="pane-section compact">
      <div className="settings-root-overview" data-help="A quick summary of the local scan folders Code Hangar is tracking. These are metadata roots only; project files stay on disk.">
        <div className="settings-root-copy">
          <span>Local inventory</span>
          <strong>{rootSummary.total} scan folder{rootSummary.total === 1 ? "" : "s"}</strong>
          <small>{rootSummary.enabled} enabled · {rootSummary.disabled} disabled · files stay untouched</small>
        </div>
        <div className="settings-root-facts" aria-label="Scan folder summary">
          <span><strong>{rootSummary.total}</strong><small>Total</small></span>
          <span><strong>{rootSummary.enabled}</strong><small>Enabled</small></span>
          <span><strong>{rootSummary.disabled}</strong><small>Disabled</small></span>
        </div>
      </div>
      <div className="settings-maintenance-grid">
        <div className="dashboard-card settings-maintenance-card" data-help="Safe maintenance reads local files or compacts Code Hangar's local database. It does not delete project files.">
          <h3>Refresh and compact</h3>
          <p className="muted help-copy">Use these when totals look stale or after metadata cleanup. They are local maintenance actions.</p>
          <div className="root-actions">
            <button type="button" disabled={anyScanRunning || roots.length === 0} data-help="Re-scan every enabled root in a single job using the current scan rules." onClick={onRescanAll}>Re-scan all roots</button>
            <button type="button" disabled={compactBusy || anyScanRunning} data-help="Compact (VACUUM) the local database to return freed index space to your disk. Refused while a scan is running." onClick={onCompactDatabase}>{compactBusy ? "Compacting…" : "Compact database"}</button>
          </div>
          {anyScanRunning ? <small>A scan is running. Maintenance is available once it finishes.</small> : null}
        </div>
        <div className="dashboard-card settings-reset-card" data-help="Start over unregisters Code Hangar's local inventory only. It does not delete project files from disk.">
          <h3>Start over safely</h3>
          <p className="muted help-copy">Unregister every project and scan folder from Code Hangar, then rebuild later with Deep Scan. Real folders stay where they are.</p>
          <button type="button" className="danger-button" disabled={anyScanRunning} onClick={onResetAll} data-help="Opens a confirmation. Unregisters every scan root and project from Code Hangar's local index. Files on disk are untouched.">
            Unregister all projects…
          </button>
        </div>
      </div>
      {roots.length === 0 ? <p className="muted">No local roots registered.</p> : null}
      {roots.length > 0 ? (
        <>
          <div className="root-list-header">
            <strong>Registered folders</strong>
            <span>{scanRootListSummaryLabel(rootPreview, visibleRoots.length)}</span>
          </div>
          <div className="root-list-toolbar" data-help="Filter registered scan roots without changing the local inventory.">
            <div className="root-search">
              <label htmlFor="scan-root-filter">Find folder</label>
              <input
                id="scan-root-filter"
                type="search"
                value={rootQuery}
                placeholder="Filter folders..."
                onChange={(event) => setRootQuery(event.target.value)}
              />
            </div>
            <div className="root-filter-tabs" aria-label="Scan root filter">
              {(["all", "enabled", "disabled"] as const).map((filter) => (
                <button key={filter} type="button" className={rootFilter === filter ? "active" : ""} onClick={() => setRootFilter(filter)}>
                  {rootFilterLabel(filter)}
                </button>
              ))}
            </div>
            <span className="root-filter-count">{visibleRoots.length} of {roots.length}</span>
          </div>
        </>
      ) : null}
      {rootsExpanded && !rootSearchActive ? (
        <button
          type="button"
          className="root-list-more"
          onClick={() => setRootsExpanded(false)}
          data-help={`Collapse Scan folders back to the first ${SCAN_ROOT_PREVIEW_LIMIT} registered roots so maintenance controls stay close.`}
        >
          Show fewer scan folders
        </button>
      ) : null}
      {rootPreview.roots.map((root) => (
        <div className="root-row" key={root.id} data-help={`Scan root ${root.path}. These actions update Code Hangar metadata only.`}>
          <span>{root.path}</span>
          <div className="root-actions">
            <button type="button" disabled={rootIsScanning(root.id)} data-help={`Re-scan metadata for ${root.path}.`} onClick={() => void startRootScan(root.id)}>Re-scan</button>
            <button type="button" disabled={root.enabled && rootIsScanning(root.id)} data-help={root.enabled ? `Disable future scans for ${root.path}.` : `Enable scans for ${root.path}.`} onClick={() => void toggleRoot(root)}>{root.enabled ? "Disable" : "Enable"}</button>
            <button type="button" disabled={rootIsScanning(root.id)} data-help={`Open a confirmation to unregister ${root.path} from Code Hangar without deleting files.`} onClick={() => void unregisterRoot(root.id)}>Unregister</button>
          </div>
          {rootIsScanning(root.id) ? <small>Scan running. Cancel it before disabling or unregistering.</small> : null}
        </div>
      ))}
      {rootPreview.compacted ? (
        <button
          type="button"
          className="root-list-more"
          onClick={() => setRootsExpanded(true)}
          data-help="Show every registered scan folder. Use Find folder to jump directly without expanding the whole list."
        >
          Show all scan folders ({rootPreview.hiddenCount} more)
        </button>
      ) : null}
      {roots.length > 0 && visibleRoots.length === 0 ? <p className="muted result-empty">No scan folders match the current filter.</p> : null}
      {latestScanStatus ? (
        <div className="scan-status">
          {scanStatusList.slice(-3).map((status) => (
            <div className="scan-status-row" key={status.jobId}>
              <p>{status.message}</p>
              <ScanProgressSummary status={status} />
              {["running", "cancelling"].includes(status.state) ? <button type="button" data-help="Cancel the active scan at the next safe checkpoint." onClick={() => void cancelScan(status.jobId)}>Cancel scan</button> : null}
              {status.error ? <small className="scan-error">{status.error}</small> : null}
            </div>
          ))}
        </div>
      ) : null}
    </section>
  );
}

export function SettingsDiagnosticsExportCard() {
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function exportDiagnostics() {
    setBusy(true);
    setStatus(null);
    setError(null);
    try {
      const path = await api.pickDiagnosticsPath();
      if (!path) return;
      const result = await api.diagnosticsExport(path);
      setStatus(`Redacted diagnostic bundle exported (${Math.max(1, Math.ceil(result.bytesWritten / 1024))} KiB).`);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dashboard-card settings-diagnostics-card" data-help="Export a local support summary without project names, file names, paths, sessions, prompts, source, diffs, logs, endpoints, credentials or model configuration.">
      <div className="card-title-row">
        <div>
          <h3>Redacted diagnostics</h3>
          <p className="muted help-copy">Create a small JSON support bundle with build, safety, aggregate inventory and hardware-capacity facts.</p>
        </div>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => void exportDiagnostics()}>
          <Download size={14} /> {busy ? "Exporting…" : "Export diagnostics"}
        </button>
      </div>
      <p className="settings-diagnostics-privacy"><ShieldCheck size={14} /> Project identity and evidence content are omitted by construction.</p>
      {status ? <small className="settings-diagnostics-status" role="status">{status}</small> : null}
      {error ? <small className="scan-error" role="alert">{error}</small> : null}
    </div>
  );
}

function rootFilterLabel(filter: ScanRootFilter) {
  if (filter === "enabled") return "Enabled";
  if (filter === "disabled") return "Disabled";
  return "All";
}

function ScanProgressSummary({ status }: { status: ScanStatus }) {
  const progress = scanProgressParts(status);
  const isRunning = ["running", "cancelling"].includes(status.state);
  return (
    <div className="scan-progress-summary" data-help="Scan progress is exact for items already visited. Existing roots reuse previous inventory estimates; new roots are counted before indexing. Percent stays below 100 until the scan really finishes.">
      <div className="scan-progress-track" aria-label="Scan progress">
        <span
          className={progress.percent == null && isRunning ? "scan-progress-fill indeterminate" : "scan-progress-fill"}
          style={progress.percent == null ? undefined : { width: `${progress.percent}%` }}
        />
      </div>
      <small>{progress.detailText} · {status.indexedDocuments.toLocaleString()} indexed</small>
      <small className="muted">
        Phase: {status.scanPhase} · Workers: {status.workerCount ?? "unknown"} · {progress.bottleneckText}
      </small>
    </div>
  );
}

export function SettingsProtectionView({
  zones,
  zoneAllowSensitiveReveal,
  setZoneAllowSensitiveReveal,
  zoneRelaxNonStrongPreview,
  setZoneRelaxNonStrongPreview,
  zoneShowProtectedMetadata,
  setZoneShowProtectedMetadata
}: {
  zones: ProtectedZone[];
  zoneAllowSensitiveReveal: boolean;
  setZoneAllowSensitiveReveal: (value: boolean) => void;
  zoneRelaxNonStrongPreview: boolean;
  setZoneRelaxNonStrongPreview: (value: boolean) => void;
  zoneShowProtectedMetadata: boolean;
  setZoneShowProtectedMetadata: (value: boolean) => void;
}) {
  const activeVisibilityMode = protectionVisibilityMode(
    zoneAllowSensitiveReveal,
    zoneRelaxNonStrongPreview
  );
  const visibilityModeLabel = activeVisibilityMode === "auto"
    ? "Auto-preview on"
    : activeVisibilityMode === "reveal"
      ? "Reveal by file"
      : "Locked by default";
  const chooseVisibilityMode = (mode: ProtectionVisibilityMode) => {
    const next = protectionVisibilityFlags(mode);
    setZoneAllowSensitiveReveal(next.allowSensitiveReveal);
    setZoneRelaxNonStrongPreview(next.relaxNonStrongPreview);
  };
  const visibilityModeNote = activeVisibilityMode === "auto"
    ? "Higher exposure: opening non-strong protected or sensitive text shows it immediately for this session, including secrets such as .env. Strong zones stay blocked."
    : activeVisibilityMode === "reveal"
      ? "Non-strong text can be revealed one file at a time after confirmation. Nothing is indexed, cached or logged; .ssh and system/app zones stay blocked."
      : "Recommended. File content stays blocked while names, paths and sizes remain available for local review.";

  return (
    <section className="pane-section">
      <div className="dashboard-card warning-card" data-help="Temporary local visibility changes only this UI session. It does not index or persist revealed content.">
        <div className="protection-card-title">
          <h3>Temporary local visibility</h3>
          <span>{visibilityModeLabel}</span>
        </div>
        <div className="protection-state-grid" role="group" aria-label="Temporary content visibility" data-help="Choose one session-only visibility mode. Strong zones stay blocked in every mode.">
          <button
            type="button"
            className={`protection-state-card ${activeVisibilityMode === "locked" ? "active" : ""}`}
            aria-pressed={activeVisibilityMode === "locked"}
            onClick={() => chooseVisibilityMode("locked")}
          >
            <ShieldCheck size={15} />
            <strong>Default</strong>
            <span>Blocked previews, metadata only.</span>
          </button>
          <button
            type="button"
            className={`protection-state-card ${activeVisibilityMode === "reveal" ? "active" : ""}`}
            aria-pressed={activeVisibilityMode === "reveal"}
            onClick={() => chooseVisibilityMode("reveal")}
          >
            <Shield size={15} />
            <strong>Reveal by file</strong>
            <span>One intentional click per non-strong text file.</span>
          </button>
          <button
            type="button"
            className={`protection-state-card caution ${activeVisibilityMode === "auto" ? "active" : ""}`}
            aria-pressed={activeVisibilityMode === "auto"}
            onClick={() => chooseVisibilityMode("auto")}
          >
            <AlertTriangle size={15} />
            <strong>Auto-preview</strong>
            <span>Immediate non-strong text preview this session.</span>
          </button>
        </div>
        <p className={`protection-mode-note ${activeVisibilityMode === "auto" ? "caution" : ""}`} aria-live="polite">
          {visibilityModeNote}
        </p>
      </div>
      <div className="dashboard-card" data-help="A display preference only. It changes styling in the file tree and never reveals file contents.">
        <h3>Tree display</h3>
        <p className="muted help-copy">Protected and sensitive files are always listed in the tree with their name, path and size. This option only changes how they look — it never reveals any content.</p>
        <label className="toggle-row" data-help="Highlight protected and sensitive rows in the file tree. Purely visual; it does not unlock metadata or content.">
          <input type="checkbox" checked={zoneShowProtectedMetadata} onChange={(event) => setZoneShowProtectedMetadata(event.target.checked)} />
          <span>
            <strong>Highlight protected files in the tree</strong>
            <small>Adds a visual marker to protected/sensitive rows such as .env, credentials.json or .git/config. Names, paths and sizes are listed either way; content stays blocked.</small>
          </span>
        </label>
      </div>
      <div className="zone-list-header">
        <strong>Protected rules</strong>
        <span>{zones.length} local pattern{zones.length === 1 ? "" : "s"}</span>
      </div>
      <div className="zone-list">
        {zones.map((zone) => (
          <div className="zone-row" key={zone.id} data-help={`Protected Zone ${zone.pattern}: ${protectedZoneHelp(zone.level)}. Pattern ${zone.pattern}, level ${zone.level}.`}>
            <strong>{zone.pattern}</strong>
            <span>{zone.level}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

function fontSizeLabel(value: "compact" | "comfortable" | "large" | "xlarge") {
  if (value === "compact") return "Compact";
  if (value === "large") return "Large";
  if (value === "xlarge") return "Extra large";
  return "Comfortable";
}

function fontSizeHelp(value: "compact" | "comfortable" | "large" | "xlarge") {
  if (value === "compact") return "Use smaller UI text to fit more rows on screen.";
  if (value === "large") return "Increase UI text for easier reading on high DPI or dark mode.";
  if (value === "xlarge") return "Use the largest text size for maximum readability.";
  return "Use the default balanced text size.";
}

function densityLabel(value: "compact" | "comfortable" | "spacious") {
  if (value === "compact") return "Compact";
  if (value === "spacious") return "Spacious";
  return "Comfortable";
}

function densityHelp(value: "compact" | "comfortable" | "spacious") {
  if (value === "compact") return "Reduce spacing so project, tree and activity lists show more rows.";
  if (value === "spacious") return "Increase spacing for easier clicking and scanning.";
  return "Use the default spacing.";
}
