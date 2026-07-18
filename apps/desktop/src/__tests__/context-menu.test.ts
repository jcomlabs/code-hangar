import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { ContextMenu, clampContextMenuPosition, contextMenuCoordinates, contextMenuDismissKey, contextMenuFocusIndex, fileContextCapabilities } from "../ContextMenu";

describe("context menu positioning", () => {
  it("keeps a menu opened near the bottom-right inside the viewport", () => {
    expect(clampContextMenuPosition(940, 680, 190, 250, 962, 720)).toEqual({
      left: 764,
      top: 462
    });
  });

  it("preserves an in-bounds pointer position and enforces the outer margin", () => {
    expect(clampContextMenuPosition(120, 90, 190, 250, 962, 720)).toEqual({ left: 120, top: 90 });
    expect(clampContextMenuPosition(-20, -4, 190, 250, 962, 720)).toEqual({ left: 8, top: 8 });
  });

  it("anchors keyboard-opened menus beside the focused row", () => {
    expect(contextMenuCoordinates(0, 0, { left: 240, top: 100, bottom: 132 })).toEqual({ x: 252, y: 132 });
    expect(contextMenuCoordinates(400, 220, { left: 240, top: 100, bottom: 132 })).toEqual({ x: 400, y: 220 });
  });
});

describe("context menu file capabilities", () => {
  it("keeps source and AI actions for readable code and context files", () => {
    expect(fileContextCapabilities("src/main.ts", "file")).toMatchObject({
      canViewSource: true,
      canUseAi: true,
      canOpenWithDefaultApp: true
    });
    expect(fileContextCapabilities("Dockerfile", "file").canViewSource).toBe(true);
  });

  it("does not offer accidental heavyweight launches for model binaries", () => {
    expect(fileContextCapabilities("models/huge-model.safetensors", "file")).toMatchObject({
      isHeavyModel: true,
      canViewSource: false,
      canUseAi: false,
      canOpenWithDefaultApp: false
    });
  });

  it("keeps directory menus focused on navigation and safe management", () => {
    expect(fileContextCapabilities("models", "directory")).toMatchObject({
      isDirectory: true,
      canViewSource: false,
      canOpenWithDefaultApp: false
    });
  });
});

describe("context menu dismissal", () => {
  it("recognizes Escape even when focus has left the menu", () => {
    expect(contextMenuDismissKey("Escape")).toBe(true);
    expect(contextMenuDismissKey("Enter")).toBe(false);
  });
});

describe("context menu keyboard navigation", () => {
  it("wraps through enabled items and supports boundary keys", () => {
    expect(contextMenuFocusIndex(0, 5, "ArrowDown")).toBe(1);
    expect(contextMenuFocusIndex(0, 5, "ArrowUp")).toBe(4);
    expect(contextMenuFocusIndex(3, 5, "Home")).toBe(0);
    expect(contextMenuFocusIndex(1, 5, "End")).toBe(4);
  });

  it("recovers predictably when focus is outside the menu", () => {
    expect(contextMenuFocusIndex(-1, 5, "ArrowDown")).toBe(0);
    expect(contextMenuFocusIndex(-1, 5, "ArrowUp")).toBe(4);
    expect(contextMenuFocusIndex(-1, 0, "ArrowDown")).toBe(-1);
  });
});

describe("context menu labels", () => {
  it("shows the menu title and each consecutive section only once", () => {
    const html = renderToStaticMarkup(createElement(ContextMenu, {
      menu: {
        x: 10,
        y: 10,
        label: "AI tools for selected text",
        items: [
          { id: "explain", label: "Explain", section: "Understand with AI", onSelect: () => undefined },
          { id: "review", label: "Check risks", section: "Understand with AI", onSelect: () => undefined },
          { id: "copy", label: "Copy", section: "Other", onSelect: () => undefined }
        ]
      },
      onClose: () => undefined
    }));

    expect(html).toContain("AI tools for selected text");
    expect(html.match(/Understand with AI/g)).toHaveLength(1);
    expect(html.match(/Other/g)).toHaveLength(1);
  });
});
