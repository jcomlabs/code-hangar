export type VisualAcceptanceState = "default" | "empty" | "loading" | "error" | "partial" | "saturated";

const ACCEPTANCE_STATES = new Set<VisualAcceptanceState>([
  "default",
  "empty",
  "loading",
  "error",
  "partial",
  "saturated"
]);

const ERROR_COMMANDS = new Set([
  "dashboard_summary",
  "project_nav_tree",
  "project_nav_children",
  "project_git_status",
  "project_context_files",
  "file_preview",
  "node_relationships",
  "project_graph_map",
  "project_discovery_report",
  "project_discovery_deep_scan",
  "session_preview",
  "operation_plan_build",
  "risk_report_build",
  "risk_report_build_for_target",
  "roots_list",
  "recovery_pending"
]);

export function parseVisualAcceptanceState(search: string, enabled: boolean): VisualAcceptanceState {
  if (!enabled) return "default";
  const requested = new URLSearchParams(search).get("acceptanceState")?.trim().toLowerCase() ?? "default";
  return ACCEPTANCE_STATES.has(requested as VisualAcceptanceState)
    ? requested as VisualAcceptanceState
    : "default";
}

export function readVisualAcceptanceState(): VisualAcceptanceState {
  if (typeof window === "undefined") return "default";
  return parseVisualAcceptanceState(window.location.search, import.meta.env.DEV);
}

export function visualAcceptanceDelayMs(state: VisualAcceptanceState): number {
  return state === "loading" ? 2500 : 0;
}

export function shouldFailVisualAcceptanceCommand(state: VisualAcceptanceState, command: string): boolean {
  return state === "error" && ERROR_COMMANDS.has(command);
}

export function visualAcceptanceProjectCount(state: VisualAcceptanceState): number {
  return state === "saturated" ? 96 : 0;
}
