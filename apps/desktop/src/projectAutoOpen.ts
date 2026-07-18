import type { ContextFile } from "./types";

export const INITIAL_CONTEXT_OPEN_OPTIONS = {
  allowProjectSwitch: false,
  recordRecent: false,
  replaceHistory: true,
  refreshOnly: true
} as const;

export function selectInitialContextFile(files: ContextFile[]) {
  if (files.length === 0) return null;
  const ordered = [...files].sort(compareContextFilesForAutoOpen);
  return ordered.find((file) => !file.isSensitive && !file.protectedLevel) ?? ordered[0];
}

function compareContextFilesForAutoOpen(left: ContextFile, right: ContextFile) {
  if (left.recommended !== right.recommended) return left.recommended ? -1 : 1;
  const rankDiff = left.contextRank - right.contextRank;
  if (rankDiff !== 0) return rankDiff;
  const priorityDiff = left.priority - right.priority;
  if (priorityDiff !== 0) return priorityDiff;
  return left.displayName.localeCompare(right.displayName);
}
