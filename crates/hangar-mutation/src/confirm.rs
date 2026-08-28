//! Session-local, cryptographically random confirmation grants.
//!
//! A grant is short-lived, single-use, and action-bound. Irreversible batch
//! actions additionally bind the exact server-generated preview digest,
//! topology groups, and target count. Nothing is persisted and a random-source
//! failure fails closed instead of falling back to time/PID/counters.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use thiserror::Error;

const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MAX_OUTSTANDING: usize = 128;
const TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    EnterMutationMode,
    PermanentDelete,
}

/// Immutable server-preview binding for an irreversible confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationBinding {
    pub preview_id: String,
    pub preview_digest: String,
    pub topology_groups: Vec<String>,
    pub target_count: u32,
}

impl ConfirmationBinding {
    pub fn new(
        preview_id: impl Into<String>,
        preview_digest: impl Into<String>,
        topology_groups: impl IntoIterator<Item = String>,
        target_count: u32,
    ) -> Result<Self, ConfirmIssueError> {
        let preview_id = preview_id.into();
        let preview_digest = preview_digest.into();
        if preview_id.is_empty() || preview_id.len() > 128 {
            return Err(ConfirmIssueError::InvalidBinding(
                "preview id must contain 1..=128 bytes".to_string(),
            ));
        }
        if !is_v2_digest(&preview_digest) {
            return Err(ConfirmIssueError::InvalidBinding(
                "preview digest must be exactly v2: followed by 64 lowercase hex characters"
                    .to_string(),
            ));
        }
        if target_count == 0 {
            return Err(ConfirmIssueError::InvalidBinding(
                "an irreversible preview must contain at least one target".to_string(),
            ));
        }
        if target_count as usize > crate::MAX_CAPABILITIES_PER_INVOCATION {
            return Err(ConfirmIssueError::InvalidBinding(
                "confirmation target count exceeds the authenticated helper protocol bound"
                    .to_string(),
            ));
        }
        let mut topology_groups = topology_groups.into_iter().collect::<Vec<_>>();
        if topology_groups.is_empty()
            || topology_groups.len() > crate::MAX_CAPABILITIES_PER_INVOCATION
        {
            return Err(ConfirmIssueError::InvalidBinding(
                "topology groups exceed the one-elevation batch bound".to_string(),
            ));
        }
        if topology_groups
            .iter()
            .any(|group| group.is_empty() || group.len() > 256)
        {
            return Err(ConfirmIssueError::InvalidBinding(
                "topology group identifiers must contain 1..=256 bytes".to_string(),
            ));
        }
        topology_groups.sort_unstable();
        topology_groups.dedup();
        Ok(Self {
            preview_id,
            preview_digest,
            topology_groups,
            target_count,
        })
    }
}

#[derive(Debug, Error)]
pub enum ConfirmIssueError {
    #[error("confirmation random source failed: {0}")]
    Random(#[source] io::Error),
    #[error("too many outstanding confirmation grants")]
    Capacity,
    #[error("invalid confirmation binding: {0}")]
    InvalidBinding(String),
}

#[derive(Debug, Clone)]
struct PendingConfirmation {
    action: ConfirmAction,
    binding: Option<ConfirmationBinding>,
    expires_at_tick: u64,
}

type TickSource = Arc<dyn Fn() -> u64 + Send + Sync>;
type RandomSource = Arc<dyn Fn(&mut [u8]) -> io::Result<()> + Send + Sync>;

pub struct ConfirmTokenStore {
    tokens: Mutex<HashMap<String, PendingConfirmation>>,
    ttl_millis: u64,
    max_outstanding: usize,
    tick_source: TickSource,
    random_source: RandomSource,
}

impl Default for ConfirmTokenStore {
    fn default() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
            ttl_millis: DEFAULT_TTL.as_millis() as u64,
            max_outstanding: DEFAULT_MAX_OUTSTANDING,
            tick_source: Arc::new(monotonic_millis),
            random_source: Arc::new(os_random),
        }
    }
}

impl ConfirmTokenStore {
    /// Issue a short-lived action-only grant. Permanent deletion callers must
    /// use `issue_scoped`; action-only irreversible grants cannot be consumed.
    pub fn issue(&self, action: ConfirmAction) -> Result<String, ConfirmIssueError> {
        if action == ConfirmAction::PermanentDelete {
            return Err(ConfirmIssueError::InvalidBinding(
                "permanent deletion requires an exact preview-scoped grant".to_string(),
            ));
        }
        self.issue_inner(action, None)
    }

    pub fn issue_scoped(
        &self,
        action: ConfirmAction,
        binding: ConfirmationBinding,
    ) -> Result<String, ConfirmIssueError> {
        self.issue_inner(action, Some(binding))
    }

    fn issue_inner(
        &self,
        action: ConfirmAction,
        binding: Option<ConfirmationBinding>,
    ) -> Result<String, ConfirmIssueError> {
        let now = (self.tick_source)();
        let mut tokens = self.tokens.lock().expect("confirm token mutex poisoned");
        tokens.retain(|_, pending| pending.expires_at_tick > now);
        if tokens.len() >= self.max_outstanding {
            return Err(ConfirmIssueError::Capacity);
        }
        for _ in 0..4 {
            let mut random = [0u8; TOKEN_BYTES];
            (self.random_source)(&mut random).map_err(ConfirmIssueError::Random)?;
            let token = hex(&random);
            if tokens.contains_key(&token) {
                continue;
            }
            tokens.insert(
                token.clone(),
                PendingConfirmation {
                    action,
                    binding,
                    expires_at_tick: now.saturating_add(self.ttl_millis),
                },
            );
            return Ok(token);
        }
        Err(ConfirmIssueError::Random(io::Error::other(
            "OS random source repeatedly returned an outstanding token",
        )))
    }

    /// Verify and consume an action-only grant. Irreversible grants are never
    /// accepted through this compatibility method.
    pub fn consume(&self, token: &str, action: ConfirmAction) -> bool {
        if action == ConfirmAction::PermanentDelete {
            return false;
        }
        self.consume_inner(token, action, None)
    }

    pub fn consume_scoped(
        &self,
        token: &str,
        action: ConfirmAction,
        binding: &ConfirmationBinding,
    ) -> bool {
        self.consume_inner(token, action, Some(binding))
    }

    fn consume_inner(
        &self,
        token: &str,
        action: ConfirmAction,
        binding: Option<&ConfirmationBinding>,
    ) -> bool {
        let now = (self.tick_source)();
        let mut tokens = self.tokens.lock().expect("confirm token mutex poisoned");
        tokens.retain(|_, pending| pending.expires_at_tick > now);
        let matches = tokens
            .get(token)
            .is_some_and(|pending| pending.action == action && pending.binding.as_ref() == binding);
        if matches {
            tokens.remove(token);
        }
        matches
    }

    #[cfg(test)]
    fn with_sources(
        ttl_millis: u64,
        max_outstanding: usize,
        tick_source: TickSource,
        random_source: RandomSource,
    ) -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
            ttl_millis,
            max_outstanding,
            tick_source,
            random_source,
        }
    }
}

fn is_v2_digest(value: &str) -> bool {
    value.len() == 67
        && value.starts_with("v2:")
        && value[3..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn monotonic_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(windows)]
fn os_random(bytes: &mut [u8]) -> io::Result<()> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let size = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "random request is too large"))?;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            size,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "BCryptGenRandom returned NTSTATUS 0x{:08x}",
            status as u32
        )))
    }
}

pub(crate) fn secure_random_hex_256() -> Result<String, ConfirmIssueError> {
    let mut random = [0u8; TOKEN_BYTES];
    os_random(&mut random).map_err(ConfirmIssueError::Random)?;
    Ok(hex(&random))
}

#[cfg(unix)]
fn os_random(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read as _;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(not(any(windows, unix)))]
fn os_random(_bytes: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no audited OS random source is available on this platform",
    ))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("String formatting cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn binding(group: &str) -> ConfirmationBinding {
        ConfirmationBinding::new(
            "preview-1",
            format!("v2:{}", "ab".repeat(32)),
            [group.to_string()],
            2,
        )
        .unwrap()
    }

    #[test]
    fn token_is_single_use_and_action_bound() {
        let store = ConfirmTokenStore::default();
        let token = store.issue(ConfirmAction::EnterMutationMode).unwrap();
        assert!(!store.consume(&token, ConfirmAction::PermanentDelete));
        assert!(store.consume(&token, ConfirmAction::EnterMutationMode));
        assert!(!store.consume(&token, ConfirmAction::EnterMutationMode));
    }

    #[test]
    fn permanent_delete_requires_exact_preview_binding_and_is_single_use() {
        let store = ConfirmTokenStore::default();
        let expected = binding("project:7");
        let token = store
            .issue_scoped(ConfirmAction::PermanentDelete, expected.clone())
            .unwrap();
        assert!(!store.consume(&token, ConfirmAction::PermanentDelete));
        assert!(!store.consume_scoped(
            &token,
            ConfirmAction::PermanentDelete,
            &binding("project:8")
        ));
        assert!(store.consume_scoped(&token, ConfirmAction::PermanentDelete, &expected));
        assert!(!store.consume_scoped(&token, ConfirmAction::PermanentDelete, &expected));
    }

    #[test]
    fn expiry_and_capacity_cleanup_are_deterministic() {
        let tick = Arc::new(AtomicU64::new(10));
        let random_counter = Arc::new(AtomicU64::new(0));
        let store = ConfirmTokenStore::with_sources(
            5,
            1,
            {
                let tick = Arc::clone(&tick);
                Arc::new(move || tick.load(Ordering::SeqCst))
            },
            {
                let random_counter = Arc::clone(&random_counter);
                Arc::new(move |bytes| {
                    let value = random_counter.fetch_add(1, Ordering::SeqCst).to_le_bytes();
                    for (index, byte) in bytes.iter_mut().enumerate() {
                        *byte = value[index % value.len()];
                    }
                    Ok(())
                })
            },
        );
        let expired = store.issue(ConfirmAction::EnterMutationMode).unwrap();
        assert!(matches!(
            store.issue(ConfirmAction::EnterMutationMode),
            Err(ConfirmIssueError::Capacity)
        ));
        tick.store(16, Ordering::SeqCst);
        assert!(!store.consume(&expired, ConfirmAction::EnterMutationMode));
        assert!(store.issue(ConfirmAction::EnterMutationMode).is_ok());
    }

    #[test]
    fn random_source_failure_never_issues_a_predictable_fallback() {
        let store = ConfirmTokenStore::with_sources(
            100,
            1,
            Arc::new(|| 0),
            Arc::new(|_| Err(io::Error::other("synthetic RNG failure"))),
        );
        assert!(matches!(
            store.issue(ConfirmAction::EnterMutationMode),
            Err(ConfirmIssueError::Random(_))
        ));
        assert!(!store.consume("", ConfirmAction::EnterMutationMode));
    }

    #[test]
    fn binding_rejects_noncanonical_digest_and_empty_groups() {
        assert!(ConfirmationBinding::new("p", format!("v2:{}x", "ab".repeat(32)), [], 1).is_err());
        assert!(
            ConfirmationBinding::new("p", format!("v2:{}", "AB".repeat(32)), ["g".into()], 1)
                .is_err()
        );
    }
}
