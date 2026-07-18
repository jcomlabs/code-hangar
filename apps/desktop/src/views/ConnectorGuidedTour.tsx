import { GuidedTour, TOUR_SELECTORS, type GuidedTourMode, type TourStep } from "./GuidedTour";

export function connectorGuidedTourStepCopy(mode: GuidedTourMode, hasRealProjects = false): readonly TourStep[] {
  const replay = mode === "replay";
  return [
    {
      title: replay ? "Tour Code Hangar AI Connector again" : "Welcome to Code Hangar AI Connector",
      body: replay
        ? "A short refresher for this edition. Your project, route and pane layout will be restored when the tour closes."
        : "This edition includes the complete local Code Hangar workflow, plus optional explanations and connections to AI coding apps. Nothing is connected and no model is used until you configure it."
    },
    {
      selector: TOUR_SELECTORS.nav,
      title: "Four places to remember",
      body: "Overview is your starting point. Discover finds projects, sessions and files. Recover holds previous versions and recovery history. Settings controls scan folders, protection, appearance and the optional Connector setup."
    },
    {
      selector: TOUR_SELECTORS.projects,
      title: "Choose a project here",
      body: "Select a project to open it. Tags show which coding apps know that folder; search and filters change only this list, never the project itself."
    },
    {
      selector: TOUR_SELECTORS.workspace,
      title: "Review locally first",
      body: "What changed, Context, Files, Sessions and Values work from local evidence. Start with What changed, inspect the source in Files, and use Values only for one small reversible correction."
    },
    {
      title: "Use a model only when it helps",
      body: "In Files, Explain translates code and What to check asks review questions. Configure a local model first, or explicitly choose your own API. Before each send, Code Hangar shows the exact destination and request without credentials; sensitive content is blocked. These tools do not write the file."
    },
    {
      selector: TOUR_SELECTORS.safeManage,
      title: "Check before cleaning",
      body: "Safe Manage explains ownership, sharing and protection before any available cleanup. Supported changes are confirmed and backup-first, with recovery kept in Recover."
    },
    {
      selector: TOUR_SELECTORS.quickOpen,
      title: "Find things without hunting",
      body: "Quick Open (Ctrl+P) jumps to a project or indexed file. Commands (Ctrl+K) lists available actions. Hover a control to read its short explanation in the status bar."
    },
    replay
      ? {
          title: "Refresher complete",
          body: "Finish to return to the screen where you started.",
          actionLabel: "Finish tour"
        }
      : hasRealProjects
        ? {
            title: "Start with one project",
            body: "Finish returns to Overview. Choose a project, begin with What changed, and open Explain only when the local evidence needs a plainer explanation.",
            actionLabel: "Finish tour"
          }
        : {
            title: "Now find your projects",
            body: "Finish opens Add projects. Deep Scan reads local project lists from supported coding tools; you can also add one folder yourself.",
            actionLabel: "Finish and find projects"
          }
  ];
}

export function ConnectorGuidedTour({
  mode,
  hasRealProjects,
  selectExample,
  onFinish,
  onSkip
}: {
  mode: GuidedTourMode;
  hasRealProjects: boolean;
  selectExample: () => void;
  onFinish: () => void;
  onSkip: () => void;
}) {
  const steps = connectorGuidedTourStepCopy(mode, hasRealProjects).map((step) =>
    step.selector === TOUR_SELECTORS.workspace || step.selector === TOUR_SELECTORS.safeManage
      ? { ...step, before: selectExample }
      : { ...step }
  );
  return (
    <GuidedTour
      steps={steps}
      mode={mode}
      productName="Code Hangar AI Connector"
      onFinish={onFinish}
      onSkip={onSkip}
    />
  );
}
