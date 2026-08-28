import type { OpenTargetInspection, PreviewMode, ScanStatus, ShellOpenMode } from "./types";

const MARKDOWN_EXTENSION = /\.(?:md|markdown|mdx)$/i;

export function shellPreviewMode(path: string): PreviewMode {
  return MARKDOWN_EXTENSION.test(path) ? "rendered" : "source";
}

export function shellScanIsPending(status: Pick<ScanStatus, "state">): boolean {
  return ["queued", "running", "cancelling"].includes(status.state);
}

export function shellScanFailedToOpen(status: Pick<ScanStatus, "state">): boolean {
  return ["failed", "cancelled"].includes(status.state);
}

export function shellOpenImmediateMode(
  inspection: Pick<OpenTargetInspection, "knownProjectRoot" | "targetKind">
): ShellOpenMode | null {
  if (inspection.knownProjectRoot) return "known";
  // A file association is an explicit request to read that file. Unknown files
  // therefore open in an isolated Viewer immediately; only unknown folders need
  // the Viewer / Automatic / Manual project-root choice.
  return inspection.targetKind === "file" ? "viewer" : null;
}

/**
 * A shell-open attachment may finish long after its DB-independent preview.
 * It may update its own tab/catalog metadata, but it can only take foreground
 * ownership while it is both the newest Explorer request and the user has not
 * navigated away from the preview that request published.
 */
export function shellOpenRequestOwnsFocus(
  requestSequence: number,
  latestRequestSequence: number,
  previewSelectionSequence: number,
  currentSelectionSequence: number,
  requestIntentGeneration: number,
  currentIntentGeneration: number
) {
  return requestSequence === latestRequestSequence
    && previewSelectionSequence === currentSelectionSequence
    && requestIntentGeneration === currentIntentGeneration;
}

/** The DB-independent first read starts before a provisional Viewer exists, so
 * explicit navigation must also be detected through the selection generation. */
export function shellOpenPreviewReadOwnsFocus(
  requestSequence: number,
  latestRequestSequence: number,
  requestIntentGeneration: number,
  currentIntentGeneration: number,
  initialSelectionSequence: number,
  currentSelectionSequence: number
) {
  return requestSequence === latestRequestSequence
    && requestIntentGeneration === currentIntentGeneration
    && initialSelectionSequence === currentSelectionSequence;
}

interface ShellViewerDisposalTarget {
  rootId: number;
  scanJobId?: string | null;
  temporary: boolean;
}

interface ShellViewerDisposalOperations {
  scanStatus: (jobId: string) => Promise<Pick<ScanStatus, "state">>;
  scanCancel: (jobId: string) => Promise<unknown>;
  waitForScan: (jobId: string) => Promise<unknown>;
  discardInvestigation: (rootId: number) => Promise<unknown>;
}

function disposalErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Terminal scan jobs may be pruned in a resident process. Their missing status
 * must not strand a temporary Viewer root: discard is the authoritative,
 * fail-closed operation and the backend refuses it while a job is truly active.
 */
export async function disposeShellViewerSafely(
  target: ShellViewerDisposalTarget,
  operations: ShellViewerDisposalOperations
) {
  if (target.rootId <= 0) return;
  let scanCleanupError: unknown = null;
  if (target.scanJobId) {
    try {
      const status = await operations.scanStatus(target.scanJobId);
      if (shellScanIsPending(status)) {
        await operations.scanCancel(target.scanJobId);
        await operations.waitForScan(target.scanJobId);
      }
    } catch (error) {
      scanCleanupError = error;
    }
  }

  if (target.temporary) {
    try {
      await operations.discardInvestigation(target.rootId);
      // A successful authoritative discard proves no scan still owns the root;
      // a missing/pruned status is therefore harmless.
      return;
    } catch (discardError) {
      const scanDetail = scanCleanupError
        ? ` Scan cleanup also failed: ${disposalErrorMessage(scanCleanupError)}.`
        : "";
      throw new Error(`Temporary Viewer cleanup failed: ${disposalErrorMessage(discardError)}.${scanDetail}`);
    }
  }

  if (scanCleanupError) throw scanCleanupError;
}
