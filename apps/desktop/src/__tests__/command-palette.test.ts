import { describe, expect, it, vi } from "vitest";

import {
  globalPaletteShortcut,
  paletteFocusIndex,
  palettePointerMayMoveFocus,
  paletteShortcutsBlocked,
  projectScopedCommandState,
  scrollPaletteResultIntoView,
  type PaletteShortcutBlockers
} from "../commandPalette";

describe("global palette shortcuts", () => {
  it("resolves Quick Open and Commands before editable-target handling", () => {
    expect(globalPaletteShortcut("p", true)).toBe("quick-open");
    expect(globalPaletteShortcut("P", true)).toBe("quick-open");
    expect(globalPaletteShortcut("k", true)).toBe("commands");
  });

  it("does not claim shortcuts without a modifier or for unrelated keys", () => {
    expect(globalPaletteShortcut("p", false)).toBeNull();
    expect(globalPaletteShortcut("x", true)).toBeNull();
  });

  it("blocks shortcuts while any modal surface owns focus", () => {
    const unblocked: PaletteShortcutBlockers = {
      quickOpen: false,
      commands: false,
      addProjects: false,
      tour: false,
      deepScan: false,
      resetAll: false,
      removeProject: false,
      rewrite: false,
      confirmation: false,
      recovery: false
    };

    expect(paletteShortcutsBlocked(unblocked)).toBe(false);
    for (const blocker of Object.keys(unblocked) as Array<keyof PaletteShortcutBlockers>) {
      expect(paletteShortcutsBlocked({ ...unblocked, [blocker]: true })).toBe(true);
    }
  });

  it("keeps the keyboard-active result inside the visible result area", () => {
    const scrollIntoView = vi.fn();
    const items = [null, { scrollIntoView }];

    expect(scrollPaletteResultIntoView(items, 1)).toBe(true);
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" });
    expect(scrollPaletteResultIntoView(items, 0)).toBe(false);
  });
});

describe("Command Palette project-scoped commands", () => {
  it("disables project commands until a project is selected", () => {
    expect(projectScopedCommandState(null)).toMatchObject({
      enabled: false,
      contextLabel: "Project required",
      projectHelp: "Select a project before using project-scoped commands.",
      reviewHelp: "Select a project before opening Safe Manage."
    });
  });

  it("shows the selected project as the command target", () => {
    expect(projectScopedCommandState("CodeHangar")).toMatchObject({
      enabled: true,
      contextLabel: "CodeHangar",
      projectHelp: "Return to CodeHangar's context.",
      reviewHelp: "Open Safe Manage for CodeHangar."
    });
  });

  it("moves predictably through enabled commands with palette navigation keys", () => {
    expect(paletteFocusIndex(0, 6, "ArrowDown")).toBe(1);
    expect(paletteFocusIndex(0, 6, "ArrowUp")).toBe(5);
    expect(paletteFocusIndex(4, 6, "Home")).toBe(0);
    expect(paletteFocusIndex(1, 6, "End")).toBe(5);
    expect(paletteFocusIndex(-1, 0, "ArrowDown")).toBe(-1);
  });

  it("does not let opening or stationary pointer events steal keyboard focus", () => {
    expect(palettePointerMayMoveFocus(false, 12, 4)).toBe(false);
    expect(palettePointerMayMoveFocus(true, 0, 0)).toBe(false);
    expect(palettePointerMayMoveFocus(true, 2, 0)).toBe(true);
  });
});
