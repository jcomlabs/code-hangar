import type { SafeManageFirstRunPreference } from "./types";

export type SafeManageFirstRunChoice = "analyze_now" | "later" | "suppress";

export interface SafeManageFirstRunBackend {
  savePreference: (
    suggestAfterDiscovery: boolean,
    promptState: SafeManageFirstRunPreference["promptState"],
    markPromptedNow: boolean
  ) => Promise<SafeManageFirstRunPreference>;
  startAnalysis: () => Promise<string>;
}

export interface SafeManageFirstRunOutcome {
  preference: SafeManageFirstRunPreference;
  analysisJobId: string | null;
}

/**
 * Apply exactly one first-run choice. This is deliberately independent from
 * React so the behavior — including which choices start work and what is
 * persisted first — has an executable contract rather than copy-only tests.
 */
export async function applySafeManageFirstRunChoice(
  choice: SafeManageFirstRunChoice,
  currentPreference: SafeManageFirstRunPreference | null,
  backend: SafeManageFirstRunBackend
): Promise<SafeManageFirstRunOutcome> {
  if (choice === "suppress") {
    return {
      preference: await backend.savePreference(false, "suppressed", false),
      analysisJobId: null
    };
  }

  const preference = await backend.savePreference(
    choice === "later" ? true : (currentPreference?.suggestAfterDiscovery ?? true),
    "postponed",
    false
  );
  if (choice === "later") {
    return { preference, analysisJobId: null };
  }

  // The queued run is durably created by the backend before this resolves.
  // If starting fails, the persisted `postponed` state keeps the optional
  // prompt from looping while manual Safe Manage remains available.
  const analysisJobId = await backend.startAnalysis();
  return { preference, analysisJobId };
}
