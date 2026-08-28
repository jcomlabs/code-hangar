import type { ProjectReviewCheckpoint, SessionDiscoveryCandidate } from "./types";

function sessionIdentity(session: SessionDiscoveryCandidate) {
  const source = session.sourceKind.trim().toLowerCase();
  const path = session.path.replaceAll("/", "\\").toLowerCase();
  return `${source}:${path}`;
}

function hashIdentities(identities: string[], seed: number) {
  let hash = seed >>> 0;
  for (const identity of identities) {
    for (let index = 0; index < identity.length; index += 1) {
      hash ^= identity.charCodeAt(index);
      hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    // Keep adjacent identities unambiguous without building one large string.
    hash ^= 0xff;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

/**
 * Stable, content-free identity for the current set of undated local session
 * records. This is comparison metadata only; it is not a security hash and it
 * never includes a session path in IPC or SQLite.
 */
export function undatedSessionFingerprint(sessions: SessionDiscoveryCandidate[]) {
  const identities = sessions
    .filter((session) => session.modifiedMs == null)
    .map(sessionIdentity)
    .sort();
  if (identities.length === 0) return null;
  const primary = hashIdentities(identities, 0x811c9dc5);
  const secondary = hashIdentities(identities, 0x9e3779b9);
  return `v1:${identities.length}:${primary}:${secondary}`;
}

export function undatedSessionsNeedReview(
  sessions: SessionDiscoveryCandidate[],
  checkpoint: ProjectReviewCheckpoint | null | undefined
) {
  if (!checkpoint) return sessions.some((session) => session.modifiedMs == null);
  return undatedSessionFingerprint(sessions) !== (checkpoint.undatedSessionFingerprint ?? null);
}

export function sessionsSinceReview(
  sessions: SessionDiscoveryCandidate[],
  checkpoint: ProjectReviewCheckpoint | null | undefined
) {
  if (!checkpoint) return sessions;
  const includeUndated = undatedSessionsNeedReview(sessions, checkpoint);
  return sessions.filter((session) => session.modifiedMs == null
    ? includeUndated
    : session.modifiedMs > checkpoint.sessionCutoffMs);
}
