export type WslGatedDiscoveryScope = "global" | "folder";

export interface WslScanPreferencePort {
  setEnabled: (enabled: boolean) => Promise<void>;
  readEnabled: () => Promise<boolean>;
}

export class WslScanPreferenceApplyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WslScanPreferenceApplyError";
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Persist and verify the backend gate. A mismatch is a refusal, never success. */
export async function applyWslScanPreference(
  requestedEnabled: boolean,
  port: WslScanPreferencePort
): Promise<boolean> {
  try {
    await port.setEnabled(requestedEnabled);
    const appliedEnabled = await port.readEnabled();
    if (appliedEnabled !== requestedEnabled) {
      throw new Error(
        `the backend reported WSL scanning ${appliedEnabled ? "on" : "off"} after ${requestedEnabled ? "on" : "off"} was requested`
      );
    }
    return appliedEnabled;
  } catch (error) {
    throw new WslScanPreferenceApplyError(errorMessage(error));
  }
}

/**
 * The sole apply-and-start boundary for global and folder discovery. `start`
 * cannot run until the persisted value has been read back and verified.
 */
export async function runWslGatedDiscovery<T>({
  scope,
  requestedEnabled,
  port,
  start
}: {
  scope: WslGatedDiscoveryScope;
  requestedEnabled: boolean;
  port: WslScanPreferencePort;
  start: (appliedEnabled: boolean) => Promise<T>;
}): Promise<{ scope: WslGatedDiscoveryScope; appliedEnabled: boolean; result: T }> {
  const appliedEnabled = await applyWslScanPreference(requestedEnabled, port);
  const result = await start(appliedEnabled);
  return { scope, appliedEnabled, result };
}

export function isWslScanPreferenceApplyError(error: unknown): error is WslScanPreferenceApplyError {
  return error instanceof WslScanPreferenceApplyError;
}
