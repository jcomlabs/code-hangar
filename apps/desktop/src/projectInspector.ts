import type { ProjectView } from "./workspaceRoute";

export interface InspectorContextCopy {
  sectionLabel: string;
  emptyTitle: string;
  emptyBody: string;
  subject: string;
}

export const FILE_INSPECTOR_CONTEXT: InspectorContextCopy = {
  sectionLabel: "File details",
  emptyTitle: "No file selected",
  emptyBody: "Choose a file to see useful details and local checks. Projects with loaded context open their first useful file automatically.",
  subject: "Choose a file or folder"
};

export function projectViewUsesFileInspector(view: ProjectView) {
  return view === "context" || view === "files" || view === "connections";
}

export function projectInspectorContext(
  view: ProjectView,
  projectName: string | null | undefined,
  sessionCount = 0
): InspectorContextCopy {
  if (projectViewUsesFileInspector(view)) return FILE_INSPECTOR_CONTEXT;
  if (view === "sessions") {
    const scope = projectName ? `${projectName} sessions` : "Project sessions";
    return {
      sectionLabel: "Project details",
      emptyTitle: scope,
      emptyBody: sessionCount > 0
        ? `${sessionCount} linked session${sessionCount === 1 ? "" : "s"} visible. Open a session card to inspect the transcript metadata; file checks stay in Context, Files and Connections.`
        : "No linked sessions are visible for this project yet. Use Discover to refresh local sessions or return to Context for file-level checks.",
      subject: "Project sessions"
    };
  }
  return {
    sectionLabel: "Project details",
    emptyTitle: projectName ? `${projectName} space` : "Project space",
    emptyBody: "Review footprint and scan completeness here. Open a file from Context, Files or Connections when you want file-specific checks.",
    subject: "Project space"
  };
}
