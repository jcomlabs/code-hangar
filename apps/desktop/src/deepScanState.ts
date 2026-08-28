import type { DeepScanPhase } from "./addProjectsDialog";

export type DeepScanOutcome =
  | "completed"
  | "partial"
  | "cancelled"
  | "failed"
  | "inventory-not-started"
  | "mapped";

export type DeepScanRecoveryAction = "retry" | "resume" | null;

export type DeepScanBuildProjectState =
  | "queued"
  | "indexing"
  | "processed"
  | "indexed"
  | "partial"
  | "stopped"
  | "failed";

export interface DeepScanTerminalPresentation {
  terminal: boolean;
  inventoryReady: boolean;
  autoDismiss: boolean;
  title: string | null;
  action: DeepScanRecoveryAction;
  actionLabel: string | null;
  readoutLabel: string | null;
}

interface ScanStatusLike {
  jobId: string;
  message?: string;
}

export type DeepScanInventoryStartAttempt<T extends ScanStatusLike> =
  | { kind: "started"; status: T }
  | { kind: "not-started"; error: string };

/**
 * Normalize the inventory-start boundary so a missing job id can never be
 * mistaken for successful completion. This is intentionally backend-agnostic
 * and therefore straightforward to exercise with rejected/null test doubles.
 */
export async function attemptDeepScanInventoryStart<T extends ScanStatusLike>(
  rootIds: number[],
  start: (rootIds: number[]) => Promise<T | null>
): Promise<DeepScanInventoryStartAttempt<T>> {
  if (rootIds.length === 0) {
    return { kind: "not-started", error: "No registered project roots were available to scan." };
  }
  try {
    const status = await start(rootIds);
    if (!status?.jobId?.trim()) {
      return { kind: "not-started", error: "The inventory service did not return a scan job." };
    }
    return { kind: "started", status };
  } catch (error) {
    return {
      kind: "not-started",
      error: error instanceof Error ? error.message : String(error)
    };
  }
}

export function deepScanOutcomeFromScanState(state: string): DeepScanOutcome | null {
  switch (state) {
    case "completed":
      return "completed";
    case "partial":
      return "partial";
    case "cancelled":
      return "cancelled";
    case "failed":
    case "unknown":
      return "failed";
    default:
      return null;
  }
}

export function deepScanTerminalPresentation(
  phase: DeepScanPhase,
  outcome: DeepScanOutcome | null | undefined
): DeepScanTerminalPresentation {
  if (phase !== "done") {
    return {
      terminal: false,
      inventoryReady: false,
      autoDismiss: false,
      title: null,
      action: null,
      actionLabel: null,
      readoutLabel: null
    };
  }
  switch (outcome) {
    case "completed":
      return {
        terminal: true,
        inventoryReady: true,
        autoDismiss: true,
        title: "Inventory ready",
        action: null,
        actionLabel: null,
        readoutLabel: "Done"
      };
    case "partial":
      return {
        terminal: true,
        inventoryReady: false,
        autoDismiss: false,
        title: "Inventory incomplete",
        action: "resume",
        actionLabel: "Resume scan",
        readoutLabel: "Partial"
      };
    case "cancelled":
      return {
        terminal: true,
        inventoryReady: false,
        autoDismiss: false,
        title: "Inventory scan stopped",
        action: "resume",
        actionLabel: "Resume scan",
        readoutLabel: "Stopped"
      };
    case "failed":
      return {
        terminal: true,
        inventoryReady: false,
        autoDismiss: false,
        title: "Deep Scan failed",
        action: "retry",
        actionLabel: "Retry",
        readoutLabel: "Failed"
      };
    case "inventory-not-started":
      return {
        terminal: true,
        inventoryReady: false,
        autoDismiss: false,
        title: "Projects added; inventory not started",
        action: "retry",
        actionLabel: "Retry inventory",
        readoutLabel: null
      };
    case "mapped":
      return {
        terminal: true,
        inventoryReady: false,
        autoDismiss: true,
        title: "Projects mapped",
        action: null,
        actionLabel: null,
        readoutLabel: null
      };
    default:
      return {
        terminal: true,
        inventoryReady: false,
        autoDismiss: false,
        title: "Deep Scan finished without a final inventory status",
        action: "retry",
        actionLabel: "Retry",
        readoutLabel: null
      };
  }
}

/** Return a measured percentage without ever promoting an incomplete scan to 100%. */
export function deepScanTerminalPercent(
  outcome: DeepScanOutcome | null | undefined,
  scannedFiles: number,
  estimatedTotalFiles: number | null | undefined
): number | null {
  if (outcome === "completed") return 100;
  if (!estimatedTotalFiles || estimatedTotalFiles <= 0) return null;
  const measured = (Math.max(0, scannedFiles) / estimatedTotalFiles) * 100;
  return Math.min(99, measured);
}

export function deepScanBuildProjectState(
  scanState: string,
  projectIndex: number,
  currentIndex: number,
  workingWithoutKnownRoot: boolean
): DeepScanBuildProjectState {
  switch (scanState) {
    case "completed":
      return "indexed";
    case "partial":
      return "partial";
    case "cancelled":
      return "stopped";
    case "failed":
    case "unknown":
      return "failed";
    default:
      if (currentIndex >= 0 && projectIndex >= 0 && projectIndex < currentIndex) return "processed";
      if (currentIndex >= 0 && projectIndex === currentIndex) return "indexing";
      if (currentIndex < 0 && workingWithoutKnownRoot) return "indexing";
      return "queued";
  }
}
