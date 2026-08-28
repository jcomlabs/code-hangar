import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";

export type GuidedTourMode = "first-run" | "replay";
export type GuidedTourEdition = "local" | "connector";

export interface TourStep {
  /** CSS selector of the element to spotlight. Omit for a centered step (intro/outro). */
  selector?: string;
  title: string;
  body: string;
  /** Optional label for the final action. */
  actionLabel?: string;
  /** Side-effect to run before the step shows — e.g. select a demo project so its
   *  workspace is on screen for the next highlight. */
  before?: () => void;
}

/** The spotlight anchors the tour walks, in order. These must stay in sync with the
 *  real UI: each value is the selector queried by {@link GuidedTour}. Centered
 *  intro/outro steps have no anchor and are omitted here. */
export const TOUR_SELECTORS = {
  nav: ".primary-nav",
  projects: "[data-tour=\"tour-projects\"]",
  workspace: ".center-pane",
  safeManage: ".project-safe-manage",
  quickOpen: "[data-tour=\"tour-quick-open\"]"
} as const;

const SHARED_TOUR_STEPS: readonly TourStep[] = [
  {
    selector: TOUR_SELECTORS.nav,
    title: "Four places to remember",
    body: "Overview is your starting point. Discover finds projects, sessions and files. Recover holds previous versions and recovery history. Settings controls scan folders, protection and appearance."
  },
  {
    selector: TOUR_SELECTORS.workspace,
    title: "Review one project",
    body: "Start with What changed to see the recorded request, changed files and evidence limits. Use Context for the important documents, Files to read the source, and Sessions for the original conversations. Values changes one recognised value at a time and keeps a previous version."
  },
  {
    selector: TOUR_SELECTORS.safeManage,
    title: "Check before cleaning",
    body: "Safe Manage explains what the project owns, what is shared and what is protected. Review the evidence before any available cleanup; supported changes are confirmed and backup-first, with recovery kept in Recover."
  },
  {
    selector: TOUR_SELECTORS.quickOpen,
    title: "Find things without hunting",
    body: "Quick Open (Ctrl+P) jumps to a project or indexed file. Commands (Ctrl+K) lists available actions. Hover a control to read its short explanation in the status bar."
  }
];

export function guidedTourStorageKey(edition: GuidedTourEdition) {
  return `codehangar:tutorial-done-v2:${edition}`;
}

/** Copy differs at the points where first-run onboarding and a later replay have
 *  different promises. Replays must never imply that the visible projects are
 *  demos or that finishing will start a scan. */
export function guidedTourStepCopy(mode: GuidedTourMode, hasRealProjects = false): readonly TourStep[] {
  const replay = mode === "replay";
  return [
    {
      title: replay ? "Tour Code Hangar again" : "Welcome to Code Hangar",
      body: replay
        ? "A short refresher using your current project. Your project list, filters and preferences will be restored when the tour closes."
        : "Code Hangar keeps your AI-assisted projects and coding sessions together on this PC. The basic flow is simple: see what changed, understand the files, then make only a small reversible correction when needed."
    },
    SHARED_TOUR_STEPS[0],
    {
      selector: TOUR_SELECTORS.projects,
      title: "Choose a project here",
      body: replay
        ? "The list stays as you configured it. Search, sort, filter or pin without changing the folders on disk."
        : "Select a project to open it. Tags show which coding apps know that folder; search and filters change only this list, never the project itself."
    },
    ...SHARED_TOUR_STEPS.slice(1),
    replay
      ? {
          title: "Refresher complete",
          body: "Finish to return to the screen where you started.",
          actionLabel: "Finish tour"
        }
      : hasRealProjects
        ? {
            title: "Start with one project",
            body: "Finish returns to Overview. Open an item in Recent work or choose a project from the left, then begin with What changed.",
            actionLabel: "Finish tour"
          }
      : {
          title: "Now find your projects",
          body: "Finish opens Add projects. Deep Scan reads local project lists from supported coding tools; you can also add one folder yourself.",
          actionLabel: "Finish and find projects"
        }
  ];
}

/** First-run copy remains available for static checks and documentation. */
export const TOUR_STEP_COPY = guidedTourStepCopy("first-run");

interface SpotRect {
  top: number;
  left: number;
  width: number;
  height: number;
}

const CARD_WIDTH = 360;
const GAP = 12;

/**
 * A guided spotlight tour: dims the screen, cuts a hole around the current target
 * element, and shows a tooltip card with Back / Next / Skip. Steps with no selector
 * render a centered card (welcome / wrap-up). Targets are matched by CSS selector,
 * re-measured on every step and on resize/scroll, so the highlight follows the real
 * UI. Esc skips; ←/→/Enter navigate.
 */
export function GuidedTour({
  steps,
  mode,
  productName,
  onFinish,
  onSkip
}: {
  steps: TourStep[];
  mode: GuidedTourMode;
  productName: string;
  onFinish: () => void;
  onSkip: () => void;
}) {
  const [index, setIndex] = useState(0);
  const [rect, setRect] = useState<SpotRect | null>(null);
  const step = steps[index];
  const isLast = index === steps.length - 1;
  const nextButtonRef = useRef<HTMLButtonElement>(null);

  const goNext = useCallback(() => {
    if (isLast) onFinish();
    else setIndex((value) => Math.min(steps.length - 1, value + 1));
  }, [isLast, onFinish, steps.length]);
  const goBack = useCallback(() => setIndex((value) => Math.max(0, value - 1)), []);

  // Run the step's side-effect (e.g. select a demo) when the step changes.
  useEffect(() => {
    step?.before?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [index]);

  useEffect(() => {
    nextButtonRef.current?.focus({ preventScroll: true });
  }, [index]);

  // Measure the spotlight target after the side-effect has had a chance to render.
  useLayoutEffect(() => {
    const selector = step?.selector;
    if (!selector) {
      setRect(null);
      return;
    }
    let raf1 = 0;
    let raf2 = 0;
    const measure = () => {
      const element = document.querySelector(selector);
      if (!element) {
        setRect(null);
        return;
      }
      element.scrollIntoView({ block: "nearest", inline: "nearest" });
      const r = element.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) {
        setRect(null);
        return;
      }
      setRect({ top: r.top, left: r.left, width: r.width, height: r.height });
    };
    // Two frames: one for the `before` side-effect to commit, one to measure.
    raf1 = window.requestAnimationFrame(() => {
      raf2 = window.requestAnimationFrame(measure);
    });
    window.addEventListener("resize", measure);
    window.addEventListener("scroll", measure, true);
    return () => {
      window.cancelAnimationFrame(raf1);
      window.cancelAnimationFrame(raf2);
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
    };
  }, [index, step?.selector]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // Never hijack keys another surface already handled (e.g. a dialog's own
      // Escape) or that the user is typing into a field — Esc would otherwise
      // close that surface AND permanently skip the tour in one press.
      if (event.defaultPrevented) return;
      const target = event.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName.toLowerCase();
        if (tag === "input" || tag === "textarea" || tag === "select" || target.isContentEditable) return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        onSkip();
      } else if (event.key === "ArrowRight" || event.key === "Enter") {
        event.preventDefault();
        goNext();
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        goBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [goNext, goBack, onSkip]);

  if (!step) return null;

  const viewportHeight = typeof window === "undefined" ? 800 : window.innerHeight;
  const viewportWidth = typeof window === "undefined" ? 1280 : window.innerWidth;

  // Place the card under the target if there's room, otherwise above; clamp to the
  // viewport. Centered when there's no target.
  let cardStyle: CSSProperties;
  if (rect) {
    const below = rect.top + rect.height + GAP;
    const placeBelow = below + 200 < viewportHeight;
    const top = placeBelow ? below : Math.max(GAP, rect.top - 200 - GAP);
    const left = Math.min(Math.max(GAP, rect.left), viewportWidth - CARD_WIDTH - GAP);
    cardStyle = { position: "fixed", top, left, width: CARD_WIDTH };
  } else {
    cardStyle = {
      position: "fixed",
      top: "50%",
      left: "50%",
      transform: "translate(-50%, -50%)",
      width: 440
    };
  }

  return (
    <div className="tour-overlay" role="dialog" aria-modal="true" aria-label={`${productName} guided tour`}>
      {rect ? (
        <div
          className="tour-spotlight"
          style={{
            position: "fixed",
            top: rect.top - 6,
            left: rect.left - 6,
            width: rect.width + 12,
            height: rect.height + 12
          }}
        />
      ) : (
        <div className="tour-scrim" />
      )}
      <div className="tour-card" style={cardStyle}>
        <div className="tour-step-count">
          Step {index + 1} of {steps.length}
        </div>
        <h3>{step.title}</h3>
        <p>{step.body}</p>
        <div className="tour-actions">
          <button type="button" className="tour-skip" onClick={onSkip}>
            {mode === "replay" ? "Close tour" : "Skip tour"}
          </button>
          <div className="tour-nav">
            {index > 0 ? (
              <button type="button" className="tour-back" onClick={goBack}>
                Back
              </button>
            ) : null}
            <button ref={nextButtonRef} type="button" className="tour-next" onClick={goNext}>
              {isLast ? (step.actionLabel ?? "Finish tour") : "Next"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
