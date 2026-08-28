import type { ScanStatus } from "./types";

export const MAX_RETAINED_TERMINAL_SCAN_STATUSES = 40;

export type ScanStatusAnnouncementKind = "new" | "state" | "phase" | "terminal" | "error" | "message";

export function scanStatusIsActive(status: ScanStatus) {
  return ["queued", "running", "cancelling"].includes(status.state);
}

function equalSnapshotValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left)
      && Array.isArray(right)
      && left.length === right.length
      && left.every((value, index) => equalSnapshotValue(value, right[index]));
  }
  if (left == null || right == null || typeof left !== "object" || typeof right !== "object") return false;

  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftRecord);
  const rightKeys = Object.keys(rightRecord);
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key) => Object.prototype.hasOwnProperty.call(rightRecord, key)
      && equalSnapshotValue(leftRecord[key], rightRecord[key]));
}

export function sameScanStatusSnapshot(left: ScanStatus, right: ScanStatus) {
  return equalSnapshotValue(left, right);
}

function sameStatusMapEntries(
  left: Readonly<Record<string, ScanStatus>>,
  right: Readonly<Record<string, ScanStatus>>
) {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key) => left[key] === right[key]);
}

/**
 * Merge one backend snapshot while keeping the resident UI store bounded.
 * Active jobs are never pruned. Terminal jobs follow the backend's 40-entry
 * retention contract and are selected by their latest `updatedAtMs` value.
 */
export function mergeScanStatusSnapshot(
  current: Readonly<Record<string, ScanStatus>>,
  incoming: ScanStatus,
  terminalLimit = MAX_RETAINED_TERMINAL_SCAN_STATUSES
): Record<string, ScanStatus> {
  const existing = current[incoming.jobId];
  const candidate = existing && sameScanStatusSnapshot(existing, incoming)
    ? current
    : { ...current, [incoming.jobId]: incoming };
  const boundedTerminalLimit = Math.max(0, Math.floor(terminalLimit));
  const terminal = Object.values(candidate)
    .filter((status) => !scanStatusIsActive(status))
    .sort((left, right) => right.updatedAtMs - left.updatedAtMs || left.jobId.localeCompare(right.jobId));

  if (terminal.length <= boundedTerminalLimit) return candidate as Record<string, ScanStatus>;

  const retainedTerminalIds = new Set(
    terminal.slice(0, boundedTerminalLimit).map((status) => status.jobId)
  );
  const bounded = Object.fromEntries(
    Object.entries(candidate).filter(([, status]) => (
      scanStatusIsActive(status) || retainedTerminalIds.has(status.jobId)
    ))
  );
  return sameStatusMapEntries(current, bounded) ? current as Record<string, ScanStatus> : bounded;
}

function semanticScanPhase(status: ScanStatus) {
  if (status.state === "cancelling") return "cancelling";
  if (["scanning", "persisting"].includes(status.scanPhase)) return "inventory";
  return status.scanPhase || status.state;
}

function phaseMessageIsSemantic(status: ScanStatus) {
  return !["estimating", "scanning", "persisting"].includes(status.scanPhase);
}

/**
 * Decide whether a polled snapshot deserves a new live-region announcement.
 * File paths, counters and batch messages remain visual-only; finalizing and
 * other low-frequency phase messages are treated as meaningful milestones.
 */
export function scanStatusAnnouncementKind(
  previous: ScanStatus | undefined,
  next: ScanStatus
): ScanStatusAnnouncementKind | null {
  if (!previous) {
    if (next.error) return "error";
    return scanStatusIsActive(next) ? "new" : "terminal";
  }
  if (next.error && next.error !== previous.error) return "error";
  if (scanStatusIsActive(previous) && !scanStatusIsActive(next)) return "terminal";
  if (previous.state !== next.state) return scanStatusIsActive(next) ? "state" : "terminal";
  if (semanticScanPhase(previous) !== semanticScanPhase(next)) return "phase";
  if (previous.message !== next.message && phaseMessageIsSemantic(next)) return "message";
  return null;
}
