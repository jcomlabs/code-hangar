//! Connector-edition AI module: the user's own configured AI provider — a local model server on
//! loopback or an external API endpoint the user chooses — plus the single outbound chat call
//! that powers "Explain this code". It speaks two interchangeable wire formats (Chat
//! Completions–compatible `/v1/chat/completions` and Messages-API–compatible `/v1/messages`), so
//! any compatible local server or API works; no provider is privileged or hardcoded. This is the
//! ONLY crate in the workspace that reaches the network, and it is compiled solely into the AI
//! Connector edition (`agent_automation`) — the strict core lane and Local edition never link it
//! (CI-enforced).
//!
//! It is intentionally narrow: API-key storage + one HTTP call against a provider the caller has
//! already resolved (base URL + model + wire format + local/remote). It performs **no** file,
//! path, or database access — the caller (`hangar-api`) assembles the context, holds the
//! configuration, and runs the sensitive/secret send-gate first, so the secret-blocking decision
//! never lives here. The provider is never hardcoded: a `local` provider is loopback-enforced and
//! keyless; a remote provider sends the user's saved key (if any). Nothing leaves the machine
//! until the user configures a provider.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const SERVICE: &str = "code-hangar";
/// Keychain account for the configured provider's API key. Renamed from the old
/// `anthropic-api-key`; the readers fall back to the legacy account once so an existing user's
/// saved key keeps working after the rename.
const ACCOUNT: &str = "ai-provider-api-key";
const LEGACY_ANTHROPIC_ACCOUNT: &str = "anthropic-api-key";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_SOFT_CAP_TOKENS: u64 = 50_000;
const MIN_SOFT_CAP_TOKENS: u64 = 10_000;
const MAX_SOFT_CAP_TOKENS: u64 = 10_000_000;

/// The wire format the configured provider speaks. Named for the protocol, not any vendor: the
/// same format is spoken by many local servers and API providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFormat {
    /// Chat Completions–compatible `/v1/chat/completions` — the de-facto standard most local
    /// servers and API providers speak.
    ChatCompletions,
    /// Messages-API–compatible `/v1/messages`.
    MessagesApi,
}

/// Exact non-secret authentication scheme that the prepared transport will use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialUse {
    None,
    BearerSaved,
    XApiKeySaved,
}

impl ProviderFormat {
    /// Stable string tag used in the encrypted settings store + over IPC.
    pub fn as_tag(&self) -> &'static str {
        match self {
            ProviderFormat::ChatCompletions => "chat_completions",
            ProviderFormat::MessagesApi => "messages_api",
        }
    }

    /// Parse the stable tag; unknown values fall back to the universal Chat Completions format.
    /// The legacy tags (`openai_compatible` / `anthropic`) are still accepted so a provider saved
    /// by an earlier build keeps working without a migration.
    pub fn from_tag(tag: &str) -> ProviderFormat {
        match tag {
            "messages_api" | "anthropic" => ProviderFormat::MessagesApi,
            _ => ProviderFormat::ChatCompletions,
        }
    }

    /// Strict parser for any value that can authorize a provider request. Legacy tags remain
    /// accepted, but unknown/corrupt state fails closed instead of silently selecting a protocol.
    pub fn try_from_tag(tag: &str) -> Result<ProviderFormat, String> {
        match tag {
            "chat_completions" | "openai_compatible" => Ok(ProviderFormat::ChatCompletions),
            "messages_api" | "anthropic" => Ok(ProviderFormat::MessagesApi),
            other => Err(format!("Unknown AI provider format \"{other}\".")),
        }
    }
}

/// A fully-resolved provider the caller wants to talk to. `local` carries the network-relevant
/// distinction: when true the base URL host MUST be loopback and any saved key is ignored (local
/// servers are keyless); when false the request goes out over the network with the saved key (if
/// one is present).
#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub base_url: String,
    pub model: String,
    pub format: ProviderFormat,
    pub local: bool,
}

/// Exact request metadata safe to show before a send. `request_body` is the same compact JSON
/// string placed on the wire; it never includes an API key or any other header secret. Local
/// streaming may retry once without streaming, so that possible second body is disclosed too.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestDisclosure {
    pub method: String,
    pub url: String,
    pub request_body: String,
    pub fallback_request_body: Option<String>,
    pub transport: String,
}

/// An immutable provider request prepared by the backend after all local content gates pass.
///
/// This value is deliberately not serializable and carries no API key. The Connector keeps it
/// only in process memory behind a short-lived one-shot preview id. Sending consumes the value,
/// so provider configuration or file contents cannot be swapped after the user reviews the
/// literal [`RequestDisclosure`].
#[derive(Debug)]
pub struct PreparedRequest {
    config: ProviderConfig,
    operation: PreparedOperation,
    /// Hash of the remote credential state captured while preparing. No key bytes are retained.
    credential_fingerprint: Option<[u8; 32]>,
    disclosure: RequestDisclosure,
}

#[derive(Debug)]
enum PreparedOperation {
    Inference {
        system: String,
        user: String,
        max_tokens: u32,
        stream: bool,
        timeout_secs: u64,
    },
    ProviderModels {
        timeout_secs: u64,
    },
}

impl PreparedRequest {
    pub fn disclosure(&self) -> &RequestDisclosure {
        &self.disclosure
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub fn is_local(&self) -> bool {
        self.config.local
    }

    pub fn format(&self) -> ProviderFormat {
        self.config.format
    }

    /// Whether this immutable remote request was prepared with a saved credential. The fingerprint
    /// and the credential itself remain private; callers use only this boolean for origin binding.
    pub fn has_credential(&self) -> bool {
        self.credential_fingerprint.is_some()
    }

    pub fn credential_use(&self) -> CredentialUse {
        match (self.credential_fingerprint.is_some(), self.config.format) {
            (false, _) => CredentialUse::None,
            (true, ProviderFormat::ChatCompletions) => CredentialUse::BearerSaved,
            (true, ProviderFormat::MessagesApi) => CredentialUse::XApiKeySaved,
        }
    }

    pub fn credential_fingerprint(&self) -> Option<String> {
        self.credential_fingerprint.as_ref().map(fingerprint_hex)
    }
}

/// A reachable server found by the explicit loopback-only discovery action.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredLocalProvider {
    pub label: String,
    pub base_url: String,
    pub models: Vec<String>,
}

/// Process-session aggregate for every model inference performed by this crate. Values are
/// deliberately estimates, not a currency claim: provider-agnostic endpoints do not expose a
/// trustworthy price table and local models have no per-token API bill.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageStatus {
    pub session_started_unix: u64,
    pub request_count: u64,
    pub estimated_input_tokens: u64,
    pub estimated_output_tokens: u64,
    pub estimated_total_tokens: u64,
    pub soft_cap_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub over_soft_cap: bool,
    pub projected_total_tokens: u64,
    pub would_exceed_soft_cap: bool,
    pub projected_output_allowance: u64,
}

#[derive(Debug)]
struct UsageStore {
    session_started_unix: u64,
    request_count: u64,
    estimated_input_tokens: u64,
    estimated_output_tokens: u64,
    soft_cap_tokens: Option<u64>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Default for UsageStore {
    fn default() -> Self {
        Self {
            session_started_unix: unix_now(),
            request_count: 0,
            estimated_input_tokens: 0,
            estimated_output_tokens: 0,
            soft_cap_tokens: Some(DEFAULT_SOFT_CAP_TOKENS),
        }
    }
}

impl UsageStore {
    fn record(&mut self, input_tokens: u64, output_tokens: u64) {
        self.request_count = self.request_count.saturating_add(1);
        self.estimated_input_tokens = self.estimated_input_tokens.saturating_add(input_tokens);
        self.estimated_output_tokens = self.estimated_output_tokens.saturating_add(output_tokens);
    }

    fn status(
        &self,
        projected_input_tokens: u64,
        projected_output_allowance: u64,
    ) -> AiUsageStatus {
        let estimated_total_tokens = self
            .estimated_input_tokens
            .saturating_add(self.estimated_output_tokens);
        let projected_total_tokens = estimated_total_tokens
            .saturating_add(projected_input_tokens)
            .saturating_add(projected_output_allowance);
        let remaining_tokens = self
            .soft_cap_tokens
            .map(|cap| cap.saturating_sub(estimated_total_tokens));
        AiUsageStatus {
            session_started_unix: self.session_started_unix,
            request_count: self.request_count,
            estimated_input_tokens: self.estimated_input_tokens,
            estimated_output_tokens: self.estimated_output_tokens,
            estimated_total_tokens,
            soft_cap_tokens: self.soft_cap_tokens,
            remaining_tokens,
            over_soft_cap: self
                .soft_cap_tokens
                .is_some_and(|cap| estimated_total_tokens >= cap),
            projected_total_tokens,
            would_exceed_soft_cap: self
                .soft_cap_tokens
                .is_some_and(|cap| projected_total_tokens > cap),
            projected_output_allowance,
        }
    }

    fn reset(&mut self) {
        self.session_started_unix = unix_now();
        self.request_count = 0;
        self.estimated_input_tokens = 0;
        self.estimated_output_tokens = 0;
    }
}

fn usage_store() -> &'static Mutex<UsageStore> {
    static STORE: OnceLock<Mutex<UsageStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(UsageStore::default()))
}

fn estimated_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn estimated_prompt_tokens(system: &str, user: &str) -> u64 {
    // Include a small framing allowance for roles and the request envelope.
    estimated_tokens(system)
        .saturating_add(estimated_tokens(user))
        .saturating_add(8)
}

fn record_usage(input_tokens: u64, output: &str) {
    usage_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record(input_tokens, estimated_tokens(output));
}

/// Current process-session usage plus an optional next-operation projection. The projection does
/// not reserve or block anything: the cap is intentionally soft and the UI remains usable.
pub fn usage_status(projected_input_tokens: u64, projected_output_allowance: u64) -> AiUsageStatus {
    usage_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .status(projected_input_tokens, projected_output_allowance)
}

/// Change the warning threshold for this process session. `None` means no warning threshold.
pub fn usage_set_soft_cap(soft_cap_tokens: Option<u64>) -> Result<AiUsageStatus, String> {
    if let Some(cap) = soft_cap_tokens {
        if !(MIN_SOFT_CAP_TOKENS..=MAX_SOFT_CAP_TOKENS).contains(&cap) {
            return Err(format!(
                "The AI session soft cap must be between {MIN_SOFT_CAP_TOKENS} and {MAX_SOFT_CAP_TOKENS} tokens."
            ));
        }
    }
    let mut store = usage_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store.soft_cap_tokens = soft_cap_tokens;
    Ok(store.status(0, 0))
}

/// Clear aggregate estimates while keeping the chosen warning threshold.
pub fn usage_reset() -> AiUsageStatus {
    let mut store = usage_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store.reset();
    store.status(0, 0)
}

#[cfg(windows)]
fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|err| format!("Keychain error: {err}"))
}

#[cfg(not(windows))]
fn entry(_account: &str) -> Result<keyring::Entry, String> {
    Err(
        "AI provider keys are supported only through Windows secure credential storage."
            .to_string(),
    )
}

/// Save the user's provider API key to the OS keychain. Rejects an obviously-wrong value.
pub fn key_set(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.len() < 12 {
        return Err("That does not look like an API key.".to_string());
    }
    entry(ACCOUNT)?
        .set_password(key)
        .map_err(|err| format!("Could not save the key: {err}"))
}

/// Whether a key is currently saved (never returns the key itself). Checks the current account
/// and the legacy account so a pre-rename key still counts.
pub fn key_status() -> bool {
    saved_key().map(|key| !key.is_empty()).unwrap_or(false)
}

/// Remove the saved key from both the current and legacy accounts. Succeeds even if none was set.
pub fn key_clear() -> Result<(), String> {
    let mut last_err = None;
    for account in [ACCOUNT, LEGACY_ANTHROPIC_ACCOUNT] {
        let result = entry(account).and_then(|entry| match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(format!("Could not remove the key: {err}")),
        });
        if let Err(err) = result {
            last_err = Some(err);
        }
    }
    match last_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Read the saved key from the current account, falling back once to the legacy account. Returns
/// None when no non-empty key is stored anywhere.
fn saved_key() -> Option<String> {
    for account in [ACCOUNT, LEGACY_ANTHROPIC_ACCOUNT] {
        if let Ok(entry) = entry(account) {
            if let Ok(key) = entry.get_password() {
                if !key.is_empty() {
                    return Some(key);
                }
            }
        }
    }
    None
}

/// Resolve the key for a request: always `None` for a local provider (loopback servers are
/// keyless, and we must never leak a saved cloud key to one), otherwise the saved key if present.
fn resolve_key(config: &ProviderConfig) -> Option<String> {
    if config.local {
        return None;
    }
    saved_key()
}

fn credential_fingerprint(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}

fn fingerprint_hex(fingerprint: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in fingerprint {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

/// One-way identifier used to bind Credential Manager state to a provider origin in encrypted
/// settings. It is never a credential and cannot be used for authentication.
pub fn key_material_fingerprint(key: &str) -> String {
    fingerprint_hex(&credential_fingerprint(key.trim()))
}

/// Fingerprint of the currently stored credential, without returning or exposing the key itself.
pub fn current_key_fingerprint() -> Option<String> {
    saved_key().map(|key| key_material_fingerprint(&key))
}

fn prepared_credential_fingerprint(config: &ProviderConfig) -> Option<[u8; 32]> {
    resolve_key(config).as_deref().map(credential_fingerprint)
}

/// Resolve the credential once for the actual send and prove it is the same credential state
/// captured during preparation. This closes the gap where a key could be added, removed or
/// replaced after review; the caller must obtain a fresh disclosure instead.
fn resolve_prepared_key(request: &PreparedRequest) -> Result<Option<String>, String> {
    let key = resolve_key(&request.config);
    let current = key.as_deref().map(credential_fingerprint);
    if current != request.credential_fingerprint {
        return Err(
            "The saved provider credential changed after this request was reviewed. Review the exact request again; nothing was sent."
                .to_string(),
        );
    }
    Ok(key)
}

/// Accept a URL ONLY when it resolves to loopback. Parses with the SAME `url` crate that reqwest
/// uses, so the host we check is byte-for-byte the host reqwest will dial — closing the
/// parser-divergence bypass class (e.g. WHATWG treats `\` as `/` for http(s), so
/// `http://evil.com\@localhost/v1` actually connects to `evil.com`, not `localhost`). Requires an
/// http(s) scheme; accepts the whole 127.0.0.0/8 + ::1 loopback ranges and the `localhost`
/// domain (with an optional trailing dot); rejects everything else (`0.0.0.0`, LAN IPs,
/// `127.0.0.1.evil.com`, userinfo tricks, …).
fn is_loopback_url(url_str: &str) -> Result<(), String> {
    let parsed =
        url::Url::parse(url_str).map_err(|_| "The endpoint URL is not valid.".to_string())?;
    validate_endpoint_components(&parsed)?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "The endpoint must be an http(s) URL, not \"{other}://\"."
            ))
        }
    }
    let loopback = match parsed.host() {
        Some(url::Host::Domain(domain)) => domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if loopback {
        Ok(())
    } else {
        Err(
            "A local model endpoint must be on this machine (127.0.0.1, localhost, or ::1)."
                .to_string(),
        )
    }
}

fn validate_endpoint_components(parsed: &url::Url) -> Result<(), String> {
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(
            "The provider endpoint must not contain embedded user information.".to_string(),
        );
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(
            "The provider endpoint must not contain a query string or fragment.".to_string(),
        );
    }
    Ok(())
}

/// Public persist-time guard: a `local` provider's endpoint must be loopback. Called by the
/// caller before storing a config so a non-loopback local URL can never be saved (loopback is
/// also re-checked at send time against the exact request URL inside the adapters below).
pub fn validate_local_endpoint(base_url: &str) -> Result<(), String> {
    is_loopback_url(base_url)
}

/// Public persist-time guard for a REMOTE (`api`) provider endpoint: require `https`, or allow
/// plain `http` ONLY when the host is loopback (a local gateway/proxy such as LiteLLM on
/// 127.0.0.1). Anything else would later get the saved Bearer/x-api-key attached to a cleartext
/// connection. Reuses `is_loopback_url` — the same `url`-crate parse reqwest dials with — so the
/// loopback decision can never diverge from what actually gets connected to (host parsing is
/// never hand-rolled here; see the backslash-bypass note on `is_loopback_url`).
pub fn validate_remote_endpoint(base_url: &str) -> Result<(), String> {
    let parsed =
        url::Url::parse(base_url).map_err(|_| "The endpoint URL is not valid.".to_string())?;
    validate_endpoint_components(&parsed)?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => is_loopback_url(base_url).map_err(|_| {
            "An API endpoint must use https:// — plain http would send your API key in cleartext. \
             (http is only allowed for a gateway on this machine: 127.0.0.1, localhost, or ::1.)"
                .to_string()
        }),
        other => Err(format!(
            "The endpoint must be an http(s) URL, not \"{other}://\"."
        )),
    }
}

/// The security-relevant origin of an endpoint — `scheme://host[:port]`, lowercased — using the
/// SAME `url` crate reqwest dials with (never a hand-rolled split, which the backslash-bypass note
/// on `is_loopback_url` shows is unsafe). Returned so a caller can tell whether two endpoints point
/// at the *same remote host*: a change here means the saved API key would travel to a DIFFERENT
/// origin and must be dropped. `None` when the URL does not parse or carries no host.
pub fn endpoint_origin(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str.trim()).ok()?;
    validate_endpoint_components(&parsed).ok()?;
    let host = parsed.host_str()?;
    // `url::Url::port_or_known_default` fills the scheme's default (443/80) so `https://h` and
    // `https://h:443` compare equal — the same TLS destination.
    let port = parsed.port_or_known_default();
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Canonical credential owner for an API-mode endpoint. Remote localhost gateways are pinned to
/// numeric IPv4 by the transport, so the durable binding uses that same effective origin rather
/// than the pre-rewrite spelling.
pub fn remote_credential_origin(base_url: &str) -> Result<String, String> {
    validate_remote_endpoint(base_url)?;
    let effective = prefer_ipv4_loopback(base_url);
    endpoint_origin(&effective)
        .ok_or_else(|| "The API provider endpoint has no valid credential origin.".to_string())
}

/// For a local provider, prefer the IPv4 loopback: rewrite a `localhost` host to `127.0.0.1`.
/// On Windows `localhost` usually resolves to `::1` (IPv6) first, but local model servers (Ollama,
/// LM Studio, vLLM) bind `127.0.0.1` (IPv4) by default, and reqwest connects to `::1` without
/// falling back — so a `localhost` URL fails with "error sending request". `127.0.0.1` is still
/// loopback (passes the guard) and is what those servers actually listen on. A user who really
/// wants IPv6 can type `[::1]` explicitly (left untouched).
fn prefer_ipv4_loopback(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut url) => {
            let is_localhost = matches!(
                url.host(),
                Some(url::Host::Domain(d)) if d.trim_end_matches('.').eq_ignore_ascii_case("localhost")
            );
            if is_localhost && url.set_host(Some("127.0.0.1")).is_ok() {
                url.to_string()
            } else {
                url_str.to_string()
            }
        }
        Err(_) => url_str.to_string(),
    }
}

/// Compute the exact URL to dial. For a local provider, prefer IPv4 loopback and enforce loopback
/// on the FINAL url (so neither a path-join nor the localhost→127.0.0.1 rewrite can smuggle a
/// non-loopback host past the guard). Remote URLs are also revalidated here, immediately before
/// transport construction, so legacy/corrupt caller state cannot downgrade an API key to HTTP.
fn finalize_url(url: String, local: bool) -> Result<String, String> {
    if local {
        let url = prefer_ipv4_loopback(&url);
        is_loopback_url(&url)?;
        Ok(url)
    } else {
        // Remote mode may deliberately point at a local gateway. If that gateway was named
        // `localhost`, pin the effective destination to numeric loopback before attaching the
        // saved API key so hosts-file/DNS state cannot turn the cleartext exception into egress.
        let url = prefer_ipv4_loopback(&url);
        validate_remote_endpoint(&url)?;
        Ok(url)
    }
}

/// Append path segments through the same URL parser reqwest uses. Query, fragment and userinfo are
/// rejected before this point, so the returned string is the literal unambiguous wire target.
fn append_url_path(base: &str, suffix: &str) -> Result<String, String> {
    let mut parsed =
        url::Url::parse(base).map_err(|_| "The endpoint URL is not valid.".to_string())?;
    validate_endpoint_components(&parsed)?;
    let base_path = parsed.path().trim_end_matches('/');
    let suffix = suffix.trim_matches('/');
    let path = if base_path.is_empty() || base_path == "/" {
        format!("/{suffix}")
    } else {
        format!("{base_path}/{suffix}")
    };
    parsed.set_path(&path);
    Ok(parsed.to_string())
}

/// Anthropic request URL: the base omits `/v1` by convention, but tolerate a base that already
/// ends in `/v1` so both `https://api.anthropic.com` and `…/v1` resolve correctly.
fn anthropic_url(base: &str) -> Result<String, String> {
    let mut parsed =
        url::Url::parse(base).map_err(|_| "The endpoint URL is not valid.".to_string())?;
    validate_endpoint_components(&parsed)?;
    let base_path = parsed.path().trim_end_matches('/');
    let base_path = base_path.strip_suffix("/v1").unwrap_or(base_path);
    let path = if base_path.is_empty() || base_path == "/" {
        "/v1/messages".to_string()
    } else {
        format!("{base_path}/v1/messages")
    };
    parsed.set_path(&path);
    Ok(parsed.to_string())
}

fn http_client(timeout_secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        // The disclosed URL is the complete network authorization. Following even a temporary
        // redirect could replay the body (and, for remote providers, credentials) to an
        // undisclosed host, including an escape from a loopback endpoint.
        .redirect(reqwest::redirect::Policy::none())
        // CRITICAL: never honour a system/env proxy. reqwest's default reads HTTP_PROXY/
        // HTTPS_PROXY/ALL_PROXY and has NO automatic loopback exclusion, so without this a
        // `local` (loopback) request — and, in api mode, the Authorization/x-api-key header —
        // would be routed to an external proxy host, breaking the loopback-only invariant and
        // leaking the key. We only ever talk to the single user-named endpoint, so a proxy is
        // never wanted.
        .no_proxy()
        .build()
        .map_err(|err| format!("Could not create the HTTP client: {err}"))
}

// --- OpenAI-compatible wire types ------------------------------------------------------------

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: Option<ChatChoiceMessage>,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: Option<String>,
}

// --- Anthropic wire types --------------------------------------------------------------------

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<Message<'a>>,
    stream: bool,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

// --- Shared error envelope (OpenAI and Anthropic both use `{ "error": { "message": … } }`) ----

#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

fn api_error(http_code: u16, body: &str) -> String {
    if let Ok(envelope) = serde_json::from_str::<ApiErrorEnvelope>(body) {
        return format!("Provider API error: {}", envelope.error.message);
    }
    format!("Provider API error (HTTP {http_code}).")
}

fn chat_request_body(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    stream: bool,
) -> Result<String, String> {
    // Direct OpenAI GPT-5.6 calls use the current Chat Completions contract. Keep the legacy
    // `max_tokens` field for every other compatible endpoint: local servers and provider
    // gateways often intentionally implement that older, broadly supported shape.
    let direct_openai_gpt56 = uses_direct_openai_gpt56_contract(config);
    serde_json::to_string(&ChatRequest {
        model: &config.model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: system,
            },
            ChatMessage {
                role: "user",
                content: user,
            },
        ],
        max_tokens: (!direct_openai_gpt56).then_some(max_tokens),
        max_completion_tokens: direct_openai_gpt56.then_some(max_tokens),
        stream,
    })
    .map_err(|error| format!("Could not serialize the provider request: {error}"))
}

fn uses_direct_openai_gpt56_contract(config: &ProviderConfig) -> bool {
    if config.local
        || config.format != ProviderFormat::ChatCompletions
        || endpoint_origin(&config.base_url).as_deref() != Some("https://api.openai.com:443")
    {
        return false;
    }

    let model = config.model.trim();
    model == "gpt-5.6" || model.starts_with("gpt-5.6-")
}

fn messages_request_body(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    stream: bool,
) -> Result<String, String> {
    serde_json::to_string(&MessagesRequest {
        model: &config.model,
        max_tokens,
        system,
        messages: vec![Message {
            role: "user",
            content: user,
        }],
        stream,
    })
    .map_err(|error| format!("Could not serialize the provider request: {error}"))
}

fn request_url(config: &ProviderConfig) -> Result<String, String> {
    match config.format {
        ProviderFormat::ChatCompletions => finalize_url(
            append_url_path(&config.base_url, "chat/completions")?,
            config.local,
        ),
        ProviderFormat::MessagesApi => finalize_url(anthropic_url(&config.base_url)?, config.local),
    }
}

fn request_body(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    stream: bool,
) -> Result<String, String> {
    match config.format {
        ProviderFormat::ChatCompletions => {
            chat_request_body(config, system, user, max_tokens, stream)
        }
        ProviderFormat::MessagesApi => {
            messages_request_body(config, system, user, max_tokens, stream)
        }
    }
}

fn read_response_text(mut response: reqwest::blocking::Response) -> Result<String, String> {
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read the API response: {error}"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("The provider response exceeded the 2 MiB safety limit.".to_string());
    }
    String::from_utf8(bytes)
        .map_err(|_| "The provider response was not valid UTF-8 text.".to_string())
}

fn parse_openai_response(body: &str) -> Result<String, String> {
    let parsed: ChatResponse =
        serde_json::from_str(body).map_err(|err| format!("Unexpected API response: {err}"))?;
    let text = parsed
        .choices
        .into_iter()
        .filter_map(|choice| choice.message)
        .filter_map(|message| message.content)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return Err("The model returned an empty response.".to_string());
    }
    Ok(text)
}

fn parse_anthropic_response(body: &str) -> Result<String, String> {
    let parsed: MessagesResponse =
        serde_json::from_str(body).map_err(|err| format!("Unexpected API response: {err}"))?;
    let text = parsed
        .content
        .into_iter()
        .filter_map(|block| block.text)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return Err("The model returned an empty response.".to_string());
    }
    Ok(text)
}

fn call_openai(
    client: &reqwest::blocking::Client,
    config: &ProviderConfig,
    key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let url = request_url(config)?;
    let body = chat_request_body(config, system, user, max_tokens, false)?;
    let mut request = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body);
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let response = request
        .send()
        .map_err(|err| format!("Network error reaching the provider: {err}"))?;
    let status = response.status();
    let text = read_response_text(response)?;
    if !status.is_success() {
        return Err(api_error(status.as_u16(), &text));
    }
    parse_openai_response(&text)
}

fn call_anthropic(
    client: &reqwest::blocking::Client,
    config: &ProviderConfig,
    key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let url = request_url(config)?;
    let body = messages_request_body(config, system, user, max_tokens, false)?;
    let mut request = client
        .post(&url)
        .header("anthropic-version", ANTHROPIC_API_VERSION)
        .header("content-type", "application/json")
        .body(body);
    if let Some(key) = key {
        request = request.header("x-api-key", key);
    }
    let response = request
        .send()
        .map_err(|err| format!("Network error reaching the provider: {err}"))?;
    let status = response.status();
    let text = read_response_text(response)?;
    if !status.is_success() {
        return Err(api_error(status.as_u16(), &text));
    }
    parse_anthropic_response(&text)
}

fn execute_nonstream(
    client: &reqwest::blocking::Client,
    config: &ProviderConfig,
    key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    match config.format {
        ProviderFormat::ChatCompletions => {
            call_openai(client, config, key, system, user, max_tokens)
        }
        ProviderFormat::MessagesApi => {
            call_anthropic(client, config, key, system, user, max_tokens)
        }
    }
}

#[derive(Debug)]
enum StreamAttemptError {
    Retry(String),
    Fatal(String),
}

fn stream_unsupported_status(status: u16) -> bool {
    matches!(status, 400 | 404 | 405 | 406 | 415 | 422 | 501)
}

fn streamed_delta(value: &serde_json::Value) -> Option<&str> {
    value
        .pointer("/choices/0/delta/content")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .pointer("/delta/text")
                .and_then(serde_json::Value::as_str)
        })
}

fn complete_response_from_value(
    value: &serde_json::Value,
    format: ProviderFormat,
) -> Result<String, String> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| format!("Unexpected API response: {error}"))?;
    match format {
        ProviderFormat::ChatCompletions => parse_openai_response(&encoded),
        ProviderFormat::MessagesApi => parse_anthropic_response(&encoded),
    }
}

fn call_streaming<F>(
    client: &reqwest::blocking::Client,
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    on_delta: &mut F,
) -> Result<String, StreamAttemptError>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let url = request_url(config).map_err(StreamAttemptError::Fatal)?;
    let body =
        request_body(config, system, user, max_tokens, true).map_err(StreamAttemptError::Fatal)?;
    let mut request = client
        .post(&url)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(body);
    if matches!(config.format, ProviderFormat::MessagesApi) {
        request = request.header("anthropic-version", ANTHROPIC_API_VERSION);
    }
    let response = request.send().map_err(|error| {
        StreamAttemptError::Fatal(format!("Network error reaching the provider: {error}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        let text = read_response_text(response).map_err(StreamAttemptError::Fatal)?;
        let error = api_error(status.as_u16(), &text);
        return if stream_unsupported_status(status.as_u16()) {
            Err(StreamAttemptError::Retry(error))
        } else {
            Err(StreamAttemptError::Fatal(error))
        };
    }

    // Cap the underlying reader, not merely the accumulated line count. `read_line` otherwise
    // allocates until a newline before we get a chance to inspect its size.
    let mut reader = BufReader::new(response.take((MAX_RESPONSE_BYTES + 1) as u64));
    let mut line = String::new();
    let mut total_bytes = 0_usize;
    let mut output = String::new();
    let mut complete_candidates = Vec::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|error| {
            StreamAttemptError::Fatal(format!("Could not read the provider stream: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(read);
        if total_bytes > MAX_RESPONSE_BYTES {
            return Err(StreamAttemptError::Fatal(
                "The provider response exceeded the 2 MiB safety limit.".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
            continue;
        }
        let payload = trimmed
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or(trimmed);
        if payload == "[DONE]" {
            break;
        }
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(delta) = streamed_delta(&value) {
            if !delta.is_empty() {
                on_delta(delta).map_err(StreamAttemptError::Fatal)?;
                output.push_str(delta);
            }
        } else {
            complete_candidates.push(value);
        }
    }
    if !output.is_empty() {
        return Ok(output);
    }
    for candidate in complete_candidates {
        if let Ok(text) = complete_response_from_value(&candidate, config.format) {
            on_delta(&text).map_err(StreamAttemptError::Fatal)?;
            return Ok(text);
        }
    }
    Err(StreamAttemptError::Retry(
        "The local model did not return a readable stream.".to_string(),
    ))
}

/// Build the exact URL and compact JSON body that a subsequent explanation send will use. The
/// caller has already gated `user`. Authentication headers are deliberately absent: credentials
/// never cross this crate's keychain boundary or appear in the disclosure.
pub fn request_disclosure(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<RequestDisclosure, String> {
    request_disclosure_with_transport(config, system, user, max_tokens, config.local)
}

/// Build the exact request used by a complete-response call. Project summaries use
/// `explain`, even for a local provider, so their disclosure must not claim that the
/// request streams or show a fallback body that will never be used.
pub fn complete_request_disclosure(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<RequestDisclosure, String> {
    request_disclosure_with_transport(config, system, user, max_tokens, false)
}

fn request_disclosure_with_transport(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    stream: bool,
) -> Result<RequestDisclosure, String> {
    Ok(RequestDisclosure {
        method: "POST".to_string(),
        url: request_url(config)?,
        request_body: request_body(config, system, user, max_tokens, stream)?,
        fallback_request_body: stream
            .then(|| request_body(config, system, user, max_tokens, false))
            .transpose()?,
        transport: if stream {
            "Local streaming; one non-streaming retry only if this server cannot stream."
                .to_string()
        } else {
            "Complete response; no automatic retry.".to_string()
        },
    })
}

/// Freeze the exact destination, compact JSON body and optional local-stream fallback that a
/// later one-shot send will use. Authentication is intentionally resolved only inside the send;
/// neither this value nor its disclosure contains key material.
pub fn prepare_request(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    stream: bool,
) -> Result<PreparedRequest, String> {
    let disclosure = request_disclosure_with_transport(config, system, user, max_tokens, stream)?;
    Ok(PreparedRequest {
        config: config.clone(),
        operation: PreparedOperation::Inference {
            system: system.to_string(),
            user: user.to_string(),
            max_tokens,
            stream,
            timeout_secs: 120,
        },
        credential_fingerprint: prepared_credential_fingerprint(config),
        disclosure,
    })
}

/// Freeze the fixed, content-free reachability probe behind the same one-shot review contract as
/// every other provider request. The prompt and short timeout are backend-owned rather than IPC
/// inputs, so confirmation can authorize only the exact disclosed ping.
pub fn prepare_provider_test(config: &ProviderConfig) -> Result<PreparedRequest, String> {
    const SYSTEM: &str = "Reply with the single word OK.";
    const USER: &str = "ping";
    let disclosure = request_disclosure_with_transport(config, SYSTEM, USER, 16, false)?;
    Ok(PreparedRequest {
        config: config.clone(),
        operation: PreparedOperation::Inference {
            system: SYSTEM.to_string(),
            user: USER.to_string(),
            max_tokens: 16,
            stream: false,
            timeout_secs: 20,
        },
        credential_fingerprint: prepared_credential_fingerprint(config),
        disclosure,
    })
}

fn provider_models_disclosure(config: &ProviderConfig) -> Result<RequestDisclosure, String> {
    if !matches!(config.format, ProviderFormat::ChatCompletions) {
        return Err(
            "This provider format does not expose an OpenAI-compatible model-list endpoint."
                .to_string(),
        );
    }
    Ok(RequestDisclosure {
        method: "GET".to_string(),
        url: finalize_url(append_url_path(&config.base_url, "models")?, config.local)?,
        request_body: String::new(),
        fallback_request_body: None,
        transport: "Model-list lookup; no automatic retry.".to_string(),
    })
}

/// Freeze the exact OpenAI-compatible `GET /models` destination. This is deliberately separate
/// from loopback discovery: discovery retains its fixed numeric allowlist and never creates an
/// IPC capability for an arbitrary draft endpoint.
pub fn prepare_provider_models(config: &ProviderConfig) -> Result<PreparedRequest, String> {
    let disclosure = provider_models_disclosure(config)?;
    Ok(PreparedRequest {
        config: config.clone(),
        operation: PreparedOperation::ProviderModels { timeout_secs: 20 },
        credential_fingerprint: prepared_credential_fingerprint(config),
        disclosure,
    })
}

fn verify_prepared_request(request: &PreparedRequest) -> Result<(), String> {
    let rebuilt = match &request.operation {
        PreparedOperation::Inference {
            system,
            user,
            max_tokens,
            stream,
            ..
        } => {
            request_disclosure_with_transport(&request.config, system, user, *max_tokens, *stream)?
        }
        PreparedOperation::ProviderModels { .. } => provider_models_disclosure(&request.config)?,
    };
    if rebuilt != request.disclosure {
        return Err(
            "The prepared provider request no longer matches its reviewed disclosure. Nothing was sent."
                .to_string(),
        );
    }
    Ok(())
}

/// Consume an immutable complete-response request. Callers cannot construct `PreparedRequest`
/// directly, so every content-bearing provider call must first pass through `prepare_request`.
pub fn send_prepared(request: PreparedRequest) -> Result<String, String> {
    verify_prepared_request(&request)?;
    let key = resolve_prepared_key(&request)?;
    let PreparedRequest {
        config, operation, ..
    } = request;
    match operation {
        PreparedOperation::Inference {
            system,
            user,
            max_tokens,
            stream: false,
            timeout_secs,
        } => explain_with_timeout_and_key(
            &config,
            &system,
            &user,
            max_tokens,
            timeout_secs,
            key.as_deref(),
        ),
        PreparedOperation::Inference { stream: true, .. } => Err(
            "This prepared request requires the bounded streaming sender. Nothing was sent."
                .to_string(),
        ),
        PreparedOperation::ProviderModels { .. } => Err(
            "This prepared request requires the model-list sender. Nothing was sent.".to_string(),
        ),
    }
}

/// Consume an immutable streaming request. Only loopback providers actually stream; remote
/// providers retain their single-response behaviour and emit one bounded delta.
pub fn send_prepared_stream<F>(request: PreparedRequest, on_delta: F) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    verify_prepared_request(&request)?;
    let key = resolve_prepared_key(&request)?;
    let PreparedRequest {
        config, operation, ..
    } = request;
    match operation {
        PreparedOperation::Inference {
            system,
            user,
            max_tokens,
            stream: true,
            timeout_secs,
        } => explain_stream_with_timeout_and_key(
            &config,
            &system,
            &user,
            max_tokens,
            timeout_secs,
            key.as_deref(),
            on_delta,
        ),
        PreparedOperation::Inference { stream: false, .. } => Err(
            "This prepared request was reviewed as a complete response. Nothing was sent."
                .to_string(),
        ),
        PreparedOperation::ProviderModels { .. } => Err(
            "This prepared request requires the model-list sender. Nothing was sent.".to_string(),
        ),
    }
}

/// Consume only a reviewed model-list request. Failures stay visible to the caller so an
/// authentication error, timeout or malformed response cannot masquerade as a valid empty list.
pub fn send_prepared_provider_models(request: PreparedRequest) -> Result<Vec<String>, String> {
    verify_prepared_request(&request)?;
    let key = resolve_prepared_key(&request)?;
    let PreparedRequest {
        config, operation, ..
    } = request;
    match operation {
        PreparedOperation::ProviderModels { timeout_secs } => {
            provider_models_probe_with_key(&config, timeout_secs, key.as_deref())
        }
        PreparedOperation::Inference { .. } => {
            Err("This prepared request is not a model-list lookup. Nothing was sent.".to_string())
        }
    }
}

/// Send one explanation request to the configured provider and return the text. Blocking — call
/// it from a blocking context (the app runs it on a blocking worker). The caller has already
/// gated the `user` content; this function never reads files itself. Loopback is enforced for
/// local providers against the exact request URL inside the adapters.
#[cfg(test)]
fn explain(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    explain_with_timeout(config, system, user, max_tokens, 120)
}

#[cfg(test)]
fn explain_with_timeout(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout_secs: u64,
) -> Result<String, String> {
    let key = resolve_key(config);
    explain_with_timeout_and_key(
        config,
        system,
        user,
        max_tokens,
        timeout_secs,
        key.as_deref(),
    )
}

fn explain_with_timeout_and_key(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout_secs: u64,
    key: Option<&str>,
) -> Result<String, String> {
    let input_tokens = estimated_prompt_tokens(system, user);
    let result = (|| {
        let client = http_client(timeout_secs)?;
        execute_nonstream(&client, config, key, system, user, max_tokens)
    })();
    record_usage(input_tokens, result.as_deref().unwrap_or(""));
    result
}

/// Stream a local provider response in bounded text deltas. External APIs deliberately keep the
/// existing single-request behavior to avoid a compatibility retry that could double a bill. A
/// local server that rejects or ignores SSE is retried once with the disclosed non-streaming body.
#[cfg(test)]
fn explain_stream<F>(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    explain_stream_with_timeout(config, system, user, max_tokens, 120, on_delta)
}

#[cfg(test)]
fn explain_stream_with_timeout<F>(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout_secs: u64,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let key = resolve_key(config);
    explain_stream_with_timeout_and_key(
        config,
        system,
        user,
        max_tokens,
        timeout_secs,
        key.as_deref(),
        on_delta,
    )
}

fn explain_stream_with_timeout_and_key<F>(
    config: &ProviderConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout_secs: u64,
    key: Option<&str>,
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let input_tokens = estimated_prompt_tokens(system, user);
    let mut observed_output = String::new();
    let result = (|| {
        let client = http_client(timeout_secs)?;
        if !config.local {
            let text = execute_nonstream(&client, config, key, system, user, max_tokens)?;
            observed_output.push_str(&text);
            on_delta(&text)?;
            return Ok(text);
        }

        let mut observed_delta = |delta: &str| {
            observed_output.push_str(delta);
            on_delta(delta)
        };
        match call_streaming(
            &client,
            config,
            system,
            user,
            max_tokens,
            &mut observed_delta,
        ) {
            Ok(text) => Ok(text),
            Err(StreamAttemptError::Retry(_reason)) => {
                let text = execute_nonstream(&client, config, key, system, user, max_tokens)?;
                observed_output.push_str(&text);
                on_delta(&text)?;
                Ok(text)
            }
            Err(StreamAttemptError::Fatal(error)) => Err(error),
        }
    })();
    record_usage(input_tokens, &observed_output);
    result
}

/// Test-only convenience wrapper. Production model-list calls must consume the immutable request
/// staged by `prepare_provider_models` after the user reviews it.
#[cfg(test)]
fn provider_models(config: &ProviderConfig) -> Result<Vec<String>, String> {
    if !matches!(config.format, ProviderFormat::ChatCompletions) {
        return Err(
            "This provider format does not expose an OpenAI-compatible model-list endpoint."
                .to_string(),
        );
    }
    send_prepared_provider_models(prepare_provider_models(config)?)
}

fn provider_models_probe(
    config: &ProviderConfig,
    timeout_secs: u64,
) -> Result<Vec<String>, String> {
    let key = resolve_key(config);
    provider_models_probe_with_key(config, timeout_secs, key.as_deref())
}

fn provider_models_probe_with_key(
    config: &ProviderConfig,
    timeout_secs: u64,
    key: Option<&str>,
) -> Result<Vec<String>, String> {
    let url = finalize_url(append_url_path(&config.base_url, "models")?, config.local)?;
    let client = http_client(timeout_secs)?;
    let mut request = client.get(&url);
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let response = request
        .send()
        .map_err(|error| model_list_transport_error(error.is_timeout(), &error.to_string()))?;
    if !response.status().is_success() {
        return Err(model_list_http_error(response.status().as_u16()));
    }
    let text = read_response_text(response)?;
    parse_model_list_response(&text)
}

fn model_list_transport_error(timed_out: bool, detail: &str) -> String {
    if timed_out {
        "The model-list request timed out before the endpoint responded.".to_string()
    } else {
        format!("Could not reach the model-list endpoint: {detail}")
    }
}

fn model_list_http_error(status: u16) -> String {
    match status {
        401 => "The model-list endpoint returned HTTP 401 Unauthorized. Check the saved API credential for this provider.".to_string(),
        403 => "The model-list endpoint returned HTTP 403 Forbidden. The saved API credential may not have permission to list models.".to_string(),
        _ => format!("The model-list endpoint returned HTTP {status}."),
    }
}

fn parse_model_list_response(text: &str) -> Result<Vec<String>, String> {
    let parsed: ModelsResponse = serde_json::from_str(text).map_err(|_| {
        "The model-list endpoint returned invalid or incompatible JSON.".to_string()
    })?;
    // Dedupe preserving first-seen order so the UI datalist has no duplicate keys.
    let mut seen = std::collections::HashSet::new();
    let models: Vec<String> = parsed
        .data
        .into_iter()
        .filter_map(|entry| entry.id)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .filter(|id| seen.insert(id.clone()))
        .collect();
    if models.is_empty() {
        return Err("The model-list endpoint returned no usable model identifiers.".to_string());
    }
    Ok(models)
}

/// Explicit, loopback-only discovery of common local OpenAI-compatible server ports. It is never
/// called automatically. Every candidate uses a numeric 127.0.0.1 URL, no DNS/proxy, a short
/// timeout and the same final-URL loopback guard as a real provider request.
pub fn discover_local_providers() -> Vec<DiscoveredLocalProvider> {
    [
        ("Ollama-compatible", "http://127.0.0.1:11434/v1"),
        ("LM Studio-compatible", "http://127.0.0.1:1234/v1"),
        ("vLLM-compatible", "http://127.0.0.1:8000/v1"),
        ("Local OpenAI-compatible", "http://127.0.0.1:8080/v1"),
    ]
    .into_iter()
    .filter_map(|(label, base_url)| {
        let config = ProviderConfig {
            base_url: base_url.to_string(),
            model: String::new(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        provider_models_probe(&config, 2)
            .ok()
            .map(|models| DiscoveredLocalProvider {
                label: label.to_string(),
                base_url: base_url.to_string(),
                models,
            })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn local(base: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: base.to_string(),
            model: "m".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        }
    }

    #[cfg(windows)]
    fn non_loopback_test_ip() -> std::net::Ipv4Addr {
        use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
        use windows_sys::Win32::NetworkManagement::IpHelper::{GetIpAddrTable, MIB_IPADDRTABLE};

        let mut size = 0_u32;
        let probe = unsafe { GetIpAddrTable(std::ptr::null_mut(), &mut size, 0) };
        assert_eq!(probe, ERROR_INSUFFICIENT_BUFFER);
        let mut buffer = vec![0_u32; size.div_ceil(4) as usize];
        let table = buffer.as_mut_ptr().cast::<MIB_IPADDRTABLE>();
        let result = unsafe { GetIpAddrTable(table, &mut size, 0) };
        assert_eq!(result, NO_ERROR);
        let count = unsafe { (*table).dwNumEntries as usize };
        let rows = unsafe { (*table).table.as_ptr() };
        (0..count)
            .map(|index| unsafe { (*rows.add(index)).dwAddr })
            .map(|address| std::net::Ipv4Addr::from(address.to_ne_bytes()))
            .find(|address| !address.is_loopback() && !address.is_unspecified())
            .expect("a non-loopback IPv4 adapter is required for the redirect regression")
    }

    #[cfg(windows)]
    fn assert_local_redirect_is_not_followed(status: &str, streaming: bool) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::thread;
        use std::time::{Duration, Instant};

        // Enumerate an assigned adapter address without opening a route-probe socket. Addressing
        // the target listener through that interface gives the redirect a genuinely non-loopback
        // destination while keeping the regression test entirely on this machine.
        let target_ip = non_loopback_test_ip();

        let target = TcpListener::bind((target_ip, 0)).expect("bind redirect target");
        let target_port = target.local_addr().unwrap().port();
        target.set_nonblocking(true).unwrap();
        let target_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(750);
            loop {
                match target.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 4096];
                        let bytes = stream.read(&mut request).unwrap_or(0);
                        let payload = r#"{"choices":[{"message":{"content":"escaped"}}]}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            payload.len(),
                            payload
                        );
                        let _ = stream.write_all(response.as_bytes());
                        return Some(String::from_utf8_lossy(&request[..bytes]).into_owned());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return None;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("redirect target accept failed: {error}"),
                }
            }
        });

        let redirector = TcpListener::bind("127.0.0.1:0").expect("bind redirector");
        let redirector_port = redirector.local_addr().unwrap().port();
        let location = format!("http://{target_ip}:{target_port}/escaped");
        assert!(is_loopback_url(&location).is_err());
        let expected_code = status.split_once(' ').unwrap().0.to_string();
        let status = status.to_string();
        let redirect_server = thread::spawn(move || {
            let (stream, _) = redirector.accept().expect("accept disclosed request");
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0_usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0_u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            request_line
        });

        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{redirector_port}/v1"),
            model: "redirect-test".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let error = if streaming {
            explain_stream(&config, "sys", "usr", 64, |_| Ok(())).unwrap_err()
        } else {
            explain(&config, "sys", "usr", 64).unwrap_err()
        };

        assert!(error.contains(&format!("HTTP {expected_code}")), "{error}");
        assert!(
            redirect_server
                .join()
                .unwrap()
                .starts_with("POST /v1/chat/completions"),
            "the disclosed loopback endpoint did not receive the request"
        );
        assert_eq!(
            target_server.join().unwrap(),
            None,
            "the request escaped to the undisclosed non-loopback redirect target"
        );
    }

    #[test]
    #[cfg(windows)]
    fn nonstream_request_does_not_follow_307_off_loopback() {
        assert_local_redirect_is_not_followed("307 Temporary Redirect", false);
    }

    #[test]
    #[cfg(windows)]
    fn streaming_request_does_not_follow_308_off_loopback() {
        assert_local_redirect_is_not_followed("308 Permanent Redirect", true);
    }

    #[test]
    fn usage_store_projects_a_soft_cap_without_blocking() {
        let mut store = UsageStore {
            session_started_unix: 123,
            request_count: 0,
            estimated_input_tokens: 0,
            estimated_output_tokens: 0,
            soft_cap_tokens: Some(10_000),
        };
        store.record(7_500, 400);
        let status = store.status(500, 2_048);
        assert_eq!(status.estimated_total_tokens, 7_900);
        assert_eq!(status.remaining_tokens, Some(2_100));
        assert!(!status.over_soft_cap);
        assert!(status.would_exceed_soft_cap);
        assert_eq!(status.projected_total_tokens, 10_448);
        // The cap is advisory: recording remains possible beyond it.
        store.record(2_500, 0);
        assert!(store.status(0, 0).over_soft_cap);
    }

    #[test]
    fn prompt_usage_estimate_includes_framing_and_is_never_currency() {
        assert_eq!(estimated_prompt_tokens("1234", "1234"), 10);
        let status = AiUsageStatus {
            session_started_unix: 1,
            request_count: 1,
            estimated_input_tokens: 10,
            estimated_output_tokens: 2,
            estimated_total_tokens: 12,
            soft_cap_tokens: None,
            remaining_tokens: None,
            over_soft_cap: false,
            projected_total_tokens: 12,
            would_exceed_soft_cap: false,
            projected_output_allowance: 0,
        };
        let encoded = serde_json::to_value(status).unwrap();
        assert_eq!(encoded["estimatedTotalTokens"], 12);
        assert!(encoded.get("estimatedCost").is_none());
    }

    #[test]
    fn loopback_accepts_localhost_variants() {
        for url in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:1234/v1",
            "http://[::1]:8000/v1",
            "http://localhost",
            "http://LocalHost:11434/v1",
            "http://localhost.:11434/v1",
            "http://127.0.0.2:8000", // anywhere in 127.0.0.0/8 is loopback
            "http://localhost\\evil.com/v1", // backslash => path, host stays localhost (matches reqwest)
        ] {
            assert!(is_loopback_url(url).is_ok(), "should accept {url}");
        }
    }

    #[test]
    fn loopback_rejects_non_loopback() {
        for url in [
            "http://0.0.0.0:8000/v1",
            "http://192.168.1.5:11434/v1",
            "https://127.0.0.1.evil.com/v1",
            "https://api.openai.com/v1",
            "http://localhost.evil.com/v1",
            "https://openrouter.ai/api/v1",
            "not a url",
            "ftp://127.0.0.1/v1",                 // non-http(s) scheme
            "http://user:pass@127.0.0.1:8000/v1", // ambiguous embedded credentials forbidden
            // Backslash-as-slash bypass attempts: WHATWG (and reqwest) connect to the host BEFORE
            // the backslash, so these must be rejected — `is_loopback_url` uses the same parser.
            "http://evil.com\\@localhost/v1",
            "http://evil.com:1234\\@localhost/v1",
            "https://attacker.example\\@127.0.0.1/v1",
        ] {
            assert!(is_loopback_url(url).is_err(), "should reject {url}");
        }
    }

    #[test]
    fn endpoint_origin_identifies_the_remote_host() {
        // The default port is filled in, so a bare host and its explicit default port
        // are the SAME origin (identical TLS destination) — not a provider switch.
        assert_eq!(
            endpoint_origin("https://api.openai.com/v1"),
            Some("https://api.openai.com:443".to_string())
        );
        assert_eq!(
            endpoint_origin("https://api.openai.com:443/v1"),
            endpoint_origin("https://api.openai.com/v1")
        );
        // Host casing is normalized; the path never affects the origin.
        assert_eq!(
            endpoint_origin("https://API.OpenAI.com/v1/chat"),
            endpoint_origin("https://api.openai.com/v1")
        );
        // A different host, port, or scheme is a different origin.
        assert_ne!(
            endpoint_origin("https://api.openai.com/v1"),
            endpoint_origin("https://openrouter.ai/api/v1")
        );
        assert_ne!(
            endpoint_origin("https://h.example/v1"),
            endpoint_origin("https://h.example:8443/v1")
        );
        // Unparseable / host-less inputs yield None (the caller treats that as "changed").
        assert_eq!(endpoint_origin("not a url"), None);
        assert_eq!(endpoint_origin(""), None);
    }

    #[test]
    fn local_provider_ignores_saved_key() {
        // resolve_key never touches the keychain for a local provider.
        assert!(resolve_key(&local("http://localhost:11434/v1")).is_none());
    }

    #[test]
    fn remote_endpoint_requires_https_or_loopback() {
        // https anywhere is fine — the key travels encrypted.
        assert!(validate_remote_endpoint("https://api.openai.com/v1").is_ok());
        assert!(validate_remote_endpoint("https://openrouter.ai/api/v1").is_ok());
        // Plain http is allowed only to this machine (a local gateway/proxy).
        assert!(validate_remote_endpoint("http://127.0.0.1:4000/v1").is_ok());
        assert!(validate_remote_endpoint("http://localhost:4000/v1").is_ok());
        assert!(validate_remote_endpoint("http://[::1]:4000/v1").is_ok());
        // http to a remote host would send the saved key in cleartext — rejected.
        for url in [
            "http://api.example.com/v1",
            "http://192.168.1.5:4000/v1",
            "http://localhost.evil.com/v1",
            // Same parser-divergence class the loopback guard closes: the WHATWG
            // parse connects to evil.com, so http must be rejected here too.
            "http://evil.com\\@localhost/v1",
            "ftp://api.example.com/v1",
            "not a url",
        ] {
            assert!(
                validate_remote_endpoint(url).is_err(),
                "should reject {url}"
            );
        }
    }

    #[test]
    fn final_request_url_revalidates_remote_transport_and_pins_localhost() {
        let cleartext_remote = ProviderConfig {
            base_url: "http://api.example.com/v1".to_string(),
            model: "model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: false,
        };
        let error = request_url(&cleartext_remote).expect_err("remote HTTP must fail closed");
        assert!(
            error.contains("must use https://"),
            "unexpected error: {error}"
        );

        let local_gateway = ProviderConfig {
            base_url: "http://localhost:4000/v1".to_string(),
            model: "model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: false,
        };
        assert_eq!(
            request_url(&local_gateway).unwrap(),
            "http://127.0.0.1:4000/v1/chat/completions"
        );
        assert_eq!(
            remote_credential_origin("http://localhost:4000/v1").unwrap(),
            "http://127.0.0.1:4000"
        );

        let secure_remote = ProviderConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: false,
        };
        assert_eq!(
            request_url(&secure_remote).unwrap(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn openai_request_body_shape() {
        let body = ChatRequest {
            model: "qwen2.5-coder",
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: "sys",
                },
                ChatMessage {
                    role: "user",
                    content: "usr",
                },
            ],
            max_tokens: Some(1200),
            max_completion_tokens: None,
            stream: false,
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["model"], "qwen2.5-coder");
        assert_eq!(value["max_tokens"], 1200);
        assert_eq!(value["stream"], false);
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][0]["content"], "sys");
        assert_eq!(value["messages"][1]["role"], "user");
        assert_eq!(value["messages"][1]["content"], "usr");
    }

    #[test]
    fn direct_openai_gpt56_uses_the_current_chat_completions_contract() {
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let config = ProviderConfig {
                base_url: "https://api.openai.com/v1".to_string(),
                model: model.to_string(),
                format: ProviderFormat::ChatCompletions,
                local: false,
            };
            let body = chat_request_body(&config, "system", "user", 1200, false).unwrap();
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();

            assert_eq!(value["model"], model);
            assert_eq!(value["max_completion_tokens"], 1200);
            assert!(value.get("max_tokens").is_none());
            assert_eq!(value["messages"][0]["role"], "system");
            assert_eq!(value["messages"][0]["content"], "system");
            assert_eq!(value["messages"][1]["role"], "user");
            assert_eq!(value["stream"], false);
        }
    }

    #[test]
    fn gpt56_disclosure_contains_the_exact_modern_token_field() {
        let config = ProviderConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5.6".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: false,
        };
        let disclosure = request_disclosure(&config, "system", "user", 987).unwrap();
        let body: serde_json::Value = serde_json::from_str(&disclosure.request_body).unwrap();

        assert_eq!(disclosure.url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(body["max_completion_tokens"], 987);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(disclosure.fallback_request_body.is_none());
    }

    #[test]
    fn gpt56_contract_is_not_assumed_for_other_compatible_endpoints() {
        for (base_url, local) in [
            ("http://127.0.0.1:8080/v1", true),
            ("https://openrouter.ai/api/v1", false),
        ] {
            let config = ProviderConfig {
                base_url: base_url.to_string(),
                model: "gpt-5.6".to_string(),
                format: ProviderFormat::ChatCompletions,
                local,
            };
            let body = chat_request_body(&config, "system", "user", 321, false).unwrap();
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();

            assert_eq!(value["max_tokens"], 321);
            assert!(value.get("max_completion_tokens").is_none());
            assert_eq!(value["messages"][0]["role"], "system");
        }
    }

    #[test]
    fn anthropic_request_body_shape() {
        let body = MessagesRequest {
            model: "claude",
            max_tokens: 1200,
            system: "sys",
            messages: vec![Message {
                role: "user",
                content: "usr",
            }],
            stream: false,
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["model"], "claude");
        assert_eq!(value["system"], "sys");
        assert_eq!(value["stream"], false);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"], "usr");
    }

    #[test]
    fn openai_response_parsing() {
        let body =
            json!({"choices":[{"message":{"role":"assistant","content":"hello"}}]}).to_string();
        assert_eq!(parse_openai_response(&body).unwrap(), "hello");
        let empty = json!({"choices":[]}).to_string();
        assert!(parse_openai_response(&empty).is_err());
        let blank = json!({"choices":[{"message":{"content":""}}]}).to_string();
        assert!(parse_openai_response(&blank).is_err());
    }

    #[test]
    fn anthropic_response_parsing() {
        let body = json!({"content":[{"type":"text","text":"hi"}]}).to_string();
        assert_eq!(parse_anthropic_response(&body).unwrap(), "hi");
        let empty = json!({"content":[]}).to_string();
        assert!(parse_anthropic_response(&empty).is_err());
    }

    #[test]
    fn api_error_prefers_envelope_message() {
        let body = json!({"error":{"message":"bad key"}}).to_string();
        assert_eq!(api_error(401, &body), "Provider API error: bad key");
        assert_eq!(
            api_error(500, "internal server error"),
            "Provider API error (HTTP 500)."
        );
    }

    #[test]
    fn url_helpers() {
        assert_eq!(
            append_url_path("http://localhost:11434/v1", "chat/completions").unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            append_url_path("http://localhost:11434/v1/", "/chat/completions").unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            anthropic_url("https://api.anthropic.com").unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_url("https://api.anthropic.com/v1/").unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn endpoint_bases_reject_ambiguous_url_components_before_disclosure() {
        for endpoint in [
            "https://user@example.com/v1",
            "https://example.com/v1?route=other",
            "https://example.com/v1#not-the-wire-path",
        ] {
            assert!(validate_remote_endpoint(endpoint).is_err(), "{endpoint}");
        }
        for endpoint in [
            "http://user@127.0.0.1:11434/v1",
            "http://127.0.0.1:11434/v1?route=other",
            "http://127.0.0.1:11434/v1#not-the-wire-path",
        ] {
            assert!(validate_local_endpoint(endpoint).is_err(), "{endpoint}");
        }

        for format in [ProviderFormat::ChatCompletions, ProviderFormat::MessagesApi] {
            let config = ProviderConfig {
                base_url: "https://example.com/v1#ambiguous".to_string(),
                model: "model".to_string(),
                format,
                local: false,
            };
            assert!(prepare_provider_test(&config).is_err());
        }
        let config = ProviderConfig {
            base_url: "https://example.com/v1?ambiguous=1".to_string(),
            model: "model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: false,
        };
        assert!(prepare_provider_models(&config).is_err());
    }

    #[test]
    fn format_tag_roundtrip() {
        assert_eq!(ProviderFormat::ChatCompletions.as_tag(), "chat_completions");
        assert_eq!(ProviderFormat::MessagesApi.as_tag(), "messages_api");
        assert_eq!(
            ProviderFormat::from_tag("messages_api"),
            ProviderFormat::MessagesApi
        );
        assert_eq!(
            ProviderFormat::from_tag("chat_completions"),
            ProviderFormat::ChatCompletions
        );
        // Legacy tags saved by an earlier build still resolve to the right wire format.
        assert_eq!(
            ProviderFormat::from_tag("anthropic"),
            ProviderFormat::MessagesApi
        );
        assert_eq!(
            ProviderFormat::from_tag("openai_compatible"),
            ProviderFormat::ChatCompletions
        );
        assert_eq!(
            ProviderFormat::from_tag("garbage"),
            ProviderFormat::ChatCompletions
        );

        // Authorization paths use the strict parser: old known tags remain compatible, while
        // unknown/corrupt state cannot silently select a wire protocol.
        assert_eq!(
            ProviderFormat::try_from_tag("openai_compatible").unwrap(),
            ProviderFormat::ChatCompletions
        );
        assert_eq!(
            ProviderFormat::try_from_tag("anthropic").unwrap(),
            ProviderFormat::MessagesApi
        );
        assert!(ProviderFormat::try_from_tag("garbage").is_err());
        assert!(ProviderFormat::try_from_tag("").is_err());
    }

    // A self-contained end-to-end proof: stand up a one-shot HTTP mock on loopback, point a
    // `local` OpenAI-compatible provider at it, and assert the full round-trip — client build,
    // loopback acceptance, URL join, request serialization, real TCP send, response parse. No
    // external server or key required, so it runs in CI.
    #[test]
    fn openai_round_trip_over_loopback() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();

        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let body_str = String::from_utf8_lossy(&body).into_owned();
            let payload =
                r#"{"choices":[{"message":{"role":"assistant","content":"round trip ok"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            (request_line, body_str)
        });

        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "test-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let text = explain(&config, "sys", "usr", 64).expect("explain round-trip");
        assert_eq!(text, "round trip ok");

        let (request_line, body_str) = server.join().unwrap();
        assert!(
            request_line.starts_with("POST /v1/chat/completions"),
            "request line was: {request_line}"
        );
        assert!(
            body_str.contains("\"model\":\"test-model\""),
            "body was: {body_str}"
        );
        assert!(body_str.contains("\"role\":\"system\""));
        assert!(body_str.contains("\"role\":\"user\""));
    }

    #[test]
    fn disclosure_is_the_exact_body_and_includes_local_fallback() {
        let config = ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "local-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let disclosure = request_disclosure(&config, "system text", "user text", 321).unwrap();
        assert_eq!(disclosure.url, "http://127.0.0.1:11434/v1/chat/completions");
        let primary: serde_json::Value = serde_json::from_str(&disclosure.request_body).unwrap();
        let fallback: serde_json::Value =
            serde_json::from_str(disclosure.fallback_request_body.as_deref().unwrap()).unwrap();
        assert_eq!(primary["stream"], true);
        assert_eq!(fallback["stream"], false);
        assert_eq!(primary["messages"][0]["content"], "system text");
        assert_eq!(primary["messages"][1]["content"], "user text");
        assert_eq!(primary["max_tokens"], 321);
        assert!(!disclosure
            .request_body
            .to_ascii_lowercase()
            .contains("authorization"));
    }

    #[test]
    fn complete_disclosure_matches_non_streaming_local_calls() {
        let config = ProviderConfig {
            base_url: "http://127.0.0.1:8080/v1".to_string(),
            model: "summary-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let disclosure =
            complete_request_disclosure(&config, "summary system", "summary user", 900).unwrap();
        let body: serde_json::Value = serde_json::from_str(&disclosure.request_body).unwrap();
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], "summary-model");
        assert!(disclosure.fallback_request_body.is_none());
        assert_eq!(
            disclosure.transport,
            "Complete response; no automatic retry."
        );
    }

    #[test]
    fn provider_maintenance_previews_are_exact_typed_and_fail_closed() {
        let config = ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "draft-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };

        let test_request = prepare_provider_test(&config).unwrap();
        let test_disclosure = test_request.disclosure();
        assert_eq!(test_disclosure.method, "POST");
        assert_eq!(
            test_disclosure.url,
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        let body: serde_json::Value = serde_json::from_str(&test_disclosure.request_body).unwrap();
        assert_eq!(body["model"], "draft-model");
        assert_eq!(
            body["messages"][0]["content"],
            "Reply with the single word OK."
        );
        assert_eq!(body["messages"][1]["content"], "ping");
        assert_eq!(body["max_tokens"], 16);
        assert_eq!(body["stream"], false);
        assert!(test_disclosure.fallback_request_body.is_none());
        let error = send_prepared_provider_models(test_request)
            .expect_err("a test preview must not authorize model listing");
        assert!(error.contains("not a model-list lookup"), "{error}");

        let models_request = prepare_provider_models(&config).unwrap();
        let models_disclosure = models_request.disclosure();
        assert_eq!(models_disclosure.method, "GET");
        assert_eq!(models_disclosure.url, "http://127.0.0.1:11434/v1/models");
        assert!(models_disclosure.request_body.is_empty());
        assert!(models_disclosure.fallback_request_body.is_none());
        let error = send_prepared(models_request)
            .expect_err("a model-list preview must not authorize inference");
        assert!(error.contains("model-list sender"), "{error}");

        let messages_config = ProviderConfig {
            format: ProviderFormat::MessagesApi,
            ..config.clone()
        };
        let error = prepare_provider_models(&messages_config)
            .expect_err("Messages API has no compatible /models probe");
        assert!(error.contains("does not expose"), "{error}");

        let mut tampered = prepare_provider_test(&config).unwrap();
        tampered.config.model = "swapped-after-review".to_string();
        let error = send_prepared(tampered).expect_err("tampered preview must fail before send");
        assert!(error.contains("no longer matches"), "{error}");

        let mut credential_changed = prepare_provider_test(&config).unwrap();
        credential_changed.credential_fingerprint = Some(credential_fingerprint("new-key"));
        let error = send_prepared(credential_changed)
            .expect_err("credential addition or rotation must invalidate the preview");
        assert!(error.contains("credential changed"), "{error}");
    }

    #[test]
    fn model_list_failures_distinguish_auth_timeout_invalid_and_empty() {
        assert!(model_list_http_error(401).contains("401 Unauthorized"));
        assert!(model_list_http_error(403).contains("403 Forbidden"));
        assert!(model_list_http_error(503).contains("HTTP 503"));
        assert!(model_list_transport_error(true, "opaque transport detail").contains("timed out"));
        assert!(!model_list_transport_error(true, "opaque transport detail")
            .contains("opaque transport detail"));
        assert!(
            model_list_transport_error(false, "connection refused").contains("connection refused")
        );

        let invalid = parse_model_list_response("not-json").unwrap_err();
        assert!(
            invalid.contains("invalid or incompatible JSON"),
            "{invalid}"
        );
        let empty = parse_model_list_response(r#"{"data":[{}, {"id":"  "}]}"#).unwrap_err();
        assert!(empty.contains("no usable model identifiers"), "{empty}");
        assert_eq!(
            parse_model_list_response(
                r#"{"data":[{"id":" alpha "},{"id":"alpha"},{"id":"beta"}]}"#
            )
            .unwrap(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn prepared_provider_test_sends_the_reviewed_fixed_ping_once() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let payload = r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            (request_line, String::from_utf8(body).unwrap())
        });
        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "test-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };

        let prepared = prepare_provider_test(&config).unwrap();
        let reviewed = prepared.disclosure().clone();
        assert_eq!(send_prepared(prepared).unwrap(), "OK");
        let (request_line, sent_body) = server.join().unwrap();
        assert!(request_line.starts_with("POST /v1/chat/completions"));
        assert_eq!(sent_body, reviewed.request_body);
    }

    #[test]
    fn prepared_provider_models_sends_the_reviewed_get_and_deduplicates() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let payload = r#"{"data":[{"id":"alpha"},{"id":"alpha"},{"id":"beta"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            request_line
        });
        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "unused-for-model-list".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };

        let prepared = prepare_provider_models(&config).unwrap();
        assert_eq!(
            send_prepared_provider_models(prepared).unwrap(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert!(server.join().unwrap().starts_with("GET /v1/models"));
    }

    #[test]
    fn local_stream_delivers_bounded_sse_deltas() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0_usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request_body = vec![0_u8; content_length];
            reader.read_exact(&mut request_body).unwrap();
            let payload = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"stream\"}}]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            String::from_utf8(request_body).unwrap()
        });
        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "stream-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let mut deltas = Vec::new();
        let result = explain_stream(&config, "sys", "usr", 64, |delta| {
            deltas.push(delta.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(result, "hello stream");
        assert_eq!(deltas, vec!["hello ", "stream"]);
        let request_body = server.join().unwrap();
        assert!(request_body.contains("\"stream\":true"));
    }

    #[test]
    fn local_stream_retries_once_without_streaming_when_unsupported() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let mut bodies = Vec::new();
            for attempt in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut content_length = 0_usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0_u8; content_length];
                reader.read_exact(&mut body).unwrap();
                bodies.push(String::from_utf8(body).unwrap());
                let (status, payload) = if attempt == 0 {
                    (
                        "400 Bad Request",
                        r#"{"error":{"message":"stream unsupported"}}"#,
                    )
                } else {
                    (
                        "200 OK",
                        r#"{"choices":[{"message":{"content":"fallback ok"}}]}"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
            }
            bodies
        });
        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "small-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let mut received = String::new();
        let result = explain_stream(&config, "sys", "usr", 64, |delta| {
            received.push_str(delta);
            Ok(())
        })
        .unwrap();
        assert_eq!(result, "fallback ok");
        assert_eq!(received, "fallback ok");
        let bodies = server.join().unwrap();
        assert!(bodies[0].contains("\"stream\":true"));
        assert!(bodies[1].contains("\"stream\":false"));
    }

    #[test]
    fn local_stream_stops_at_the_response_byte_limit() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let payload = "x".repeat(MAX_RESPONSE_BYTES + 128);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = reader.get_mut().write_all(header.as_bytes());
            let _ = reader.get_mut().write_all(payload.as_bytes());
        });
        let config = ProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "stream-model".to_string(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let error = explain_stream(&config, "sys", "usr", 64, |_| Ok(())).unwrap_err();
        assert!(error.contains("2 MiB safety limit"), "{error}");
        server.join().unwrap();
    }

    #[test]
    fn prefer_ipv4_rewrites_only_localhost() {
        // localhost -> 127.0.0.1 (Windows resolves localhost to ::1 first; local servers are IPv4)
        assert_eq!(
            prefer_ipv4_loopback("http://localhost:11434/v1/chat/completions"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            prefer_ipv4_loopback("http://LOCALHOST:1234/v1/models"),
            "http://127.0.0.1:1234/v1/models"
        );
        // Already-explicit hosts are left untouched.
        assert_eq!(
            prefer_ipv4_loopback("http://127.0.0.1:8000/v1/chat/completions"),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
        assert_eq!(
            prefer_ipv4_loopback("http://[::1]:8000/v1/chat/completions"),
            "http://[::1]:8000/v1/chat/completions"
        );
        assert_eq!(
            prefer_ipv4_loopback("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    // Real-data check against a locally-running Ollama (the exact scenario that failed:
    // `http://localhost:11434`). Run with: cargo test -p hangar-ai --ignored -- --nocapture
    // Proves the localhost->127.0.0.1 rewrite lets reqwest actually connect.
    #[test]
    #[ignore]
    fn live_ollama_localhost_connects() {
        let config = ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: String::new(),
            format: ProviderFormat::ChatCompletions,
            local: true,
        };
        let models = provider_models(&config).expect("provider_models should not error");
        println!("ollama models via localhost: {models:?}");
        // /v1/models returned (possibly empty if no models pulled), but the CONNECTION succeeded —
        // before the fix this errored with "error sending request" against ::1.
    }
}
