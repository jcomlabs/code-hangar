/**
 * Refresh an unindexed path without ever sending a provisional project id to
 * the project-scoped preview command. Cold shell-open previews use the isolated
 * local-file reader until inventory attachment assigns a positive project id.
 */
export function refreshUnindexedPreview<T>(
  projectId: number,
  path: string,
  refreshLocal: (path: string) => Promise<T>,
  refreshAttached: (projectId: number, path: string) => Promise<T>
): Promise<T> {
  return projectId > 0
    ? refreshAttached(projectId, path)
    : refreshLocal(path);
}
