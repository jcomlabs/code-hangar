export function unregisterRootConfirmationMessage(path: string | null | undefined): string {
  const target = path?.trim() || "this scan folder";
  return `Unregister ${target} from Code Hangar?\n\nThis removes only its local inventory entry. Files on disk stay untouched. You can add the folder again later.`;
}

export function unregisterProjectConfirmationMessage(name: string): string {
  const target = name.trim() || "this project";
  return `Remove ${target} from Code Hangar?\n\nThis removes only its local inventory entry. Files on disk stay untouched. You can add the project again later.`;
}
