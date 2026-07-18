import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { ChevronUp } from "lucide-react";

export function Sidebar({
  children,
  collapsed = false,
  onScrolledChange
}: {
  children: ReactNode;
  collapsed?: boolean;
  onScrolledChange?: (scrolled: boolean) => void;
}) {
  const ref = useRef<HTMLElement>(null);
  const [scrolled, setScrolled] = useState(false);
  useEffect(() => {
    onScrolledChange?.(scrolled);
  }, [scrolled, onScrolledChange]);
  return (
    <aside
      ref={ref}
      className={`pane left-pane ${collapsed ? "pane-collapsed" : ""}`}
      onScroll={() => setScrolled((ref.current?.scrollTop ?? 0) > 240)}
    >
      {children}
      {!collapsed && scrolled ? (
        <button
          type="button"
          className="sidebar-scroll-top"
          onClick={() => ref.current?.scrollTo({ top: 0, behavior: "smooth" })}
          aria-label="Scroll back to the top of the sidebar"
          title="Back to top"
        >
          <ChevronUp size={18} />
        </button>
      ) : null}
    </aside>
  );
}

export function ProjectWorkspace({
  children,
  className = "pane center-pane"
}: {
  children: ReactNode;
  className?: string;
}) {
  return <section className={className}>{children}</section>;
}

interface WorkspacePaneProps {
  children: ReactNode;
  collapsed?: boolean;
  scrollResetKey?: string;
}

export function ToolWorkspace({ children, collapsed = false, scrollResetKey }: WorkspacePaneProps) {
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    if (scrollResetKey == null || !ref.current) return;
    ref.current.scrollTop = 0;
    ref.current.scrollLeft = 0;
  }, [scrollResetKey]);
  return <aside ref={ref} className={`pane right-pane ${collapsed ? "pane-collapsed" : ""}`}>{children}</aside>;
}

export function InspectorPane({ children, collapsed = false }: WorkspacePaneProps) {
  return <aside className={`pane right-pane ${collapsed ? "pane-collapsed" : ""}`}>{children}</aside>;
}

export function WorkspaceGrid({
  mode,
  style,
  leftCollapsed = false,
  rightCollapsed = false,
  className = "",
  children
}: {
  mode: "project" | "tool";
  style: CSSProperties;
  leftCollapsed?: boolean;
  rightCollapsed?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return <main className={`workspace workspace-${mode} ${leftCollapsed ? "left-collapsed" : ""} ${rightCollapsed ? "right-collapsed" : ""} ${className}`} style={style}>{children}</main>;
}
