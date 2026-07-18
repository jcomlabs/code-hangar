import { ChevronDown, ChevronUp, CircleHelp } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { DanglingAfter, FilePreview, OrphanStatus } from "./types";

// Shared presentational helpers used by App and the extracted view modules.
// Keeping these here (rather than in App.tsx) lets views import them without a
// circular dependency on App.

export function SectionTitle({ icon, label, trailing }: { icon: ReactNode; label: string; trailing?: ReactNode }) {
  return (
    <div className="section-title">
      {icon}
      <span>{label}</span>
      {trailing ? <span className="section-title-trailing">{trailing}</span> : null}
    </div>
  );
}

export interface HelpPopoverAnchor {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface HelpPopoverPlacement {
  left: number;
  top: number;
  maxHeight: number;
  side: "above" | "below";
}

export function placeHelpPopover(
  anchor: HelpPopoverAnchor,
  panelWidth: number,
  panelHeight: number,
  viewportWidth: number,
  viewportHeight: number,
  margin = 12,
  gap = 8
): HelpPopoverPlacement {
  const usableWidth = Math.max(1, viewportWidth - margin * 2);
  const measuredWidth = Math.min(Math.max(1, panelWidth), usableWidth);
  const belowSpace = Math.max(1, viewportHeight - margin - anchor.bottom - gap);
  const aboveSpace = Math.max(1, anchor.top - margin - gap);
  const side = panelHeight <= belowSpace || belowSpace >= aboveSpace ? "below" : "above";
  const maxHeight = side === "below" ? belowSpace : aboveSpace;
  const visibleHeight = Math.min(Math.max(1, panelHeight), maxHeight);
  const maxLeft = Math.max(margin, viewportWidth - measuredWidth - margin);
  const left = Math.min(Math.max(anchor.right - measuredWidth, margin), maxLeft);
  const desiredTop = side === "below"
    ? anchor.bottom + gap
    : anchor.top - gap - visibleHeight;
  const maxTop = Math.max(margin, viewportHeight - visibleHeight - margin);
  return {
    left,
    top: Math.min(Math.max(desiredTop, margin), maxTop),
    maxHeight,
    side
  };
}

export function HelpPopover({
  title,
  label = "What is this?",
  compact = false,
  children
}: {
  title: string;
  label?: string;
  compact?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [placement, setPlacement] = useState<HelpPopoverPlacement | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const portalTarget = typeof document === "undefined"
    ? null
    : document.querySelector<HTMLElement>(".app-shell") ?? document.body;

  useLayoutEffect(() => {
    if (!open) {
      setPlacement(null);
      return;
    }
    const updatePlacement = () => {
      const trigger = triggerRef.current;
      const panel = panelRef.current;
      if (!trigger || !panel) return;
      const anchor = trigger.getBoundingClientRect();
      const bounds = panel.getBoundingClientRect();
      setPlacement(placeHelpPopover(
        anchor,
        bounds.width,
        bounds.height,
        window.innerWidth,
        window.innerHeight
      ));
    };
    updatePlacement();
    window.addEventListener("resize", updatePlacement);
    window.addEventListener("scroll", updatePlacement, true);
    window.visualViewport?.addEventListener("resize", updatePlacement);
    return () => {
      window.removeEventListener("resize", updatePlacement);
      window.removeEventListener("scroll", updatePlacement, true);
      window.visualViewport?.removeEventListener("resize", updatePlacement);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const dismissOnPointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (triggerRef.current?.contains(target) || panelRef.current?.contains(target)) return;
      setOpen(false);
    };
    const dismissOnKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus({ preventScroll: true });
    };
    window.addEventListener("pointerdown", dismissOnPointer);
    window.addEventListener("keydown", dismissOnKey);
    return () => {
      window.removeEventListener("pointerdown", dismissOnPointer);
      window.removeEventListener("keydown", dismissOnKey);
    };
  }, [open]);

  const panel = open && portalTarget ? createPortal(
    <div
      id={panelId}
      ref={panelRef}
      className="help-popover-panel"
      role="note"
      data-side={placement?.side ?? "below"}
      style={placement
        ? { left: placement.left, top: placement.top, maxHeight: placement.maxHeight }
        : { left: 0, top: 0, visibility: "hidden" }}
    >
      <strong>{title}</strong>
      <div>{children}</div>
    </div>,
    portalTarget
  ) : null;

  return (
    <span className={`help-popover${compact ? " compact" : ""}`}>
      <button
        ref={triggerRef}
        className="help-popover-trigger"
        type="button"
        aria-label={compact ? `Explain ${title}` : undefined}
        aria-expanded={open}
        aria-controls={panelId}
        data-help={`Open a simple explanation of ${title}.`}
        onClick={() => setOpen((value) => !value)}
      >
        <CircleHelp size={15} />
        {compact ? null : <span>{label}</span>}
      </button>
      {panel}
    </span>
  );
}

export function ExpandableText({
  text,
  className = "",
  threshold = 180,
  expandLabel = "Show full text"
}: {
  text: string;
  className?: string;
  threshold?: number;
  expandLabel?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const canCollapse = text.length > threshold || text.split(/\r?\n/).length > 3;
  return (
    <div className={`expandable-text${expanded ? " expanded" : ""}${className ? ` ${className}` : ""}`}>
      <div>{text}</div>
      {canCollapse ? (
        <button type="button" onClick={() => setExpanded((value) => !value)} aria-expanded={expanded}>
          {expanded ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
          {expanded ? "Show less" : expandLabel}
        </button>
      ) : null}
    </div>
  );
}

export function formatBytes(value: number) {
  if (!Number.isFinite(value)) return "Unknown";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let scaled = Math.max(0, value);
  let unitIndex = 0;
  while (scaled >= 1024 && unitIndex < units.length - 1) {
    scaled /= 1024;
    unitIndex += 1;
  }
  return unitIndex === 0 ? `${Math.round(scaled)} B` : `${scaled.toFixed(1)} ${units[unitIndex]}`;
}

export function formatOptionalBytes(value?: number | null) {
  return value == null ? "Unknown" : formatBytes(value);
}

export function plainConfidenceLabel(value?: string | null, subject = "match") {
  switch (value?.trim().toLowerCase()) {
    case "high":
    case "strong":
      return `Strong ${subject}`;
    case "medium":
    case "moderate":
      return `Possible ${subject}`;
    case "low":
    case "weak":
      return `Weak ${subject}`;
    default:
      return value ? `${value} ${subject}` : `${subject.charAt(0).toUpperCase()}${subject.slice(1)} not rated`;
  }
}

export function storedBooleanPreference(value: string | null, fallback: boolean) {
  if (value === "true") return true;
  if (value === "false") return false;
  return fallback;
}

type PreviewFileIdentity = Pick<FilePreview, "displayName" | "displayPath" | "path" | "fileKind">;

export function previewFileTypeLabel(preview?: PreviewFileIdentity | null) {
  if (!preview) return "None";
  if (preview.fileKind === "directory") return "Folder";
  if (preview.fileKind === "symlink") return "Link or cloud placeholder";
  if (preview.fileKind === "binary") return "Binary";
  if (preview.fileKind === "unsupported") return "Unsupported";

  const name = preview.displayName.toLowerCase();
  const extension = fileExtension(name);
  if (name === "package.json") return "JSON package manifest";
  if (["tsconfig.json", "jsconfig.json", "composer.json"].includes(name)) return "JSON config";
  if (extension === "json" || extension === "jsonc") return "JSON";
  if (extension === "yaml" || extension === "yml") return "YAML";
  if (extension === "toml") return "TOML";
  if (extension === "md" || extension === "mdx" || extension === "markdown") return "Markdown";
  if (["js", "jsx", "ts", "tsx", "rs", "py", "sh", "ps1", "css", "html"].includes(extension)) return "Source text";
  if (preview.fileKind === "markdown") return "Context text";
  if (preview.fileKind === "text") return "Text";
  return preview.fileKind;
}

export function quickOpenLocationLabel(path: string, projectName?: string | null, options?: { compactLocalPaths?: boolean }) {
  const rawDisplayPath = displayLocalPath(path).trim() || "Project root";
  const displayPath = options?.compactLocalPaths && isAbsoluteLocalPath(rawDisplayPath)
    ? compactLocalPath(rawDisplayPath)
    : rawDisplayPath;
  return projectName ? `${projectName} / ${displayPath}` : displayPath;
}

export function displayLocalPath(path: string) {
  return path
    .replace(/^\\+\?\\UNC\\/i, "\\\\")
    .replace(/^\\+\?\\/i, "");
}

export function compactLocalPath(path: string, trailingSegments = 2) {
  const displayPath = displayLocalPath(path).replace(/\//g, "\\");
  const normalizedTrailingSegments = Math.max(1, Math.floor(trailingSegments));
  const driveMatch = /^([a-z]:\\)(.*)$/i.exec(displayPath);
  if (driveMatch) {
    const [, root, rest] = driveMatch;
    const segments = rest.split("\\").filter(Boolean);
    if (segments.length <= normalizedTrailingSegments + 1) return displayPath;
    return `${root}...\\${segments.slice(-normalizedTrailingSegments).join("\\")}`;
  }

  const uncMatch = /^(\\\\[^\\]+\\[^\\]+)\\?(.*)$/i.exec(displayPath);
  if (uncMatch) {
    const [, root, rest] = uncMatch;
    const segments = rest.split("\\").filter(Boolean);
    if (segments.length <= normalizedTrailingSegments) return displayPath;
    return `${root}\\...\\${segments.slice(-normalizedTrailingSegments).join("\\")}`;
  }

  const segments = displayPath.split("\\").filter(Boolean);
  if (segments.length <= normalizedTrailingSegments + 1) return displayPath;
  return `...\\${segments.slice(-normalizedTrailingSegments).join("\\")}`;
}

function isAbsoluteLocalPath(path: string) {
  return /^[a-z]:\\/i.test(path) || /^\\\\/.test(path);
}

function fileExtension(name: string) {
  const index = name.lastIndexOf(".");
  if (index <= 0 || index === name.length - 1) return "";
  return name.slice(index + 1);
}

/** A local date-time string for a file timestamp (epoch ms), or "—" when absent. */
export function formatTimestamp(ms?: number | null) {
  if (ms == null) return "—";
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  });
}

export function previewStateLabel(state?: FilePreview["state"]) {
  switch (state) {
    case "ready":
      return "Ready to preview";
    case "blocked":
      return "Blocked by policy";
    case "missing":
      return "Missing from inventory";
    case "unsupported":
      return "Preview unsupported";
    default:
      return "No file selected";
  }
}

export function previewStateHelp(state?: FilePreview["state"]) {
  switch (state) {
    case "ready":
      return "The selected file has been read safely and can be shown in the preview pane.";
    case "blocked":
      return "Preview is blocked by sensitive-file or Protected Zone policy unless a permitted session reveal is enabled.";
    case "missing":
      return "The node exists in UI state but no matching file is present in the current inventory.";
    case "unsupported":
      return "Code Hangar has metadata for this node, but this file kind or read error cannot be previewed as text.";
    default:
      return "Open a file or folder to show its preview state.";
  }
}

export function protectedZoneHelp(level: string) {
  switch (level) {
    case "no_preview":
      return "blocks preview, reveal, and content indexing by default";
    case "protected":
      return "marks a high-risk path that must stay excluded from automatic content access";
    default:
      return `applies policy level ${level}`;
  }
}

function isPriorityContextStatus(status: OrphanStatus) {
  return status.reason?.toLowerCase().includes("priority context") ?? false;
}

export function orphanReferenceSummary(previewOpen: boolean, status?: OrphanStatus | null) {
  if (!previewOpen) {
    return {
      state: "No file",
      tone: "idle",
      countLabel: "not checked",
      confidenceLabel: "Local inventory only",
      reason: "Open a file to inspect references."
    };
  }

  if (!status) {
    return {
      state: "Not checked",
      tone: "idle",
      countLabel: "not checked",
      confidenceLabel: "Local inventory only",
      reason: "Run Check references to inspect this file."
    };
  }

  const priorityContext = isPriorityContextStatus(status);
  const state = status.isCandidate
    ? "No known reference"
    : priorityContext
      ? "Protected context file"
      : status.incomingReferences > 0
        ? "Known reference found"
        : status.protectedOrSensitive
          ? "Protected/sensitive file"
          : "Not an orphan candidate";

  return {
    state,
    tone: status.isCandidate ? "review" : "ready",
    countLabel: String(status.incomingReferences),
    confidenceLabel: status.confidence ? plainConfidenceLabel(status.confidence, "local signal") : "Local inventory only",
    reason: status.reason ?? "Local inventory checked."
  };
}

export function orphanReferenceStatusText(displayName: string, status: OrphanStatus) {
  if (status.isCandidate) {
    return `${displayName} has no known local reference (${plainConfidenceLabel(status.confidence, "local signal").toLowerCase()}). This is not a delete recommendation.`;
  }
  if (isPriorityContextStatus(status)) {
    return `${displayName} is protected as priority context; zero counted references does not make it an orphan candidate.`;
  }
  if (status.incomingReferences > 0) {
    return `${displayName} has ${status.incomingReferences} counted local reference${status.incomingReferences === 1 ? "" : "s"}.`;
  }
  if (status.protectedOrSensitive) {
    return `${displayName} is protected or sensitive, so Code Hangar excludes it from orphan review.`;
  }
  return `${displayName} is not classified as a reviewable orphan asset.`;
}

/**
 * One secondary fact rendered beside a PlanList row's primary name. `warn`
 * segments read as small alert pills (e.g. cross-project links); `tag` segments
 * are muted category chips (e.g. "workflow"); plain segments are muted text.
 */
export type PlanSegment = { text: string; tone?: "warn" | "tag" };

/** A structured PlanList row: one strong primary line plus muted/pill facts. */
export type PlanRow = { primary: string; segments?: PlanSegment[] };

/**
 * Turn a `DanglingAfter` dependent into a labeled-segment row while preserving
 * exact information parity with the previous `·`-joined string
 * (`"⚠ path → missingPath · in Project · workflow"`). The referrer→target path
 * stays the strong primary; cross-project becomes a ⚠ warn pill; the owning
 * project and a workflow origin become muted facts. Pure so the parity can be
 * asserted without a rendering environment.
 */
export function formatDependentRow(item: DanglingAfter): PlanRow {
  const segments: PlanSegment[] = [];
  if (item.crossProject) segments.push({ text: "⚠ cross-project", tone: "warn" });
  if (item.crossProject && item.projectName) segments.push({ text: `in ${item.projectName}` });
  if (item.dependencyKind === "workflow") segments.push({ text: "workflow", tone: "tag" });
  return { primary: `${item.path} → ${item.missingPath}`, segments };
}

/** Wrap a plain "primary" string (no secondary facts) as a PlanRow. */
function textRow(primary: string): PlanRow {
  return { primary };
}

function PlanRowItem({ row }: { row: PlanRow }) {
  const segments = row.segments ?? [];
  return (
    <>
      <strong>{row.primary}</strong>
      {segments.length ? (
        <span className="plan-row-facts">
          {segments.map((segment, index) => (
            <span
              key={`${segment.text}-${index}`}
              className={segment.tone === "warn" ? "plan-row-pill warn" : segment.tone === "tag" ? "plan-row-tag" : "plan-row-fact"}
            >
              {segment.text}
            </span>
          ))}
        </span>
      ) : null}
    </>
  );
}

export function textMentionsDependencyCache(value: string) {
  const haystack = value
    .replaceAll("\\", "/")
    .toLowerCase();
  return haystack.startsWith(".local/cargo/registry/")
    || haystack.includes(" .local/cargo/registry/")
    || haystack.includes("/.local/cargo/registry/")
    || haystack.startsWith(".cargo/registry/")
    || haystack.includes(" .cargo/registry/")
    || haystack.includes("/.cargo/registry/")
    || haystack.startsWith(".venv/")
    || haystack.includes("/.venv/")
    || haystack.startsWith("venv/")
    || haystack.includes("/venv/")
    || haystack.startsWith("site-packages/")
    || haystack.includes("/site-packages/")
    || haystack.startsWith("dist-packages/")
    || haystack.includes("/dist-packages/")
    || haystack.startsWith("node_modules/")
    || haystack.includes("/node_modules/")
    || haystack.startsWith(".pnpm/")
    || haystack.includes("/.pnpm/")
    || haystack.startsWith("vendor/")
    || haystack.includes("/vendor/")
    || haystack.startsWith("vendors/")
    || haystack.includes("/vendors/");
}

function isDependencyCacheRow(row: PlanRow) {
  return textMentionsDependencyCache([row.primary, ...(row.segments ?? []).map((segment) => segment.text)].join(" "));
}

/**
 * Split plan rows into the direct list and the rows that physically live inside a
 * dependency cache (node_modules, .cargo/registry, vendor, …). The cache rows are
 * the F7 noise win — kept OUT of the main list — but they are NOT dropped: the
 * caller renders them in a collapsed, expandable subsection so a sensitive file
 * inside a cache stays reachable (zero information loss). When nothing lives in a
 * cache, `rows` is the original list unchanged and `cacheRows` is empty.
 */
export function groupPlanRowsForDisplay(rows: PlanRow[]): { rows: PlanRow[]; cacheRows: PlanRow[] } {
  const cacheRows: PlanRow[] = [];
  const directRows: PlanRow[] = [];
  for (const row of rows) {
    if (isDependencyCacheRow(row)) cacheRows.push(row);
    else directRows.push(row);
  }
  if (!cacheRows.length) return { rows, cacheRows };
  return { rows: directRows, cacheRows };
}

/**
 * A titled, count-pilled subsection listing plan facts as labeled rows. `items`
 * may be structured `PlanRow`s or plain strings (wrapped as primary-only rows).
 * Long lists reuse the shared "Show next N" / "First N of M" pagination. When a
 * list is empty because the target is genuinely clean, pass `cleanNote` for a
 * quiet green ✓ line instead of the neutral `empty` note.
 */
export function PlanList({ title, note, empty, items, help, cleanNote }: { title: string; note: string; empty: string; items: Array<PlanRow | string>; help: string; cleanNote?: string }) {
  const visibleLimit = 6;
  const panelLimit = 20;
  const rows: PlanRow[] = items.map((item) => (typeof item === "string" ? textRow(item) : item));
  const { rows: displayRows, cacheRows } = groupPlanRowsForDisplay(rows);
  const visibleItems = displayRows.slice(0, visibleLimit);
  const hiddenItems = displayRows.slice(visibleLimit, panelLimit);
  // Cache rows never vanish: they are collapsed by default (the F7 noise win) but
  // remain fully reachable when the group is expanded. They page through the same
  // panel limit so a huge cache does not flood the expanded view.
  const cacheItems = cacheRows.slice(0, panelLimit);
  return (
    <div className="plan-subsection" data-help={help}>
      <div className="plan-subsection-head">
        <h3>{title}</h3>
        <span>{rows.length}</span>
      </div>
      <p className="plan-subsection-note">{note}</p>
      {rows.length ? (
        <>
          {displayRows.length ? (
            <ul className="plan-list">
              {visibleItems.map((row, index) => (
                <li key={`${title}-${index}`}>
                  <PlanRowItem row={row} />
                </li>
              ))}
            </ul>
          ) : null}
          {hiddenItems.length ? (
            <details className="plan-list-more">
              <summary>Show next {hiddenItems.length}</summary>
              <ul className="plan-list">
                {hiddenItems.map((row, index) => (
                  <li key={`${title}-more-${index}`}>
                    <PlanRowItem row={row} />
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          {cacheRows.length ? (
            <details className="plan-list-more plan-list-cache-group">
              <summary>Inside dependency caches ({cacheRows.length})</summary>
              <ul className="plan-list">
                {cacheItems.map((row, index) => (
                  <li key={`${title}-cache-${index}`}>
                    <PlanRowItem row={row} />
                  </li>
                ))}
              </ul>
              {cacheRows.length > panelLimit ? (
                <p className="muted result-empty">First {panelLimit} of {cacheRows.length} cache entries shown here.</p>
              ) : null}
            </details>
          ) : null}
        </>
      ) : cleanNote ? (
        <p className="plan-clean-note"><span className="plan-clean-check" aria-hidden="true">✓</span>{cleanNote}</p>
      ) : (
        <p className="muted result-empty">{empty}</p>
      )}
      {cacheRows.length ? <p className="muted result-empty">{cacheRows.length} of {rows.length} rows live inside dependency caches — grouped for readability, gate inputs unchanged.</p> : null}
      {displayRows.length > panelLimit ? <p className="muted result-empty">First {panelLimit} of {displayRows.length} readable entries available here.</p> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Light-dopamine motion primitives.
//
// Design intent: subtle, fast, consistent micro-feedback — feedback, not party.
// The pure helpers (`countUpEasing`, `computeCountUpValue`, `prefersReducedMotion`)
// are exported so they can be unit-tested in the node test environment, which
// has no DOM to render React into.
// ---------------------------------------------------------------------------

/** Duration of the count-up sweep. Kept just under the "numbers may take ~600ms" ceiling. */
export const COUNT_UP_DURATION_MS = 600;

/** Cubic ease-out — matches the CSS `--ease-out` curve's feel for numeric count-ups. */
export function countUpEasing(progress: number): number {
  const clamped = progress <= 0 ? 0 : progress >= 1 ? 1 : progress;
  return 1 - Math.pow(1 - clamped, 3);
}

/**
 * The integer a count-up should display at `elapsedMs` while animating from
 * `from` to `to` over `durationMs`. Pure and deterministic so the easing curve
 * and endpoints can be asserted without a rendering environment.
 */
export function computeCountUpValue(from: number, to: number, elapsedMs: number, durationMs: number = COUNT_UP_DURATION_MS): number {
  if (durationMs <= 0 || elapsedMs >= durationMs) return to;
  if (elapsedMs <= 0) return from;
  const eased = countUpEasing(elapsedMs / durationMs);
  return Math.round(from + (to - from) * eased);
}

/**
 * Whether motion should be suppressed for the current environment. Reads the
 * OS-level `prefers-reduced-motion` media query, guarding for the node test /
 * SSR case where `window.matchMedia` is unavailable. An explicit `override`
 * (from the in-app appearance setting) always wins when provided.
 */
export function prefersReducedMotion(override?: boolean): boolean {
  if (override != null) return override;
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

/**
 * A number that animates from a lower value up to `value` with ease-out on
 * mount and whenever the value increases (a small "the dashboard came alive"
 * moment). Decreases and reduced-motion snap instantly to the final value.
 */
export function CountUp({
  value,
  reduceMotion,
  format,
  className
}: {
  value: number;
  reduceMotion?: boolean;
  format?: (value: number) => string;
  className?: string;
}) {
  const reduce = prefersReducedMotion(reduceMotion);
  const [display, setDisplay] = useState(value);
  const fromRef = useRef(value);
  const frameRef = useRef<number | null>(null);

  useEffect(() => {
    const from = fromRef.current;
    // Snap instantly for reduced motion, equal targets, or a decrease
    // (counting down reads as an error state, not an accomplishment).
    if (reduce || from === value || value < from) {
      fromRef.current = value;
      setDisplay(value);
      return;
    }
    if (typeof window === "undefined" || typeof window.requestAnimationFrame !== "function") {
      fromRef.current = value;
      setDisplay(value);
      return;
    }
    const start = performance.now();
    const step = (now: number) => {
      const elapsed = now - start;
      setDisplay(computeCountUpValue(from, value, elapsed));
      if (elapsed < COUNT_UP_DURATION_MS) {
        frameRef.current = window.requestAnimationFrame(step);
      } else {
        fromRef.current = value;
      }
    };
    frameRef.current = window.requestAnimationFrame(step);
    return () => {
      if (frameRef.current != null) window.cancelAnimationFrame(frameRef.current);
      fromRef.current = value;
    };
  }, [value, reduce]);

  const shown = reduce ? value : display;
  return <span className={className}>{format ? format(shown) : shown.toLocaleString()}</span>;
}

/** How long the success pulse stays before auto-removing. */
export const SUCCESS_PULSE_MS = 1500;

/**
 * A small inline ✓ that mounts visible and fades out over ~1.5s to mark an
 * "action completed" moment. Auto-removes and calls `onDone`. Under reduced
 * motion it shows statically for the same duration, then disappears.
 */
export function SuccessPulse({
  label = "Done",
  reduceMotion,
  onDone
}: {
  label?: string;
  reduceMotion?: boolean;
  onDone?: () => void;
}) {
  const reduce = prefersReducedMotion(reduceMotion);
  const [visible, setVisible] = useState(true);
  const doneRef = useRef(onDone);
  doneRef.current = onDone;

  useEffect(() => {
    if (typeof window === "undefined") return;
    const timer = window.setTimeout(() => {
      setVisible(false);
      doneRef.current?.();
    }, SUCCESS_PULSE_MS);
    return () => window.clearTimeout(timer);
  }, []);

  if (!visible) return null;
  return (
    <span className="success-pulse" data-reduce-motion={reduce ? "true" : "false"} role="status" aria-live="polite">
      <span className="success-pulse-check" aria-hidden="true">✓</span>
      <span className="success-pulse-label">{label}</span>
    </span>
  );
}
