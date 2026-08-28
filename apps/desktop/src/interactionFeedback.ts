import type { MutationMoveSummary } from "./types";
import { formatBytes } from "./ui";

export function pinSuccessMessage(label: string, pinned: boolean): string {
  return pinned
    ? `${label} pinned for quick access.`
    : `${label} removed from Pinned.`;
}

export function pinFailureMessage(label: string, pinned: boolean, reason: unknown): string {
  const detail = reason instanceof Error ? reason.message : String(reason);
  return pinned
    ? `Could not pin ${label}: ${detail}`
    : `Could not unpin ${label}: ${detail}`;
}

export function scanRootToggleMessage(path: string, enabled: boolean): string {
  return enabled
    ? `${path} enabled for future scans.`
    : `${path} disabled. Existing inventory remains available.`;
}

export function scanRootToggleFailureMessage(path: string, enabled: boolean, reason: unknown): string {
  const detail = reason instanceof Error ? reason.message : String(reason);
  return `Could not ${enabled ? "enable" : "disable"} ${path}: ${detail}`;
}

export function postActionHoverHelp(pointerInitiated: boolean, underlyingHelp?: string | null): string | null {
  if (!pointerInitiated) return null;
  const value = underlyingHelp?.trim();
  return value ? value : null;
}

export function mutationMoveCompletionMessage(result: MutationMoveSummary): string {
  const removedFolders = result.removedDirs > 0
    ? ` ${result.removedDirs} eligible empty source folder${result.removedDirs === 1 ? " was" : "s were"} removed.`
    : "";
  const removedLinks = result.removedLinks > 0
    ? ` ${result.removedLinks} supported link entr${result.removedLinks === 1 ? "y was" : "ies were"} removed without following targets; blocked links remain.`
    : "";
  const sourceVolumeEffect = result.spaceRecovered > 0
    ? ` The legacy journal recorded ${formatBytes(result.spaceRecovered)} as a source-volume effect; that is not a measurement of current free space or held allocation.`
    : " The legacy journal did not record a source-volume effect; current free space and held allocation are measured separately.";
  return `Move operation ${result.operationId} finished: ${result.moved} supported original item${result.moved === 1 ? "" : "s"} moved to recovery holding, ${result.skipped} skipped, ${result.failed} failed.${removedFolders}${removedLinks} Skipped, failed or blocked objects remain at their original paths. Held allocation remains until Finish removing; the verified backup/archive is retained separately.${sourceVolumeEffect}`;
}
