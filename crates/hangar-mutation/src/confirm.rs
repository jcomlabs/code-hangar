//! Single-use confirmation tokens.
//!
//! A mutation command requires a fresh token that the UI obtains only after
//! showing the relevant warning, so a programmatic caller cannot skip the human
//! confirmation handshake. Tokens are session-local, single-use, and bound to a
//! specific action. They are not persisted and never leave the process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Required to leave read-only mode and enable any mutation.
    EnterMutationMode,
    /// Required, additionally, to permanently delete (irreversible).
    PermanentDelete,
}

/// In-memory store of issued, not-yet-consumed confirmation tokens.
#[derive(Default)]
pub struct ConfirmTokenStore {
    tokens: Mutex<HashMap<String, ConfirmAction>>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl ConfirmTokenStore {
    /// Issue a fresh single-use token bound to `action`.
    pub fn issue(&self, action: ConfirmAction) -> String {
        let token = generate_token();
        self.tokens
            .lock()
            .expect("confirm token mutex poisoned")
            .insert(token.clone(), action);
        token
    }

    /// Verify and consume a token. Returns true only if the token was issued for
    /// exactly `action` and had not been used; the token is removed on success.
    pub fn consume(&self, token: &str, action: ConfirmAction) -> bool {
        let mut tokens = self.tokens.lock().expect("confirm token mutex poisoned");
        match tokens.get(token) {
            Some(stored) if *stored == action => {
                tokens.remove(token);
                true
            }
            _ => false,
        }
    }
}

fn generate_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&nanos.to_le_bytes());
    hasher.update(&counter.to_le_bytes());
    hasher.update(&pid.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_single_use_and_action_bound() {
        let store = ConfirmTokenStore::default();
        let token = store.issue(ConfirmAction::EnterMutationMode);

        // Wrong action is rejected even with the right token.
        assert!(!store.consume(&token, ConfirmAction::PermanentDelete));
        // Correct action succeeds once.
        assert!(store.consume(&token, ConfirmAction::EnterMutationMode));
        // The same token cannot be reused.
        assert!(!store.consume(&token, ConfirmAction::EnterMutationMode));
    }

    #[test]
    fn unknown_token_is_rejected_and_tokens_are_unique() {
        let store = ConfirmTokenStore::default();
        assert!(!store.consume("not-a-real-token", ConfirmAction::EnterMutationMode));
        let a = store.issue(ConfirmAction::PermanentDelete);
        let b = store.issue(ConfirmAction::PermanentDelete);
        assert_ne!(a, b);
    }
}
