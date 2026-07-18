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
