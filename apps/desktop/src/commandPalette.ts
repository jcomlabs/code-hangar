export interface ProjectScopedCommandState {
  enabled: boolean;
  contextLabel: string;
  projectHelp: string;
  reviewHelp: string;
}

export interface PaletteShortcutBlockers {
  quickOpen: boolean;
  commands: boolean;
  addProjects: boolean;
  tour: boolean;
  deepScan: boolean;
  resetAll: boolean;
  removeProject: boolean;
  rewrite: boolean;
  confirmation: boolean;
  recovery: boolean;
}

export interface PaletteScrollable {
  scrollIntoView(options?: ScrollIntoViewOptions): void;
}

export type PaletteNavigationKey = "ArrowDown" | "ArrowUp" | "Home" | "End";
export type GlobalPaletteShortcut = "quick-open" | "commands" | null;

export function globalPaletteShortcut(
  key: string,
  ctrlOrMeta: boolean
): GlobalPaletteShortcut {
  if (!ctrlOrMeta) return null;
  if (key.toLowerCase() === "p") return "quick-open";
  if (key.toLowerCase() === "k") return "commands";
  return null;
}

export function paletteShortcutsBlocked(blockers: PaletteShortcutBlockers): boolean {
  return Object.values(blockers).some(Boolean);
}

export function scrollPaletteResultIntoView(
  items: ArrayLike<PaletteScrollable | null>,
  activeIndex: number
): boolean {
  const activeItem = items[activeIndex];
  if (!activeItem) return false;
  activeItem.scrollIntoView({ block: "nearest" });
  return true;
}

export function paletteFocusIndex(
  currentIndex: number,
  itemCount: number,
  key: PaletteNavigationKey
): number {
  if (itemCount <= 0) return -1;
  if (key === "Home") return 0;
  if (key === "End") return itemCount - 1;
  if (key === "ArrowDown") return currentIndex < 0 ? 0 : (currentIndex + 1) % itemCount;
  return currentIndex < 0 ? itemCount - 1 : (currentIndex <= 0 ? itemCount - 1 : currentIndex - 1);
}

export function palettePointerMayMoveFocus(
  pointerFocusReady: boolean,
  movementX: number,
  movementY: number
): boolean {
  return pointerFocusReady && (movementX !== 0 || movementY !== 0);
}

export function projectScopedCommandState(projectName?: string | null): ProjectScopedCommandState {
  const selectedName = projectName?.trim();
  if (!selectedName) {
    return {
      enabled: false,
      contextLabel: "Project required",
      projectHelp: "Select a project before using project-scoped commands.",
      reviewHelp: "Select a project before opening Safe Manage."
    };
  }

  return {
    enabled: true,
    contextLabel: selectedName,
    projectHelp: `Return to ${selectedName}'s context.`,
    reviewHelp: `Open Safe Manage for ${selectedName}.`
  };
}
