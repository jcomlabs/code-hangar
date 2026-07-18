import type { ProjectSummary, SessionDiscoveryCandidate } from "./types";

/** A normalized AI-app identity used for badges and the app filter. */
export interface AppMeta {
  /** Canonical lowercase slug used as the filter value and CSS modifier. */
  slug: string;
  /** Short human label for the badge (e.g. "ChatGPT", "Antigravity"). */
  label: string;
}

interface KnownApp {
  slug: string;
  label: string;
  /** Lowercase substrings that map a raw source/kind string to this app. */
  match: string[];
}

// Order matters: the first entry whose `match` substring appears in the raw
// (lowercased) value wins. Keep the most specific apps before generic ones.
const KNOWN_APPS: KnownApp[] = [
  // Keep the `codex` slug as the stable on-disk/filter identity. ChatGPT is the
  // current user-facing product label for that local workspace and session store.
  { slug: "codex", label: "ChatGPT", match: ["codex", "chatgpt"] },
  { slug: "claude", label: "Claude", match: ["claude"] },
  { slug: "cursor", label: "Cursor", match: ["cursor"] },
  { slug: "antigravity", label: "Antigravity", match: ["antigravity", "gemini"] },
  { slug: "hermes", label: "Hermes", match: ["hermes", "nemoclaw"] },
  { slug: "openclaw", label: "OpenClaw", match: ["openclaw"] },
  { slug: "zed", label: "Zed", match: ["zed"] },
  { slug: "copilot", label: "Copilot", match: ["copilot"] },
  { slug: "cody", label: "Cody", match: ["cody"] },
  { slug: "continue", label: "Continue", match: ["continue"] },
  { slug: "roo", label: "Roo", match: ["roo"] },
  { slug: "cline", label: "Cline", match: ["cline"] },
  { slug: "kilo", label: "Kilo Code", match: ["kilo"] },
  { slug: "windsurf", label: "Windsurf", match: ["windsurf"] }
];

const OTHER: AppMeta = { slug: "other", label: "Other" };

/** Normalize legacy human-facing labels without changing ids, paths or stores. */
export function displayAppText(raw: string): string {
  return raw.trim().toLocaleLowerCase() === "codex" ? "ChatGPT" : raw;
}

/** Resolve a raw source/kind string to a normalized {slug,label}. */
export function appMeta(raw: string | null | undefined): AppMeta {
  if (!raw) return OTHER;
  const lower = raw.toLowerCase();
  for (const app of KNOWN_APPS) {
    if (app.match.some((needle) => lower.includes(needle))) {
      return { slug: app.slug, label: app.label };
    }
  }
  return OTHER;
}

/** The owning AI app of a project — its explicit `app`, else inferred from `source`. Used for the badge. */
export function projectAppMeta(project: Pick<ProjectSummary, "app" | "source">): AppMeta {
  return appMeta(project.app ?? project.source);
}

/** Every app a project belongs to (the primary plus any others it's also used in),
 *  deduped by slug. Falls back to the single primary when `apps` is absent. Used for
 *  the app FILTER so a Claude+ChatGPT project is found under both. */
export function projectAppMetas(project: Pick<ProjectSummary, "app" | "source" | "apps">): AppMeta[] {
  const raw = project.apps && project.apps.length > 0 ? project.apps : [project.app ?? project.source];
  const map = new Map<string, AppMeta>();
  for (const value of raw) {
    const meta = appMeta(value);
    if (!map.has(meta.slug)) map.set(meta.slug, meta);
  }
  return [...map.values()];
}

/** The originating AI app of a session — from its kind, else its source. */
export function sessionAppMeta(session: Pick<SessionDiscoveryCandidate, "sessionKind" | "sourceKind">): AppMeta {
  return appMeta(session.sessionKind || session.sourceKind);
}
