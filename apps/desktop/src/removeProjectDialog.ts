export interface RemoveProjectSelection {
  fromApps: boolean;
  fromHangar: boolean;
  fromDisk: boolean;
}

export function removeProjectActionLabel(selection: RemoveProjectSelection) {
  if (selection.fromDisk) return "Continue to Safe Manage";
  if (selection.fromApps && selection.fromHangar) return "Remove from apps & Code Hangar";
  if (selection.fromApps) return "Remove from AI apps";
  if (selection.fromHangar) return "Remove from Code Hangar";
  return "Choose what to remove";
}
