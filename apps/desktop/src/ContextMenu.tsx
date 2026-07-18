import { Fragment, useEffect, useLayoutEffect, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";

export interface ContextMenuItem {
  id: string;
  label: string;
  help?: string;
  icon?: ReactNode;
  disabled?: boolean;
  danger?: boolean;
  section?: string;
  onSelect: () => void | Promise<void>;
}

export interface ContextMenuState {
  x: number;
  y: number;
  label: string;
  items: ContextMenuItem[];
}

export interface ContextMenuAnchorBounds {
  left: number;
  top: number;
  bottom: number;
}

export function contextMenuCoordinates(
  clientX: number,
  clientY: number,
  anchor: ContextMenuAnchorBounds
) {
  // Keyboard Context Menu / Shift+F10 events commonly arrive at (0, 0). Anchor
  // those beside the focused row instead of opening a detached menu in the
  // top-left corner of the window.
  if (clientX !== 0 || clientY !== 0) return { x: clientX, y: clientY };
  return {
    x: Math.max(8, anchor.left + 12),
    y: Math.max(8, anchor.bottom || anchor.top + 24)
  };
}

const SOURCE_EXTENSIONS = new Set([
  ".bat", ".c", ".cc", ".cfg", ".cmd", ".conf", ".cpp", ".cs", ".css", ".csv",
  ".env", ".go", ".h", ".hpp", ".htm", ".html", ".ini", ".java", ".js", ".json",
  ".jsonl", ".jsx", ".log", ".lua", ".md", ".mdx", ".php", ".ps1", ".py", ".rb",
  ".rs", ".scss", ".sh", ".sql", ".svg", ".toml", ".ts", ".tsx", ".txt", ".xml",
  ".yaml", ".yml"
]);

const SOURCE_FILENAMES = new Set([
  ".editorconfig", ".eslintignore", ".eslintrc", ".gitattributes", ".gitignore", ".npmrc",
  ".prettierignore", ".prettierrc", "dockerfile", "license", "makefile", "readme"
]);

const HEAVY_MODEL_EXTENSIONS = new Set([
  ".bin", ".ckpt", ".engine", ".ggml", ".gguf", ".h5", ".model", ".onnx", ".pb",
  ".pt", ".pth", ".safetensors", ".tflite", ".weights"
]);

export function fileContextCapabilities(path: string, itemKind = "file") {
  const kind = itemKind.toLowerCase();
  const isDirectory = kind === "directory";
  const isLink = kind === "symlink" || kind === "junction" || kind === "reparse";
  const name = path.split(/[\\/]/).pop()?.toLowerCase() ?? "";
  const extensionIndex = name.lastIndexOf(".");
  const extension = extensionIndex >= 0 ? name.slice(extensionIndex) : "";
  const canViewSource = !isDirectory && !isLink
    && (SOURCE_EXTENSIONS.has(extension) || SOURCE_FILENAMES.has(name));
  const isHeavyModel = HEAVY_MODEL_EXTENSIONS.has(extension);
  return {
    isDirectory,
    isLink,
    isHeavyModel,
    canViewSource,
    canUseAi: canViewSource,
    canOpenWithDefaultApp: !isDirectory && !isLink && !isHeavyModel
  };
}

export function clampContextMenuPosition(
  x: number,
  y: number,
  menuWidth: number,
  menuHeight: number,
  viewportWidth: number,
  viewportHeight: number,
  margin = 8
) {
  const maxLeft = Math.max(margin, viewportWidth - menuWidth - margin);
  const maxTop = Math.max(margin, viewportHeight - menuHeight - margin);
  return {
    left: Math.min(Math.max(x, margin), maxLeft),
    top: Math.min(Math.max(y, margin), maxTop)
  };
}

export function contextMenuDismissKey(key: string): boolean {
  return key === "Escape";
}

export type ContextMenuNavigationKey = "ArrowDown" | "ArrowUp" | "Home" | "End";

export function contextMenuFocusIndex(
  currentIndex: number,
  itemCount: number,
  key: ContextMenuNavigationKey
): number {
  if (itemCount <= 0) return -1;
  if (key === "Home") return 0;
  if (key === "End") return itemCount - 1;
  if (key === "ArrowDown") return currentIndex < 0 ? 0 : (currentIndex + 1) % itemCount;
  return currentIndex < 0 ? itemCount - 1 : (currentIndex <= 0 ? itemCount - 1 : currentIndex - 1);
}

export function ContextMenu({ menu, onClose }: { menu: ContextMenuState; onClose: () => void }) {
  const menuRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useLayoutEffect(() => {
    const element = menuRef.current;
    if (!element) return;
    const bounds = element.getBoundingClientRect();
    const position = clampContextMenuPosition(
      menu.x,
      menu.y,
      bounds.width,
      bounds.height,
      window.innerWidth,
      window.innerHeight
    );
    element.style.left = `${position.left}px`;
    element.style.top = `${position.top}px`;
  }, [menu.items.length, menu.label, menu.x, menu.y]);

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) {
        onClose();
      }
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [onClose]);

  useEffect(() => {
    const onViewportChange = (event: Event) => {
      if (event.type === "scroll" && event.target instanceof Node && menuRef.current?.contains(event.target)) {
        return;
      }
      onClose();
    };
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("blur", onViewportChange);
    document.addEventListener("scroll", onViewportChange, true);
    return () => {
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("blur", onViewportChange);
      document.removeEventListener("scroll", onViewportChange, true);
    };
  }, [onClose]);

  useEffect(() => {
    const onWindowKeyDown = (event: globalThis.KeyboardEvent) => {
      if (!contextMenuDismissKey(event.key)) return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", onWindowKeyDown);
    return () => window.removeEventListener("keydown", onWindowKeyDown);
  }, [onClose]);

  useLayoutEffect(() => {
    previousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true });
    return () => {
      if (previousFocusRef.current?.isConnected) {
        previousFocusRef.current.focus({ preventScroll: true });
      }
    };
  }, []);

  const selectItem = (item: ContextMenuItem) => {
    if (item.disabled) return;
    onClose();
    void item.onSelect();
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (contextMenuDismissKey(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    const focusable = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    if (focusable.length === 0) return;
    const currentIndex = focusable.indexOf(document.activeElement as HTMLButtonElement);
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const nextIndex = contextMenuFocusIndex(
        currentIndex,
        focusable.length,
        event.key as ContextMenuNavigationKey
      );
      focusable[nextIndex]?.focus();
    }
  };

  return (
    <div
      ref={menuRef}
      className="context-menu"
      role="menu"
      aria-label={menu.label}
      aria-orientation="vertical"
      style={{ left: menu.x, top: menu.y }}
      onKeyDown={onKeyDown}
      onContextMenu={(event) => event.preventDefault()}
    >
      <div className="context-menu-title" role="presentation">{menu.label}</div>
      {menu.items.map((item, index) => (
        <Fragment key={item.id}>
          {item.section && item.section !== menu.items[index - 1]?.section
            ? <div className="context-menu-section" role="presentation">{item.section}</div>
            : null}
          <button
            className={item.danger ? "danger" : undefined}
            disabled={item.disabled}
            role="menuitem"
            type="button"
            data-help={item.help ?? `${item.label} for ${menu.label}.`}
            onClick={() => selectItem(item)}
          >
            <span className="context-menu-icon">{item.icon}</span>
            <span>{item.label}</span>
          </button>
        </Fragment>
      ))}
    </div>
  );
}
