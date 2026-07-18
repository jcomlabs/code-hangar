import { AlertTriangle, Bot, Copy, Download, KeyRound, PlayCircle, Plug, RotateCcw, Shield, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { ConceptHelp } from "../BeginnerHelp";
import { scanProgressParts } from "../scanProgress";
import type {
  AgentActionRequest,
  ResolveInputs,
  AutomationActivityEntry,
  AutomationAgentSummary,
  AutomationCredential,
  AutomationStatus,
  ConnectedAppStatus,
  ProjectSummary,
  ProtectedZone,
  ScanRoot,
  ScanStatus
} from "../types";
import type { StartupPreferences } from "../workspaceRoute";
import { SectionTitle, protectedZoneHelp } from "../ui";

const connectorApiLoader = import.meta.env.MODE === "test" || import.meta.env.MODE === "connector"
  ? () => import("../connectorApi").then((module) => module.connectorApi)
  : null;

async function requireConnectorApi() {
  if (!connectorApiLoader) {
    throw new Error("This feature is not compiled into this frontend edition.");
  }
  return connectorApiLoader();
}

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
          <span><strong>Show navigation icons in the top bar</strong><small>On by default: quick Overview-to-Settings access from the header.</small></span>
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

const AUTOMATION_SCOPE_OPTIONS = [
  { id: "read_structure", label: "Project structure", help: "Read project and context-file metadata, never file bodies." },
  { id: "read_graph", label: "Dependency graph & cleanup", help: "Read the project graph, node relationships, and orphan/duplicate candidates. Structure only — never file bodies." },
  { id: "read_body", label: "File bodies", help: "Read non-sensitive file bodies inside selected projects. Protected policy still wins." },
  { id: "comments_read", label: "Read comments", help: "List the comments on selected projects, folders and files." },
  { id: "comments_write", label: "Write its own comments", help: "Add and edit only its OWN comments. Also needs the global AI write toggle; it can never touch a comment you wrote." },
  { id: "build_plan", label: "Build impact previews", help: "Build read-only OperationPlan and Risk Report previews for selected projects." },
  { id: "execute_plan", label: "Request safe actions", help: "Request verified backup or holding-area moves. A fresh human confirmation token remains mandatory." },
  { id: "history_search", label: "Search project sessions", help: "Run bounded, redacted, on-demand history search for an explicitly selected project." }
] as const;

export function SettingsAutomationView({
  status,
  agents,
  activity,
  credential,
  projects,
  currentFile,
  busy,
  error,
  onRefresh,
  onRegister,
  onRevoke,
  onForget,
  onGrantRead,
  onCopy,
  onClearCredential
}: {
  status: AutomationStatus | null;
  agents: AutomationAgentSummary[];
  activity: AutomationActivityEntry[];
  credential: AutomationCredential | null;
  projects: ProjectSummary[];
  currentFile: { nodeId: number; displayName: string } | null;
  busy: boolean;
  error: string | null;
  onRefresh: () => void;
  onRegister: (name: string, scopes: string[], projectIds: number[]) => void;
  onRevoke: (agentId: number) => void;
  onForget: (agentId: number) => void;
  onGrantRead: (agentId: number, nodeId: number) => void;
  onCopy: (value: string) => void;
  onClearCredential: () => void;
}) {
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<string[]>(["read_structure"]);
  const [projectIds, setProjectIds] = useState<number[]>([]);
  const [grantAgentId, setGrantAgentId] = useState<number | null>(null);

  useEffect(() => {
    const firstEnabled = agents.find((agent) => agent.enabled)?.id ?? null;
    if (grantAgentId === null || !agents.some((agent) => agent.id === grantAgentId && agent.enabled)) {
      setGrantAgentId(firstEnabled);
    }
  }, [agents, grantAgentId]);

  const toggleScope = (scope: string) => {
    setScopes((current) => current.includes(scope) ? current.filter((item) => item !== scope) : [...current, scope]);
  };
  const toggleProject = (projectId: number) => {
    setProjectIds((current) => current.includes(projectId) ? current.filter((item) => item !== projectId) : [...current, projectId]);
  };

  return (
    <section className="pane-section compact automation-settings">
      <SectionTitle icon={<Bot size={15} />} label="Local automation" trailing={<ConceptHelp concept="localAutomation" />} />
      <p className="muted help-copy">Optional advanced integration for local tools. It uses an authenticated Windows named pipe, never an external network listener. Every agent is limited to explicit projects and scopes.</p>
      {!status ? <p className="muted">Checking this build...</p> : null}
      {status && !status.enabled ? (
        <div className="dashboard-card" data-help="This executable has no local automation server. Core navigation, discovery and safe management remain unchanged.">
          <h3>Not included in this build</h3>
          <p>{status.message}</p>
        </div>
      ) : null}
      {status?.enabled ? (
        <>
          <div className="dashboard-card" data-help="This endpoint is a Windows named pipe restricted to local-machine clients. Every non-status request also needs a registered token.">
            <div className="card-title-row">
              <h3>Local endpoint</h3>
              <button type="button" className="icon-button" aria-label="Copy local endpoint" onClick={() => status.endpoint && onCopy(status.endpoint)} data-help="Copy the local named-pipe endpoint for configuring a trusted local tool."><Copy size={15} /></button>
            </div>
            <code className="path-code">{status.endpoint}</code>
            <p className="muted help-copy">Protocol {status.protocol}. Guest requests see capabilities only; project data always requires authentication.</p>
          </div>

          {credential ? (
            <div className="dashboard-card warning-card" data-help="The raw token is shown once. Code Hangar stores only its hash; closing this card cannot be undone except by registering a new token.">
              <div className="card-title-row"><h3>New credential: {credential.agent.name}</h3><KeyRound size={16} /></div>
              <p>Store this token in the local tool now. It will not be shown again.</p>
              <code className="credential-token">{credential.token}</code>
              <div className="inline-actions">
                <button type="button" onClick={() => onCopy(credential.token)} data-help="Copy the one-time local authentication token."><Copy size={14} /> Copy token</button>
                <button type="button" className="secondary-button" onClick={onClearCredential} data-help="Hide this one-time token. Register a new credential if it is lost.">I stored it</button>
              </div>
            </div>
          ) : null}

          <div className="dashboard-card" data-help="Register a local tool with the smallest scopes and project set it needs. The token is generated locally and displayed once.">
            <h3>Register a local tool</h3>
            <label className="field-label">
              Name
              <input value={name} maxLength={80} onChange={(event) => setName(event.target.value)} placeholder="Example: local ChatGPT helper" />
            </label>
            <div className="automation-choice-list">
              <strong>Allowed capabilities</strong>
              {AUTOMATION_SCOPE_OPTIONS.map((scope) => (
                <label className="toggle-row" key={scope.id} data-help={scope.help}>
                  <input type="checkbox" checked={scopes.includes(scope.id)} onChange={() => toggleScope(scope.id)} />
                  <span><strong>{scope.label}</strong><small>{scope.help}</small></span>
                </label>
              ))}
            </div>
            <div className="automation-choice-list project-scope-list">
              <strong>Allowed projects</strong>
              {projects.map((project) => (
                <label className="toggle-row" key={project.id} data-help={`Allow this credential to address only project ${project.name}.`}>
                  <input type="checkbox" checked={projectIds.includes(project.id)} onChange={() => toggleProject(project.id)} />
                  <span><strong>{project.name}</strong><small>{project.path}</small></span>
                </label>
              ))}
            </div>
            <button type="button" disabled={busy || !name.trim() || scopes.length === 0 || projectIds.length === 0} onClick={() => onRegister(name, scopes, projectIds)} data-help="Generate one local token for exactly these scopes and projects. No remote account is created.">
              <ShieldCheck size={15} /> Register and show token once
            </button>
          </div>

          <div className="dashboard-card" data-help="Revoking a credential immediately blocks its token and revokes all temporary read grants.">
            <div className="card-title-row"><h3>Registered local tools</h3><button type="button" className="secondary-button slim" disabled={busy} onClick={onRefresh}>Refresh</button></div>
            {agents.length === 0 ? <p className="muted">No local tools registered.</p> : agents.map((agent) => (
              <div className="automation-agent-row" key={agent.id}>
                <div><strong>{agent.name}</strong><small>{agent.enabled ? agent.scopes.join(" · ") : "Revoked"}</small></div>
                {agent.enabled ? <button type="button" className="danger-button slim" disabled={busy} onClick={() => onRevoke(agent.id)} data-help={`Revoke ${agent.name}, its token and all temporary file grants.`}>Revoke</button> : <button type="button" className="secondary-button slim" disabled={busy} onClick={() => onForget(agent.id)} data-help={`Remove the revoked ${agent.name} registry entry. Its body-free activity records remain.`}>Remove entry</button>}
              </div>
            ))}
          </div>

          <div className="dashboard-card" data-help="Grant one registered tool temporary body access to the file currently open. Sensitive and Protected Zone policy still overrides this grant.">
            <h3>Temporary access to open file</h3>
            {currentFile ? (
              <div className="automation-grant-row">
                <span><strong>{currentFile.displayName}</strong><small>Expires after 10 minutes and is revoked with the credential.</small></span>
                <select value={grantAgentId ?? ""} onChange={(event) => setGrantAgentId(Number(event.target.value) || null)} aria-label="Local tool">
                  <option value="">Choose local tool</option>
                  {agents.filter((agent) => agent.enabled).map((agent) => <option value={agent.id} key={agent.id}>{agent.name}</option>)}
                </select>
                <button type="button" disabled={busy || grantAgentId === null} onClick={() => grantAgentId !== null && onGrantRead(grantAgentId, currentFile.nodeId)} data-help="Allow this tool to request the currently open file body for 10 minutes. Protected policy still applies.">Grant 10 minutes</button>
              </div>
            ) : <p className="muted">Open a file first to create a temporary grant.</p>}
          </div>

          <div className="dashboard-card" data-help="The local activity log records which method was allowed or denied. Response bodies and file content are never stored here.">
            <h3>Local automation activity</h3>
            {activity.length === 0 ? <p className="muted">No local automation requests recorded.</p> : (
              <div className="automation-activity-list">
                {activity.slice(0, 100).map((entry) => (
                  <div key={entry.id}><strong>{entry.method}</strong><span className={entry.status === "allowed" ? "status-good" : "status-warning"}>{entry.status}</span><small>{entry.agentName ?? "Unregistered client"} · {entry.createdAt}</small><p>{entry.detail}</p></div>
                ))}
              </div>
            )}
          </div>
        </>
      ) : null}
      {error ? <p className="scan-error">{error}</p> : null}
    </section>
  );
}

// The "AI App Integration" panel: connect Code Hangar to the AI apps it catalogs
// (Claude, Cursor, ChatGPT) over the Model Context Protocol so they can read — and, only if the user opts
// in, annotate — the curated comment/context knowledge. Self-contained: it loads
// its own status and toggles and only needs a confirmation callback for the
// strongly-signposted, accountable switches and the per-app config writes.
export function SettingsConnectedAppsView({
  confirm,
  projects
}: {
  confirm: (message: string) => Promise<boolean>;
  projects: ProjectSummary[];
}) {
  const [hosts, setHosts] = useState<ConnectedAppStatus[]>([]);
  const [projectIds, setProjectIds] = useState<number[]>([]);
  const [projectQuery, setProjectQuery] = useState("");
  const [writeEnabled, setWriteEnabled] = useState(false);
  const [fullControl, setFullControl] = useState(false);
  const [readOnly, setReadOnly] = useState(false);
  const [requests, setRequests] = useState<AgentActionRequest[]>([]);
  const [approvingId, setApprovingId] = useState<number | null>(null);
  const [backupChecked, setBackupChecked] = useState(true);
  const [backupDir, setBackupDir] = useState<string | null>(null);
  // Extra strengthened-gate state for the mutation request kinds.
  const [holdingDir, setHoldingDir] = useState<string | null>(null);
  const [includeProtected, setIncludeProtected] = useState(false);
  const [liabilityAck, setLiabilityAck] = useState(false);
  const [recommendAck, setRecommendAck] = useState(false);
  const [crossScopeAck, setCrossScopeAck] = useState(false);
  const [typedConfirm, setTypedConfirm] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const visibleProjects = useMemo(() => {
    const query = projectQuery.trim().toLowerCase();
    return projects.filter((project) => !query || `${project.name} ${project.path}`.toLowerCase().includes(query));
  }, [projectQuery, projects]);
  const selectedProjects = useMemo(
    () => projects.filter((project) => projectIds.includes(project.id)),
    [projectIds, projects]
  );

  useEffect(() => {
    const knownIds = new Set(projects.map((project) => project.id));
    setProjectIds((current) => current.filter((projectId) => knownIds.has(projectId)));
  }, [projects]);

  const toggleProject = (projectId: number) => {
    setProjectIds((current) => current.includes(projectId)
      ? current.filter((item) => item !== projectId)
      : [...current, projectId]);
  };

  const reloadRequests = useCallback(async () => {
    const connectorApi = await requireConnectorApi();
    setRequests(await connectorApi.agentRequestsPending());
  }, []);

  const reload = useCallback(async () => {
    try {
      const connectorApi = await requireConnectorApi();
      const [hostList, write, full, frozen, pending] = await Promise.all([
        connectorApi.connectedAppStatus(),
        connectorApi.commentWriteEnabled(),
        connectorApi.mcpFullControlEnabled(),
        connectorApi.mcpReadOnlyMode(),
        connectorApi.agentRequestsPending()
      ]);
      setHosts(hostList);
      setWriteEnabled(write);
      setFullControl(full);
      setReadOnly(frozen);
      setRequests(pending);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  // A request appears out-of-band: a connected app files it while this panel is
  // already open. Total control is the only tier that can queue one, so while it is
  // on, poll for pending requests and also refresh the moment the window regains
  // focus (e.g. right after switching back from the AI app that just asked). Without
  // this, a queued request stayed invisible until the panel was remounted.
  useEffect(() => {
    if (!fullControl) return;
    let cancelled = false;
    let refreshing = false;
    let timer: number | null = null;
    const schedule = (delay: number) => {
      if (cancelled) return;
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(refresh, delay);
    };
    const refresh = () => {
      if (cancelled || refreshing) return;
      if (document.hidden) {
        schedule(30_000);
        return;
      }
      refreshing = true;
      void reloadRequests().catch(() => {}).finally(() => {
        refreshing = false;
        schedule(4_000);
      });
    };
    const onVisibilityChange = () => {
      if (!document.hidden) {
        if (timer !== null) window.clearTimeout(timer);
        schedule(100);
      }
    };
    schedule(4_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [fullControl, reloadRequests]);

  const isMutationKind = (kind: string) =>
    kind === "backup_protected" || kind === "move_to_holding" || kind === "final_remove";
  const describeRequest = (request: AgentActionRequest) => {
    const who = request.agentName;
    switch (request.kind) {
      case "comment_edit":
        return `${who} wants to edit a comment`;
      case "comment_delete":
        return `${who} wants to delete a comment`;
      case "read_body":
        return `${who} wants to read a file's contents`;
      case "backup_protected":
        return `${who} wants to back up a folder, INCLUDING protected/sensitive files`;
      case "move_to_holding":
        return `${who} wants to move ${request.detail ?? "a target"} to the holding area`;
      case "final_remove":
        return `${who} wants to PERMANENTLY delete ${request.detail ?? "a held item"}`;
      default:
        return `${who} filed a request`;
    }
  };

  const approveRequestHelp = (request: AgentActionRequest) => {
    if (request.kind === "read_body") {
      return "Review a temporary file-content grant. The app gets no body text unless you approve.";
    }
    if (isMutationKind(request.kind)) {
      return "Review a privileged disk-action request. Code Hangar revalidates the plan, scopes, backups and final-remove gate before doing anything.";
    }
    return "Review a requested comment change. Backing up the comment first is offered as an easy checkbox.";
  };

  // Opening the approve panel is the first deliberate step; clicking "Approve" raises
  // the second confirmation. For mutation kinds the strengthened gate adds the
  // liability waiver, the recommendation-against, folder pickers and (for delete) a
  // typed confirmation; cross-scope requests add a cross-project authorization.
  const openApprove = (request: AgentActionRequest) => {
    setApprovingId(request.id);
    setBackupChecked(true);
    setBackupDir(null);
    setHoldingDir(null);
    setIncludeProtected(false);
    setLiabilityAck(false);
    setRecommendAck(false);
    setCrossScopeAck(false);
    setTypedConfirm("");
    setError(null);
  };

  const cancelApprove = () => {
    setApprovingId(null);
    setBackupDir(null);
    setHoldingDir(null);
  };

  const chooseHoldingFolder = async () => {
    const dir = await api.pickFolder("Choose a folder to move the target into");
    if (dir) {
      setHoldingDir(dir);
    }
  };

  const chooseBackupFolder = async () => {
    const dir = await api.pickFolder("Choose a safe folder for the comment backup");
    if (dir) {
      setBackupDir(dir);
    }
  };

  const finishApprove = async (request: AgentActionRequest, inputs: ResolveInputs) => {
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.agentRequestResolve(request.id, true, inputs);
      setApprovingId(null);
      setBackupDir(null);
      setHoldingDir(null);
      await reloadRequests();
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const confirmApprove = async (request: AgentActionRequest) => {
    // Comment + read-body kinds keep the light gate (optional easy backup).
    if (!isMutationKind(request.kind)) {
      if ((request.kind === "comment_edit" || request.kind === "comment_delete") && backupChecked && !backupDir) {
        setError("Choose a backup folder, or untick the backup option to proceed without one.");
        return;
      }
      const willBackUp = backupChecked && backupDir != null;
      if (request.kind === "read_body") {
        if (!(await confirm(`Grant ${request.agentName} read access to this file for 10 minutes?`))) return;
        await finishApprove(request, {});
        return;
      }
      if (
        !(await confirm(
          `Apply this change to your comment as yourself? Code Hangar will ${request.kind === "comment_delete" ? "delete" : "edit"} it on your behalf${willBackUp ? " after backing it up" : " WITHOUT a backup"}. This cannot be undone.`
        ))
      ) {
        return;
      }
      await finishApprove(request, { backupDir: willBackUp ? backupDir : null });
      return;
    }

    // Mutation kinds: the strengthened gate.
    const needsBackupFolder = request.kind === "backup_protected" || request.kind === "move_to_holding";
    if (needsBackupFolder && !backupDir) {
      setError("Choose a backup folder before approving.");
      return;
    }
    if (request.kind === "move_to_holding" && !holdingDir) {
      setError("Choose a holding folder before approving.");
      return;
    }
    if (request.kind === "backup_protected" && !includeProtected) {
      setError("Tick the protected-files option to back up sensitive files.");
      return;
    }
    if (!liabilityAck || !recommendAck) {
      setError("Accept both acknowledgements to proceed.");
      return;
    }
    if (request.crossScope && !crossScopeAck) {
      setError("Authorize the cross-project action to proceed.");
      return;
    }
    if (request.kind === "final_remove" && typedConfirm.trim().toUpperCase() !== "DELETE") {
      setError('Type DELETE to confirm the irreversible removal.');
      return;
    }
    if (
      !(await confirm(
        `Code Hangar recommends AGAINST this. Proceed with this ${request.kind === "final_remove" ? "IRREVERSIBLE permanent delete" : request.kind === "move_to_holding" ? "move" : "protected backup"} as yourself? It was requested by a connected app, not by you.`
      ))
    ) {
      return;
    }
    if (
      request.kind === "final_remove" &&
      !(await confirm("Are you absolutely sure? This permanently removes the held item and cannot be undone."))
    ) {
      return;
    }
    await finishApprove(request, {
      backupDir: needsBackupFolder ? backupDir : null,
      holdingRoot: request.kind === "move_to_holding" ? holdingDir : null,
      includeProtectedOptIn: includeProtected,
      crossScopeAuthorized: crossScopeAck
    });
  };

  const rejectRequest = async (request: AgentActionRequest) => {
    if (!(await confirm(`Reject this request from ${request.agentName}? Nothing will change.`))) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.agentRequestResolve(request.id, false, {});
      await reloadRequests();
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleWrite = async () => {
    const next = !writeEnabled;
    if (
      next &&
      !(await confirm(
        "Allow connected AI apps to write comments?\n\nThey can add and edit only their OWN comments — never the ones you wrote. A less capable model could still add noise you would have to clean up. You are enabling AI to write into your knowledge base. Continue?"
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.setCommentWriteEnabled(next);
      setWriteEnabled(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleFullControl = async () => {
    const next = !fullControl;
    if (next) {
      if (
        !(await confirm(
          "Give a connected AI app total-control request privileges?\n\nThis lets a trusted app file requests it could not perform directly: edit/delete protected comments, request temporary file-content access, request protected backups, request moves to the holding area, or request final removal of a held item. Nothing happens until you approve each request inside Code Hangar. Continue?"
        ))
      ) {
        return;
      }
      if (
        !(await confirm(
          "Are you absolutely sure?\n\nEach request still waits for your review. If you approve, Code Hangar acts as you, revalidates scopes, protected locations, file locks, backups and final-removal availability before acting. Enable this advanced tier now?"
        ))
      ) {
        return;
      }
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.setMcpFullControlEnabled(next);
      setFullControl(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleReadOnly = async () => {
    const next = !readOnly;
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.setMcpReadOnlyMode(next);
      setReadOnly(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const register = async (host: ConnectedAppStatus) => {
    if (projectIds.length === 0) {
      setError("Choose at least one project before connecting an AI app.");
      return;
    }
    const projectSummary = selectedProjects.length <= 3
      ? selectedProjects.map((project) => project.name).join(", ")
      : `${selectedProjects.slice(0, 3).map((project) => project.name).join(", ")} and ${selectedProjects.length - 3} more`;
    if (
      !(await confirm(
        `Add Code Hangar to ${host.label}'s configuration for ${projectIds.length} selected project${projectIds.length === 1 ? "" : "s"}?\n\nProjects: ${projectSummary}.\n\nThe file is backed up first and only Code Hangar's own entry is added; everything else is left untouched. The app can read curated context only inside this project scope, and write comments only if you enable AI write mode above.`
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      const updated = await connectorApi.connectedAppRegister(host.host, projectIds);
      // The backend returns only after it has re-read and verified the entry. Reflect that
      // postcondition immediately instead of leaving a stale "Not connected" card until remount.
      setHosts((current) => current.map((item) => (item.host === updated.host ? updated : item)));
      setProjectIds([]);
      setProjectQuery("");
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (host: ConnectedAppStatus) => {
    if (
      !(await confirm(
        `Remove Code Hangar from ${host.label}'s configuration and revoke its token? Your other entries are left untouched.`
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      const updated = await connectorApi.connectedAppRemove(host.host);
      setHosts((current) => current.map((item) => (item.host === updated.host ? updated : item)));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  // The host list is non-empty only when this build includes the connected-app
  // surface; the Local edition has no AI-app integration to configure.
  if (loaded && hosts.length === 0) {
    return (
      <section className="pane-section compact">
        <SectionTitle icon={<Plug size={15} />} label="AI app integration" />
        <div className="dashboard-card" data-help="Connecting AI apps is an opt-in power feature built into the AI Connector edition.">
          <h3>Not included in this build</h3>
          <p>Connecting your AI apps to Code Hangar is compiled into the AI Connector edition. This Local edition keeps that surface absent while retaining local project management.</p>
        </div>
        {error ? <p className="scan-error">{error}</p> : null}
      </section>
    );
  }

  return (
    <section className="pane-section compact">
      <SectionTitle icon={<Plug size={15} />} label="AI app integration" />
      <p className="muted help-copy">
        Let the AI apps you use (Claude, Cursor, ChatGPT) read curated project metadata — and, only if you allow it, annotate —
        over the Model Context Protocol. File contents are not shared by default; a body read requires an explicit scope or a
        temporary request you approve. Access is local and limited to the projects you registered.
      </p>

      {requests.length > 0 ? (
        <div className="dashboard-card warning-card" data-help="A connected app filed a privileged request. Nothing happens until you approve; on approval Code Hangar acts as you and revalidates the relevant safety gate.">
          <h3>Requests awaiting your approval ({requests.length})</h3>
          {requests.map((request) => (
            <div className="automation-agent-row" key={request.id}>
              <div>
                <strong>{describeRequest(request)}</strong>
                {request.currentBody != null ? <small>Now: “{request.currentBody}”</small> : null}
                {request.kind !== "comment_delete" && request.proposedBody ? (
                  <small>Proposed: “{request.proposedBody}”</small>
                ) : null}
              </div>
              {approvingId === request.id ? (
                <div className="request-approve">
                  {isMutationKind(request.kind) ? (
                    <>
                      <p className="comment-error"><AlertTriangle size={13} /> Code Hangar recommends against this. A connected app requested it — not you.</p>
                      {request.kind === "backup_protected" || request.kind === "move_to_holding" ? (
                        <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void chooseBackupFolder()} data-help="Choose the folder the verified backup is written to.">
                          {backupDir ? `Backup folder: ${backupDir}` : "Choose backup folder…"}
                        </button>
                      ) : null}
                      {request.kind === "move_to_holding" ? (
                        <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void chooseHoldingFolder()} data-help="Choose the folder the target is moved into.">
                          {holdingDir ? `Move into: ${holdingDir}` : "Choose holding folder…"}
                        </button>
                      ) : null}
                      {request.kind === "backup_protected" || request.kind === "move_to_holding" ? (
                        <label className="toggle-row">
                          <input type="checkbox" checked={includeProtected} disabled={busy} onChange={(event) => setIncludeProtected(event.target.checked)} />
                          <span><strong>Include protected/sensitive files</strong><small>Copies secret bytes into the backup. You must tick this for a protected backup.</small></span>
                        </label>
                      ) : null}
                      {request.crossScope ? (
                        <label className="toggle-row">
                          <input type="checkbox" checked={crossScopeAck} disabled={busy} onChange={(event) => setCrossScopeAck(event.target.checked)} />
                          <span><strong>Authorize cross-project action</strong><small>This app is not scoped to the target&apos;s project.</small></span>
                        </label>
                      ) : null}
                      <label className="toggle-row">
                        <input type="checkbox" checked={liabilityAck} disabled={busy} onChange={(event) => setLiabilityAck(event.target.checked)} />
                        <span><strong>I accept full responsibility</strong><small>I release Code Hangar from liability for any data loss.</small></span>
                      </label>
                      <label className="toggle-row">
                        <input type="checkbox" checked={recommendAck} disabled={busy} onChange={(event) => setRecommendAck(event.target.checked)} />
                        <span><strong>Proceed against the recommendation</strong><small>I understand Code Hangar advises against this and choose to continue.</small></span>
                      </label>
                      {request.kind === "final_remove" ? (
                        <input type="text" className="comment-input" value={typedConfirm} placeholder="Type DELETE to confirm" disabled={busy} onChange={(event) => setTypedConfirm(event.target.value)} />
                      ) : null}
                    </>
                  ) : request.kind === "read_body" ? (
                    <p className="muted help-copy">Grant {request.agentName} a 10-minute read of this file. No file is changed.</p>
                  ) : (
                    <>
                      <label className="toggle-row" data-help="Recommended, but optional. When ticked, the comment is written to a folder you choose and verified before the change.">
                        <input type="checkbox" checked={backupChecked} disabled={busy} onChange={(event) => setBackupChecked(event.target.checked)} />
                        <span><strong>Back up the comment first</strong><small>Recommended. Untick to proceed without a backup.</small></span>
                      </label>
                      {backupChecked ? (
                        <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void chooseBackupFolder()} data-help="Choose the safe folder the comment backup is written to.">
                          {backupDir ? `Backup folder: ${backupDir}` : "Choose backup folder…"}
                        </button>
                      ) : null}
                    </>
                  )}
                  <div className="inline-actions">
                    <button type="button" className={isMutationKind(request.kind) ? "danger-button slim" : "primary-button slim"} disabled={busy} onClick={() => void confirmApprove(request)} data-help="Perform the action as yourself. You will be asked to confirm once more.">
                      {request.kind === "final_remove" ? "Delete permanently" : isMutationKind(request.kind) ? "Approve action" : request.kind === "read_body" ? "Grant access" : "Approve change"}
                    </button>
                    <button type="button" className="secondary-button slim" disabled={busy} onClick={cancelApprove}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : (
                <div className="inline-actions">
                  <button type="button" className="primary-button slim" disabled={busy} onClick={() => openApprove(request)} data-help={approveRequestHelp(request)}>
                    Approve…
                  </button>
                  <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void rejectRequest(request)} data-help="Reject. Nothing changes.">
                    Reject
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      ) : null}

      <div className={`dashboard-card ${readOnly ? "warning-card" : ""}`} data-help="A master freeze. When on, connected apps can READ but never write or change anything — overriding the toggles below, including requests already awaiting approval.">
        <label className="toggle-row">
          <input type="checkbox" checked={readOnly} disabled={busy} onChange={() => void toggleReadOnly()} />
          <span>
            <strong>Read-only mode — freeze all AI writes</strong>
            <small>
              Off by default. When on, connected apps can still read your curated knowledge, but every write, comment change
              and action is refused — including any request already waiting for your approval. A one-flip safety brake that
              overrides the two settings below.
            </small>
          </span>
        </label>
      </div>

      <div className={`dashboard-card ${writeEnabled ? "warning-card" : ""}`} data-help="Off by default. Connected apps can add and edit only their own comments; they can never touch a comment you wrote.">
        <label className="toggle-row">
          <input type="checkbox" checked={writeEnabled} disabled={busy} onChange={() => void toggleWrite()} />
          <span>
            <strong>Allow AI apps to write comments</strong>
            <small>
              Off by default. When on, a connected app can add and edit its OWN comments. It can never change a comment you
              wrote. A less capable model could still add noise you would clean up — you are accountable for enabling this.
            </small>
          </span>
        </label>
      </div>

      <div className={`dashboard-card ${fullControl ? "danger-card" : ""}`} data-help="Off by default. Lets a trusted app file privileged requests that cannot execute directly: comment changes, temporary file reads, protected backup/move requests and final-remove requests. Every request waits for your approval.">
        <label className="toggle-row">
          <input type="checkbox" checked={fullControl} disabled={busy} onChange={() => void toggleFullControl()} />
          <span>
            <strong><AlertTriangle size={13} /> Give AI total control (advanced)</strong>
            <small>
              Off by default. For a trusted, capable app only. This lets it file privileged requests, not execute them:
              comment changes, temporary file reads, protected backups, holding-area moves and final-remove requests.
              Nothing changes until you approve each request; Code Hangar acts as you and rechecks the safety gates.
            </small>
          </span>
        </label>
      </div>

      <div className="dashboard-card connected-app-project-scope" data-help="Every new AI app connection is restricted to the projects explicitly selected here. No selection never means every project.">
        <div className="card-title-row">
          <h3>Projects for the next connection</h3>
          <span className={projectIds.length > 0 ? "status-good" : "status-warning"}>{projectIds.length} selected</span>
        </div>
        <p className="muted help-copy">Choose the smallest project set this AI app needs. Each app receives a separate scope when you connect it; reconnect the app to change that scope.</p>
        <label className="field-label">
          Find project
          <input value={projectQuery} onChange={(event) => setProjectQuery(event.target.value)} placeholder="Project name or local path" />
        </label>
        <div className="automation-choice-list project-scope-list" role="group" aria-label="Projects allowed for the next AI app connection">
          {visibleProjects.map((project) => (
            <label className="toggle-row" key={project.id} data-help={`Allow the next connected AI app to address project ${project.name}.`}>
              <input type="checkbox" checked={projectIds.includes(project.id)} onChange={() => toggleProject(project.id)} />
              <span><strong>{project.name}</strong><small>{project.path}</small></span>
            </label>
          ))}
          {visibleProjects.length === 0 ? <p className="muted">No projects match this search.</p> : null}
        </div>
        {projectIds.length === 0 ? <p className="warning-inline">Select at least one project. Connect stays locked until you do.</p> : null}
      </div>

      {hosts.map((host) => (
        <div className="dashboard-card" key={host.host} data-help={`Code Hangar registers itself into ${host.label}'s config. The file is backed up and only our entry is changed.`}>
          <div className="card-title-row">
            <h3>{host.label}</h3>
            <span className={host.registered ? "status-good" : host.readable ? "status-muted" : "status-warning"}>
              {host.registered ? "Connected" : !host.readable ? "Config unreadable" : host.configExists ? "Not connected" : "No config yet"}
            </span>
          </div>
          <code className="path-code">{host.configPath}</code>
          <div className="inline-actions">
            {host.registered ? (
              <button type="button" className="secondary-button" disabled={busy} onClick={() => void remove(host)} data-help={`Remove Code Hangar from ${host.label} and revoke its token.`}>
                Disconnect
              </button>
            ) : (
              <button type="button" className="primary-button" disabled={busy || !host.readable || projectIds.length === 0} onClick={() => void register(host)} data-help={projectIds.length === 0 ? "Choose at least one project above before connecting this AI app." : `Add Code Hangar to ${host.label} with a fresh per-app token limited to ${projectIds.length} selected project${projectIds.length === 1 ? "" : "s"}.`}>
                <Plug size={14} /> Connect
              </button>
            )}
          </div>
          {!host.readable ? (
            <p className="comment-error">This app&apos;s config could not be parsed, so Code Hangar will not modify it.</p>
          ) : null}
        </div>
      ))}

      {error ? <p className="scan-error">{error}</p> : null}
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
