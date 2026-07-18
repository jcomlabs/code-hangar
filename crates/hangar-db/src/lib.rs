use chrono::Utc;
use hangar_core::{
    display_name_for_path, display_path_for_path, normalize_path, AdapterSummary,
    AgentActionRequest, AiGlossaryEntry, AiProviderConfig, CodeAnnotation, Comment,
    ConfirmedDuplicateGroup, ContextFile, DashboardSummary, DocumentHit, DocumentSearchResult,
    DuplicateCandidates, DuplicateConfirmProgress, DuplicateConfirmation, DuplicateGroup,
    DuplicateMember, FileIdentity, FileKind, FilePreview, FolderExplanation, FolderInvestigation,
    GitRepoSummary, GraphEdge, GraphIssue, GraphMap, GraphNode, InvestigationOwner,
    LostProjectCandidate, LostProjectCandidates, MarkdownLink, NavChildrenPage, NavItem,
    NodeRelationship, NodeRelationships, OperationPlan, OrphanCandidate, OrphanCandidates,
    OrphanStatus, PinnedItem, PreviewMode, PreviewPolicy, PreviewState, ProjectDetail,
    ProjectFootprintSummary, ProjectReviewCheckpoint, ProjectSummary, QuickOpenResult, RecentItem,
    RecoverableSummary, RelationshipIssue, ReviewLedgerEntry, RiskReport, ScanRoot, ScannedFile,
    SessionChangeSet,
};
#[cfg(feature = "agent_automation")]
use hangar_core::{AutomationActivityEntry, AutomationAgentSummary, AutomationReadGrant};
use hangar_graph::{
    cache_category, cache_category_is_shared_by_default, cache_category_label,
    extract_workflow_model_references, is_cache_path, is_model_path, is_workflow_candidate_path,
    model_category, model_header_probe_bytes, safetensors_header_len, summarize_gguf_header,
    summarize_safetensors_header, ModelHeaderError, WorkflowReference,
};
use hangar_nav::{build_tree, score_quick_open_with_project, sort_quick_open};
use hangar_preview::render_markdown_safe;
use hangar_protect::{
    context_priority, is_context_path, is_markdown_path, is_sensitive_path,
    is_strong_protected_path, protected_level_for_path, should_index_body,
};
use rusqlite::{params, params_from_iter, Connection, ErrorCode, OptionalExtension};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};
use thiserror::Error;

const PREVIEW_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const PREVIEW_SNIFF_BYTES: usize = 8 * 1024;
const DUPLICATE_MIN_SIZE_BYTES: u64 = 1024;
const DUPLICATE_PARTIAL_HASH_BYTES: usize = 64 * 1024;
const FULL_HASH_CHUNK_BYTES: usize = 1024 * 1024;
/// Max bind variables to put in one `IN (...)` list, well under the bundled SQLCipher
/// SQLITE_MAX_VARIABLE_NUMBER (32766). Chunk unbounded id/size lists by this.
const SQL_IN_CHUNK: usize = 500;
const PROJECT_CONTEXT_LIMIT: i64 = 160;
const MARKDOWN_EDGE_BACKFILL_SETTING: &str = "maintenance.markdown_edges.v1";
const MAX_STARTUP_MARKDOWN_BACKFILL_DOCS: i64 = 5_000;
const MAX_WORKFLOW_JSON_BYTES: u64 = 2 * 1024 * 1024;
const RECENT_LIMIT: i64 = 100;
const REVIEW_LEDGER_MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const REVIEW_LEDGER_MAX_ENTRIES_PER_PROJECT: i64 = 200;
const REVIEW_LEDGER_MAX_TOTAL_BYTES_PER_PROJECT: i64 = 32 * 1024 * 1024;
const REVIEW_LEDGER_MAX_AGE_DAYS: i64 = 180;
const GLOSSARY_MAX_ENTRIES: i64 = 128;
const ANNOTATION_MAX_NOTE_CHARS: usize = 2_000;
const ANNOTATION_MAX_SNIPPET_BYTES: usize = 16 * 1024;
const DB_KEY_BYTES: usize = 32;
const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database mutex poisoned")]
    MutexPoisoned,
    #[error("file read error: {0}")]
    FileRead(String),
}

pub type DbResult<T> = Result<T, DbError>;

/// Internal persisted annotation payload. The snippet is retained only inside
/// the encrypted catalog so the API can re-anchor the note after nearby edits;
/// it is never included in the serialized UI response.
#[derive(Debug, Clone)]
pub struct StoredCodeAnnotation {
    pub annotation: CodeAnnotation,
    pub snippet: String,
}

struct StoredLedgerRow {
    id: i64,
    project_id: i64,
    node_id: Option<i64>,
    source_kind: String,
    source_ref: String,
    source_modified_ms: i64,
    observed_at: String,
    origin: Option<String>,
    session_id: Option<String>,
    before_hash: Option<String>,
    after_hash: Option<String>,
    content_hash: String,
    previous_entry_hash: Option<String>,
    entry_hash: String,
    encoded_bytes: i64,
    change_set_json: String,
}

fn plan_error_to_db_error(error: hangar_plan::PlanError) -> DbError {
    match error {
        hangar_plan::PlanError::Sqlite(error) => DbError::Sqlite(error),
        other => DbError::FileRead(other.to_string()),
    }
}

fn run_interruptible_read<T>(
    conn: &Connection,
    cancel: Arc<AtomicBool>,
    f: impl FnOnce(&Connection) -> DbResult<T>,
) -> DbResult<T> {
    let done = Arc::new(AtomicBool::new(false));
    let interrupt = conn.get_interrupt_handle();
    let watcher_done = Arc::clone(&done);
    let watcher_cancel = Arc::clone(&cancel);
    let watcher = thread::spawn(move || {
        while !watcher_done.load(Ordering::Relaxed) {
            if watcher_cancel.load(Ordering::Relaxed) {
                interrupt.interrupt();
                return;
            }
            sleep(Duration::from_millis(25));
        }
    });

    let result = f(conn);
    done.store(true, Ordering::Relaxed);
    let _ = watcher.join();

    match result {
        Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(error, _)))
            if error.code == ErrorCode::OperationInterrupted || cancel.load(Ordering::Relaxed) =>
        {
            Err(DbError::FileRead("Cancelled".to_string()))
        }
        other => other,
    }
}

fn cancelled_db_error() -> DbError {
    DbError::FileRead("Cancelled".to_string())
}

fn check_cancelled(cancel: Option<&AtomicBool>) -> DbResult<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err(cancelled_db_error())
    } else {
        Ok(())
    }
}

fn run_interruptible_step<T>(
    conn: &Connection,
    cancel: Option<&Arc<AtomicBool>>,
    f: impl FnOnce(&Connection) -> DbResult<T>,
) -> DbResult<T> {
    if let Some(cancel) = cancel {
        check_cancelled(Some(cancel.as_ref()))?;
        let result = run_interruptible_read(conn, Arc::clone(cancel), f);
        if cancel.load(Ordering::Relaxed) {
            Err(cancelled_db_error())
        } else {
            result
        }
    } else {
        f(conn)
    }
}

#[derive(Debug, Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    path: Option<Arc<PathBuf>>,
    cipher_key: Option<Arc<String>>,
    // Free-list of already-configured SQLCipher read connections, reused across with_read_conn
    // calls. Opening a fresh encrypted connection runs the SQLCipher key derivation (KDF), which
    // dominates the cost of a small read; pooling pays that once per connection instead of on every
    // read. Empty/unused for in-memory DBs (those read through the single persistent connection).
    read_pool: Arc<Mutex<Vec<Connection>>>,
}

/// Upper bound on idle read connections kept in the pool. Concurrent reads beyond this still work
/// (extra connections are opened on demand) but surplus ones are closed instead of pooled.
const MAX_POOLED_READ_CONNS: usize = 6;

#[derive(Debug, Clone)]
pub struct ScanTarget {
    pub root_id: i64,
    pub raw_path: String,
    pub display_path: String,
}

#[derive(Debug, Clone)]
pub struct SubtreeScanTarget {
    pub root_id: i64,
    pub root_path: String,
    pub display_root_path: String,
    pub project_id: i64,
    pub nav_id: i64,
    pub relative_path: String,
    pub absolute_path: String,
}

#[derive(Debug, Clone)]
pub struct NodeWatchFingerprint {
    pub node_id: i64,
    pub project_id: Option<i64>,
    pub path: String,
    pub display_name: String,
    pub is_markdown: bool,
    pub is_context: bool,
    pub stored_mtime: Option<String>,
    pub stored_size: Option<u64>,
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiExplainTarget {
    pub path: String,
    pub is_sensitive: bool,
    pub protected_level: Option<String>,
    pub is_reparse: bool,
    pub reparse_kind: Option<String>,
}

pub struct DbWriteSession {
    conn: Connection,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NavAggregateTiming {
    pub select_ms: u64,
    pub compute_ms: u64,
    pub update_ms: u64,
}

pub struct RootScanFinish<'a> {
    pub root_path: &'a str,
    pub git: Option<&'a GitRepoSummary>,
    pub scan_completed: bool,
    pub cancel: Option<Arc<AtomicBool>>,
}

pub struct DocumentSearchOptions<'a> {
    pub query: &'a str,
    pub project_id: Option<i64>,
    pub indexed_kind: Option<&'a str>,
    pub path_filter: Option<&'a str>,
    pub name_filter: Option<&'a str>,
    pub include_fixture_projects: bool,
    pub limit: usize,
}

pub struct OrphanAssetSearchOptions<'a> {
    pub min_size_bytes: Option<u64>,
    pub project_id: Option<i64>,
    pub asset_kind: Option<&'a str>,
    pub min_confidence: Option<&'a str>,
    pub include_partial: bool,
    pub include_fixture_projects: bool,
    pub limit: usize,
}

pub struct LostProjectSearchOptions<'a> {
    pub min_size_bytes: Option<u64>,
    pub project_id: Option<i64>,
    pub stale_preset: Option<&'a str>,
    pub signals: &'a [String],
    pub keyword: Option<&'a str>,
    pub include_partial: bool,
    pub include_fixture_projects: bool,
    pub limit: usize,
}

struct LostProjectLoadOptions<'a> {
    min_size_bytes: u64,
    project_id: Option<i64>,
    stale_preset: &'a str,
    requested_signals: &'a [String],
    keyword: &'a str,
    include_partial: bool,
    include_fixture_projects: bool,
    limit: usize,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        let cipher_key = load_or_create_database_key(&key_path_for_database(&path))?;
        // Finish/clean up any migration a previous run left half-done, so a crash can
        // never strand a readable plaintext copy of the DB on disk (or lose data).
        reconcile_crashed_migration(&path, &cipher_key)?;
        if is_plaintext_sqlite_database(&path)? {
            migrate_plaintext_database(&path, &cipher_key)?;
        }
        let conn = Connection::open(&path)?;
        configure_file_connection(&conn, &cipher_key)?;
        checkpoint_stale_wal(&conn);
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(Arc::new(path)),
            cipher_key: Some(Arc::new(cipher_key)),
            read_pool: Arc::new(Mutex::new(Vec::new())),
        };
        db.init()?;
        Ok(db)
    }

    pub fn open_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        configure_memory_connection(&conn)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: None,
            cipher_key: None,
            read_pool: Arc::new(Mutex::new(Vec::new())),
        };
        db.init()?;
        Ok(db)
    }

    fn with_conn<T>(&self, mut f: impl FnMut(&Connection) -> DbResult<T>) -> DbResult<T> {
        retry_busy(|| {
            let conn = self.conn.lock().map_err(|_| DbError::MutexPoisoned)?;
            f(&conn)
        })
    }

    fn with_read_conn<T>(&self, mut f: impl FnMut(&Connection) -> DbResult<T>) -> DbResult<T> {
        if let Some(path) = &self.path {
            let cipher_key = self.cipher_key.as_deref().ok_or_else(|| {
                DbError::FileRead("File-backed database is missing its SQLCipher key.".to_string())
            })?;
            // Reuse a pooled, already-keyed read connection when one is free; otherwise open + key a
            // fresh one (retry_busy because the cipher-key validation touches the DB and can briefly
            // hit a lock). Either way the per-query work below is retried for transient busy/locked.
            let conn = match self.checkout_read_conn() {
                Some(conn) => conn,
                None => retry_busy(|| {
                    let conn = Connection::open(path.as_ref())?;
                    configure_file_read_connection(&conn, cipher_key)?;
                    Ok(conn)
                })?,
            };
            let result = retry_busy(|| f(&conn));
            // Only a connection that completed cleanly goes back to the pool; one whose query errored
            // (including an interrupted/cancelled read) is dropped so a half-used connection is never
            // reused.
            if result.is_ok() {
                self.checkin_read_conn(conn);
            }
            result
        } else {
            retry_busy(|| {
                let conn = self.conn.lock().map_err(|_| DbError::MutexPoisoned)?;
                f(&conn)
            })
        }
    }

    fn checkout_read_conn(&self) -> Option<Connection> {
        self.read_pool.lock().ok().and_then(|mut pool| pool.pop())
    }

    fn checkin_read_conn(&self, conn: Connection) {
        if let Ok(mut pool) = self.read_pool.lock() {
            if pool.len() < MAX_POOLED_READ_CONNS {
                pool.push(conn);
            }
            // Surplus connection falls out of scope here and is closed.
        }
    }

    fn with_writer<T>(&self, mut f: impl FnMut(&mut Connection) -> DbResult<T>) -> DbResult<T> {
        retry_busy(|| {
            if let Some(path) = &self.path {
                let cipher_key = self.cipher_key.as_deref().ok_or_else(|| {
                    DbError::FileRead(
                        "File-backed database is missing its SQLCipher key.".to_string(),
                    )
                })?;
                let mut conn = Connection::open(path.as_ref())?;
                configure_file_connection(&conn, cipher_key)?;
                f(&mut conn)
            } else {
                let mut conn = self.conn.lock().map_err(|_| DbError::MutexPoisoned)?;
                f(&mut conn)
            }
        })
    }

    pub fn open_write_session(&self) -> DbResult<DbWriteSession> {
        let Some(path) = &self.path else {
            return Err(DbError::FileRead(
                "Streaming scan writer requires a file-backed database.".to_string(),
            ));
        };
        let cipher_key = self.cipher_key.as_deref().ok_or_else(|| {
            DbError::FileRead("File-backed database is missing its SQLCipher key.".to_string())
        })?;
        let conn = Connection::open(path.as_ref())?;
        configure_file_connection(&conn, cipher_key)?;
        Ok(DbWriteSession { conn })
    }

    /// Writer for the Gate-3 mutation/recovery executors (backup, quarantine/move, permanent
    /// delete, restore). Unlike [`with_writer`], it runs the closure **exactly once** — it is
    /// never wrapped in `retry_busy`. These closures interleave irreversible filesystem
    /// mutations (backup copies, file moves, unlinks) with incremental journal commits;
    /// re-invoking the whole closure on a transient `SQLITE_BUSY`/`SQLITE_LOCKED` would
    /// re-copy / re-move / re-unlink files, orphan a half-written `executing` operation row,
    /// and double-consume a single-use confirmation token. The connection still carries
    /// `busy_timeout=5000`, so every individual statement waits up to 5s for the write lock;
    /// only the unsafe whole-closure retry is removed. A busy that outlasts the timeout fails
    /// the mutation cleanly, leaving an incrementally-journaled partial state that startup
    /// recovery reconciles — exactly the Gate-3 crash-consistency contract.
    #[cfg(feature = "mutation")]
    pub fn with_recovery_writer<T>(
        &self,
        mut f: impl FnMut(&mut Connection) -> DbResult<T>,
    ) -> DbResult<T> {
        if let Some(path) = &self.path {
            let cipher_key = self.cipher_key.as_deref().ok_or_else(|| {
                DbError::FileRead("File-backed database is missing its SQLCipher key.".to_string())
            })?;
            let mut conn = Connection::open(path.as_ref())?;
            configure_file_connection(&conn, cipher_key)?;
            f(&mut conn)
        } else {
            let mut conn = self.conn.lock().map_err(|_| DbError::MutexPoisoned)?;
            f(&mut conn)
        }
    }

    fn init(&self) -> DbResult<()> {
        self.with_conn(|conn| {
            conn.execute_batch(MIGRATION_001)?;
            ensure_phase1b_columns(conn)?;
            ensure_change_ledger_schema(conn)?;
            insert_default_zones(conn)?;
            seed_builtin_adapters(conn)?;
            #[cfg(feature = "agent_automation")]
            ensure_automation_schema(conn)?;
            load_fixtures_if_empty(conn)?;
            reconcile_stale_context_classification(conn)?;
            Ok(())
        })
    }

    pub fn run_startup_maintenance(&self) -> DbResult<()> {
        self.with_writer(|conn| {
            backfill_missing_nav_aggregates(conn)?;
            backfill_missing_markdown_edges(conn)?;
            Ok(())
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_register(
        &self,
        name: &str,
        token_hash: &str,
        scopes: &[String],
        project_ids: &[i64],
    ) -> DbResult<AutomationAgentSummary> {
        self.with_writer(|conn| {
            ensure_automation_schema(conn)?;
            let scopes_json = serde_json::to_string(scopes)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let projects_json = serde_json::to_string(project_ids)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let created_at = now();
            conn.execute(
                "INSERT INTO automation_agent(name, token_hash, scopes_json, project_ids_json, enabled, created_at)
                 VALUES(?1, ?2, ?3, ?4, 1, ?5)",
                params![name, token_hash, scopes_json, projects_json, created_at],
            )?;
            Ok(AutomationAgentSummary {
                id: conn.last_insert_rowid(),
                name: name.to_string(),
                scopes: scopes.to_vec(),
                project_ids: project_ids.to_vec(),
                enabled: true,
                created_at,
                last_seen_at: None,
            })
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_agents(&self) -> DbResult<Vec<AutomationAgentSummary>> {
        self.with_read_conn(|conn| {
            ensure_automation_schema(conn)?;
            let mut stmt = conn.prepare(
                "SELECT id, name, scopes_json, project_ids_json, enabled, created_at, last_seen_at
                 FROM automation_agent ORDER BY id DESC",
            )?;
            let rows = stmt.query_map([], automation_agent_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_authenticate(
        &self,
        token_hash: &str,
    ) -> DbResult<Option<AutomationAgentSummary>> {
        self.with_writer(|conn| {
            ensure_automation_schema(conn)?;
            let agent = conn
                .query_row(
                    "SELECT id, name, scopes_json, project_ids_json, enabled, created_at, last_seen_at
                     FROM automation_agent WHERE token_hash = ?1 AND enabled = 1",
                    params![token_hash],
                    automation_agent_from_row,
                )
                .optional()?;
            if let Some(agent) = &agent {
                conn.execute(
                    "UPDATE automation_agent SET last_seen_at = ?1 WHERE id = ?2",
                    params![now(), agent.id],
                )?;
            }
            Ok(agent)
        })
    }

    /// Resolve an enabled agent by token hash WITHOUT the `last_seen` bump — a pure
    /// read used to compute the scope-aware MCP tool catalog on `tools/list` (which
    /// must not be a write). Returns None for an invalid/revoked/disabled token, so
    /// the caller can fall back to advertising only the read-only tool set. The real
    /// per-call `automation_authenticate` (which does bump `last_seen`) still runs on
    /// every actual `tools/call`, so this never becomes the auth path.
    #[cfg(feature = "agent_automation")]
    pub fn automation_scopes_for_token(&self, token_hash: &str) -> DbResult<Option<Vec<String>>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, name, scopes_json, project_ids_json, enabled, created_at, last_seen_at
                 FROM automation_agent WHERE token_hash = ?1 AND enabled = 1",
                params![token_hash],
                automation_agent_from_row,
            )
            .optional()
            .map(|agent| agent.map(|agent| agent.scopes))
            .map_err(DbError::from)
        })
    }

    /// Load an agent by id, but ONLY if it is still enabled. Returns None for a
    /// revoked/disabled/forgotten agent — so a queued request can be re-authorized
    /// against the agent's CURRENT standing at approval time.
    #[cfg(feature = "agent_automation")]
    pub fn automation_agent_by_id(
        &self,
        agent_id: i64,
    ) -> DbResult<Option<AutomationAgentSummary>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, name, scopes_json, project_ids_json, enabled, created_at, last_seen_at
                 FROM automation_agent WHERE id = ?1 AND enabled = 1",
                params![agent_id],
                automation_agent_from_row,
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_revoke(&self, agent_id: i64) -> DbResult<bool> {
        self.with_writer(|conn| {
            ensure_automation_schema(conn)?;
            // One transaction: the comment tombstone below is NOT idempotent (it appends a
            // marker), so a retry_busy re-run without a transaction could double-append.
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE automation_agent SET enabled = 0 WHERE id = ?1 AND enabled = 1",
                params![agent_id],
            )?;
            if changed > 0 {
                // Tombstone the comments this agent authored so a future agent that re-uses the
                // same display name cannot inherit edit/delete rights over them
                // (guard_comment_actor keys ownership on source == name). The comments stay
                // visible as agent records and the local user can still delete them.
                tx.execute(
                    "UPDATE comment
                     SET source = source || ' (revoked agent)'
                     WHERE source <> 'user'
                       AND source = (SELECT name FROM automation_agent WHERE id = ?1)",
                    params![agent_id],
                )?;
            }
            tx.execute(
                "UPDATE automation_read_grant SET revoked_at = ?1
                 WHERE agent_id = ?2 AND revoked_at IS NULL",
                params![now(), agent_id],
            )?;
            tx.commit()?;
            Ok(changed > 0)
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_forget_revoked(&self, agent_id: i64) -> DbResult<bool> {
        self.with_writer(|conn| {
            ensure_automation_schema(conn)?;
            let changed = conn.execute(
                "DELETE FROM automation_agent WHERE id = ?1 AND enabled = 0",
                params![agent_id],
            )?;
            Ok(changed > 0)
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_grant_read(
        &self,
        agent_id: i64,
        node_id: i64,
        expires_at_ms: i64,
    ) -> DbResult<AutomationReadGrant> {
        self.with_writer(|conn| {
            ensure_automation_schema(conn)?;
            let enabled: bool = conn.query_row(
                "SELECT enabled FROM automation_agent WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )?;
            if !enabled {
                return Err(DbError::FileRead("The local agent is revoked.".to_string()));
            }
            conn.execute(
                "INSERT INTO automation_read_grant(agent_id, node_id, expires_at_ms, created_at)
                 VALUES(?1, ?2, ?3, ?4)",
                params![agent_id, node_id, expires_at_ms, now()],
            )?;
            Ok(AutomationReadGrant {
                id: conn.last_insert_rowid(),
                agent_id,
                node_id,
                expires_at_ms,
                revoked: false,
            })
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_has_read_grant(
        &self,
        agent_id: i64,
        node_id: i64,
        now_ms: i64,
    ) -> DbResult<bool> {
        self.with_read_conn(|conn| {
            ensure_automation_schema(conn)?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM automation_read_grant
                 WHERE agent_id = ?1 AND node_id = ?2 AND revoked_at IS NULL AND expires_at_ms > ?3",
                params![agent_id, node_id, now_ms],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_log(
        &self,
        agent_id: Option<i64>,
        method: &str,
        status: &str,
        detail: &str,
    ) -> DbResult<()> {
        self.with_writer(|conn| {
            ensure_automation_schema(conn)?;
            conn.execute(
                "INSERT INTO automation_activity(agent_id, method, status, detail, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![agent_id, method, status, detail, now()],
            )?;
            Ok(())
        })
    }

    #[cfg(feature = "agent_automation")]
    pub fn automation_activity(&self, limit: usize) -> DbResult<Vec<AutomationActivityEntry>> {
        self.with_read_conn(|conn| {
            ensure_automation_schema(conn)?;
            let mut stmt = conn.prepare(
                "SELECT aa.id, aa.agent_id, a.name, aa.method, aa.status, aa.detail, aa.created_at
                 FROM automation_activity aa
                 LEFT JOIN automation_agent a ON a.id = aa.agent_id
                 ORDER BY aa.id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit.min(1000) as i64], |row| {
                Ok(AutomationActivityEntry {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    agent_name: row.get(2)?,
                    method: row.get(3)?,
                    status: row.get(4)?,
                    detail: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    #[cfg(feature = "mutation")]
    pub fn node_project_id(&self, node_id: i64) -> DbResult<Option<i64>> {
        self.with_read_conn(|conn| {
            // ORDER BY makes the choice deterministic for a node that several projects
            // inventory (otherwise the "owning project" — and any scope decision keyed
            // on it — would depend on arbitrary row order).
            conn.query_row(
                "SELECT project_id FROM nav_item WHERE node_id = ?1 ORDER BY project_id LIMIT 1",
                params![node_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    /// Every registered project that inventories this node, ascending and
    /// deterministic. Used to resolve a node's owning project against an agent's
    /// grants without depending on arbitrary row order.
    #[cfg(feature = "agent_automation")]
    pub fn node_project_ids(&self, node_id: i64) -> DbResult<Vec<i64>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT project_id FROM nav_item WHERE node_id = ?1 ORDER BY project_id",
            )?;
            let rows = stmt.query_map(params![node_id], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
        })
    }

    pub fn projects_list(&self) -> DbResult<Vec<ProjectSummary>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "WITH nav_state AS (
                    SELECT project_id,
                           COUNT(*) AS item_count,
                           SUM(CASE
                               WHEN fully_scanned = 0
                                 OR scan_error IS NOT NULL
                                 OR aggregate_bytes_partial = 1
                               THEN 1 ELSE 0 END) AS partial_count
                    FROM nav_item
                    GROUP BY project_id
                 ),
                 context_counts AS (
                    SELECT project_id, COUNT(*) AS context_count
                    FROM nav_item
                    WHERE is_context = 1
                       OR lower(path) LIKE '.claude/%.md'
                       OR lower(path) LIKE '%/.claude/%.md'
                       OR lower(path) LIKE 'commands/%.md'
                       OR lower(path) LIKE '%/commands/%.md'
                       OR lower(path) LIKE 'instructions/%.md'
                       OR lower(path) LIKE '%/instructions/%.md'
                    GROUP BY project_id
                 ),
                 pinned_projects AS (
                    SELECT DISTINCT node_id
                    FROM pinned_item
                    WHERE item_kind = 'project'
                 )
                 SELECT p.id, p.name, p.path, COALESCE(json_extract(p.attributes, '$.source'), 'fixture') AS source,
                        p.protected_level,
                        COALESCE(context_counts.context_count, 0) AS context_count,
                        pinned_projects.node_id IS NOT NULL AS pinned,
                        CASE
                            WHEN sr.id IS NOT NULL
                             AND (
                                sr.last_scanned_at IS NULL
                                OR COALESCE(nav_state.partial_count, 0) > 0
                                OR COALESCE(nav_state.item_count, 0) = 0
                             )
                            THEN 'outdated'
                            ELSE 'scanned'
                        END AS scan_state,
                        sr.id AS scan_root_id
                 FROM node p
                 LEFT JOIN scan_root sr ON sr.path = p.path
                 LEFT JOIN nav_state ON nav_state.project_id = p.id
                 LEFT JOIN context_counts ON context_counts.project_id = p.id
                 LEFT JOIN pinned_projects ON pinned_projects.node_id = p.id
                 WHERE p.kind = 'project' AND p.present = 1 AND COALESCE(sr.adhoc, 0) = 0
                 ORDER BY pinned DESC, p.name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ProjectSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    source: row.get(3)?,
                    protected_level: row.get(4)?,
                    context_count: row.get(5)?,
                    pinned: row.get::<_, i64>(6)? == 1,
                    scan_state: row.get(7)?,
                    scan_root_id: row.get(8)?,
                    antigravity_name: None,
                    // Enriched by the API layer; the DB never marks a project current
                    // or attributes it to an app.
                    is_current: false,
                    app: None,
                    apps: Vec::new(),
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn projects_list_lite(&self) -> DbResult<Vec<ProjectSummary>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "WITH nav_state AS (
                    SELECT project_id,
                           COUNT(*) AS item_count,
                           SUM(CASE
                               WHEN fully_scanned = 0
                                 OR scan_error IS NOT NULL
                                 OR aggregate_bytes_partial = 1
                               THEN 1 ELSE 0 END) AS partial_count
                    FROM nav_item
                    GROUP BY project_id
                 ),
                 context_counts AS (
                    SELECT project_id, COUNT(*) AS context_count
                    FROM nav_item
                    WHERE is_context = 1
                       OR lower(path) LIKE '.claude/%.md'
                       OR lower(path) LIKE '%/.claude/%.md'
                       OR lower(path) LIKE 'commands/%.md'
                       OR lower(path) LIKE '%/commands/%.md'
                       OR lower(path) LIKE 'instructions/%.md'
                       OR lower(path) LIKE '%/instructions/%.md'
                    GROUP BY project_id
                 ),
                 pinned_projects AS (
                    SELECT DISTINCT node_id
                    FROM pinned_item
                    WHERE item_kind = 'project'
                 )
                 SELECT p.id, p.name, p.path, COALESCE(json_extract(p.attributes, '$.source'), 'fixture') AS source,
                        p.protected_level,
                        COALESCE(context_counts.context_count, 0) AS context_count,
                        pinned_projects.node_id IS NOT NULL AS pinned,
                        CASE
                            WHEN sr.id IS NOT NULL
                             AND (
                                sr.last_scanned_at IS NULL
                                OR COALESCE(nav_state.partial_count, 0) > 0
                                OR COALESCE(nav_state.item_count, 0) = 0
                             )
                            THEN 'outdated'
                            ELSE 'scanned'
                        END AS scan_state,
                        sr.id AS scan_root_id
                 FROM node p
                 LEFT JOIN scan_root sr ON sr.path = p.path
                 LEFT JOIN nav_state ON nav_state.project_id = p.id
                 LEFT JOIN context_counts ON context_counts.project_id = p.id
                 LEFT JOIN pinned_projects ON pinned_projects.node_id = p.id
                 WHERE p.kind = 'project' AND p.present = 1 AND COALESCE(sr.adhoc, 0) = 0
                 ORDER BY pinned DESC, p.name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ProjectSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    source: row.get(3)?,
                    protected_level: row.get(4)?,
                    context_count: row.get(5)?,
                    pinned: row.get::<_, i64>(6)? == 1,
                    scan_state: row.get(7)?,
                    scan_root_id: row.get(8)?,
                    antigravity_name: None,
                    // Enriched by the API layer; the DB never marks a project current
                    // or attributes it to an app.
                    is_current: false,
                    app: None,
                    apps: Vec::new(),
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn project_get(&self, project_id: i64) -> DbResult<Option<ProjectDetail>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT p.id, p.name, p.path, COALESCE(json_extract(p.attributes, '$.source'), 'fixture') AS source,
                        p.protected_level,
                        COALESCE((SELECT COUNT(*) FROM nav_item n WHERE n.project_id = p.id AND (
                            n.is_context = 1
                            OR lower(n.path) LIKE '.claude/%.md'
                            OR lower(n.path) LIKE '%/.claude/%.md'
                            OR lower(n.path) LIKE 'commands/%.md'
                            OR lower(n.path) LIKE '%/commands/%.md'
                            OR lower(n.path) LIKE 'instructions/%.md'
                            OR lower(n.path) LIKE '%/instructions/%.md'
                        )), 0) AS context_count,
                        CASE
                            WHEN sr.id IS NOT NULL
                             AND (
                                sr.last_scanned_at IS NULL
                                OR EXISTS(
                                    SELECT 1
                                    FROM nav_item partial
                                    WHERE partial.project_id = p.id
                                      AND (
                                        partial.fully_scanned = 0
                                        OR partial.scan_error IS NOT NULL
                                        OR partial.aggregate_bytes_partial = 1
                                      )
                                )
                                OR NOT EXISTS(
                                    SELECT 1
                                    FROM nav_item any_item
                                    WHERE any_item.project_id = p.id
                                )
                             )
                            THEN 'outdated'
                            ELSE 'scanned'
                        END AS scan_state,
                        sr.id AS scan_root_id
                 FROM node p
                 LEFT JOIN scan_root sr ON sr.path = p.path
                 WHERE p.kind = 'project' AND p.id = ?1",
                params![project_id],
                |row| {
                    Ok(ProjectDetail {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        path: row.get(2)?,
                        source: row.get(3)?,
                        protected_level: row.get(4)?,
                        context_count: row.get(5)?,
                        scan_state: row.get(6)?,
                        scan_root_id: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    pub fn project_review_checkpoint(
        &self,
        project_id: i64,
    ) -> DbResult<Option<ProjectReviewCheckpoint>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT project_id, reviewed_at, session_cutoff_ms, git_fingerprint, git_head
                 FROM project_review_checkpoint WHERE project_id = ?1",
                [project_id],
                |row| {
                    Ok(ProjectReviewCheckpoint {
                        project_id: row.get(0)?,
                        reviewed_at: row.get(1)?,
                        session_cutoff_ms: row.get(2)?,
                        git_fingerprint: row.get(3)?,
                        git_head: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    pub fn project_review_checkpoints(&self) -> DbResult<Vec<ProjectReviewCheckpoint>> {
        self.with_read_conn(|conn| {
            let mut statement = conn.prepare(
                "SELECT checkpoint.project_id, checkpoint.reviewed_at,
                        checkpoint.session_cutoff_ms, checkpoint.git_fingerprint,
                        checkpoint.git_head
                 FROM project_review_checkpoint checkpoint
                 INNER JOIN node project ON project.id = checkpoint.project_id
                 WHERE project.kind = 'project' AND project.present = 1
                 ORDER BY checkpoint.reviewed_at DESC, checkpoint.project_id ASC",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(ProjectReviewCheckpoint {
                    project_id: row.get(0)?,
                    reviewed_at: row.get(1)?,
                    session_cutoff_ms: row.get(2)?,
                    git_fingerprint: row.get(3)?,
                    git_head: row.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
        })
    }

    pub fn set_project_review_checkpoint(
        &self,
        project_id: i64,
        session_cutoff_ms: i64,
        git_fingerprint: Option<&str>,
        git_head: Option<&str>,
    ) -> DbResult<ProjectReviewCheckpoint> {
        let reviewed_at = now();
        self.with_writer(|conn| {
            let project_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM node WHERE id = ?1 AND kind = 'project' AND present = 1)",
                [project_id],
                |row| row.get(0),
            )?;
            if !project_exists {
                return Err(DbError::FileRead(
                    "The project is no longer registered.".to_string(),
                ));
            }
            conn.execute(
                "INSERT INTO project_review_checkpoint(
                    project_id, reviewed_at, session_cutoff_ms, git_fingerprint, git_head
                 ) VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(project_id) DO UPDATE SET
                    reviewed_at = excluded.reviewed_at,
                    session_cutoff_ms = excluded.session_cutoff_ms,
                    git_fingerprint = excluded.git_fingerprint,
                    git_head = excluded.git_head",
                params![
                    project_id,
                    reviewed_at,
                    session_cutoff_ms,
                    git_fingerprint,
                    git_head
                ],
            )?;
            Ok(ProjectReviewCheckpoint {
                project_id,
                reviewed_at: reviewed_at.clone(),
                session_cutoff_ms,
                git_fingerprint: git_fingerprint.map(str::to_string),
                git_head: git_head.map(str::to_string),
            })
        })
    }

    pub fn project_check_approval(
        &self,
        project_id: i64,
        check_id: &str,
    ) -> DbResult<Option<(String, String)>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT fingerprint, approved_at
                 FROM project_check_approval
                 WHERE project_id = ?1 AND check_id = ?2",
                params![project_id, check_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    pub fn set_project_check_approval(
        &self,
        project_id: i64,
        check_id: &str,
        fingerprint: &str,
    ) -> DbResult<String> {
        let approved_at = now();
        self.with_writer(|conn| {
            let project_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM node WHERE id = ?1 AND kind = 'project' AND present = 1)",
                [project_id],
                |row| row.get(0),
            )?;
            if !project_exists {
                return Err(DbError::FileRead(
                    "The project is no longer registered.".to_string(),
                ));
            }
            conn.execute(
                "INSERT INTO project_check_approval(project_id, check_id, fingerprint, approved_at)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(project_id, check_id) DO UPDATE SET
                    fingerprint = excluded.fingerprint,
                    approved_at = excluded.approved_at",
                params![project_id, check_id, fingerprint, approved_at],
            )?;
            Ok(approved_at.clone())
        })
    }

    pub fn revoke_project_check_approval(&self, project_id: i64, check_id: &str) -> DbResult<bool> {
        self.with_writer(|conn| {
            Ok(conn.execute(
                "DELETE FROM project_check_approval WHERE project_id = ?1 AND check_id = ?2",
                params![project_id, check_id],
            )? > 0)
        })
    }

    pub fn store_review_evidence(
        &self,
        project_id: i64,
        source_ref: &str,
        source_modified_ms: Option<i64>,
        change_set: &SessionChangeSet,
    ) -> DbResult<i64> {
        self.store_change_evidence(
            project_id,
            None,
            source_ref,
            source_modified_ms,
            None,
            None,
            None,
            None,
            change_set,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_change_evidence(
        &self,
        project_id: i64,
        node_id: Option<i64>,
        source_ref: &str,
        source_modified_ms: Option<i64>,
        origin: Option<&str>,
        session_id: Option<&str>,
        before_hash: Option<&str>,
        after_hash: Option<&str>,
        change_set: &SessionChangeSet,
    ) -> DbResult<i64> {
        let normalized = normalize_ledger_change_set(change_set);
        let encoded = serde_json::to_string(&normalized).map_err(|error| {
            DbError::FileRead(format!("Could not encode review evidence: {error}"))
        })?;
        if encoded.len() > REVIEW_LEDGER_MAX_JSON_BYTES {
            return Err(DbError::FileRead(
                "This reconstruction is too large to retain in the review ledger.".to_string(),
            ));
        }
        let observed_at = now();
        let source_modified_ms = source_modified_ms.unwrap_or(-1);
        let content_hash = blake3::hash(encoded.as_bytes()).to_hex().to_string();
        let encoded_bytes = i64::try_from(encoded.len()).unwrap_or(i64::MAX);
        self.with_writer(|conn| {
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM change_ledger
                     WHERE project_id = ?1 AND source_ref = ?2
                       AND source_modified_ms = ?3 AND content_hash = ?4
                     ORDER BY id DESC LIMIT 1",
                    params![project_id, source_ref, source_modified_ms, content_hash],
                    |row| row.get(0),
                )
                .optional()?
            {
                return Ok(id);
            }
            let previous_entry_hash = conn
                .query_row(
                    "SELECT entry_hash FROM change_ledger
                     WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
                    [project_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let entry_hash = change_ledger_entry_hash(
                project_id,
                source_ref,
                source_modified_ms,
                &observed_at,
                &content_hash,
                previous_entry_hash.as_deref(),
            );
            conn.execute(
                "INSERT INTO change_ledger(
                    project_id, node_id, source_kind, source_ref, source_modified_ms,
                    observed_at, origin, session_id, before_hash, after_hash, content_hash,
                    previous_entry_hash, entry_hash, encoded_bytes, change_set_json
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    project_id,
                    node_id,
                    normalized.source_kind,
                    source_ref,
                    source_modified_ms,
                    observed_at,
                    origin,
                    session_id,
                    before_hash,
                    after_hash,
                    content_hash,
                    previous_entry_hash,
                    entry_hash,
                    encoded_bytes,
                    encoded
                ],
            )?;
            let id = conn.last_insert_rowid();
            conn.execute(
                "DELETE FROM change_ledger
                 WHERE project_id = ?1 AND id NOT IN (
                    SELECT id FROM change_ledger WHERE project_id = ?1
                    ORDER BY id DESC LIMIT ?2
                 )",
                params![project_id, REVIEW_LEDGER_MAX_ENTRIES_PER_PROJECT],
            )?;
            conn.execute(
                "DELETE FROM change_ledger
                 WHERE project_id = ?1
                   AND datetime(observed_at) < datetime('now', ?2)",
                params![project_id, format!("-{REVIEW_LEDGER_MAX_AGE_DAYS} days")],
            )?;
            conn.execute(
                "DELETE FROM change_ledger WHERE id IN (
                   SELECT id FROM (
                     SELECT id,
                            SUM(encoded_bytes) OVER (ORDER BY id DESC) AS retained_bytes
                     FROM change_ledger WHERE project_id = ?1
                   ) WHERE retained_bytes > ?2
                 )",
                params![project_id, REVIEW_LEDGER_MAX_TOTAL_BYTES_PER_PROJECT],
            )?;
            Ok(id)
        })
    }

    pub fn project_review_ledger(
        &self,
        project_id: i64,
        limit: usize,
    ) -> DbResult<Vec<ReviewLedgerEntry>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, node_id, source_kind, source_ref, source_modified_ms,
                        observed_at, origin, session_id, before_hash, after_hash, content_hash,
                        previous_entry_hash, entry_hash, encoded_bytes, change_set_json
                 FROM change_ledger WHERE project_id = ?1
                 ORDER BY id ASC",
            )?;
            let raw = stmt
                .query_map([project_id], |row| {
                    Ok(StoredLedgerRow {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        node_id: row.get(2)?,
                        source_kind: row.get(3)?,
                        source_ref: row.get(4)?,
                        source_modified_ms: row.get(5)?,
                        observed_at: row.get(6)?,
                        origin: row.get(7)?,
                        session_id: row.get(8)?,
                        before_hash: row.get(9)?,
                        after_hash: row.get(10)?,
                        content_hash: row.get(11)?,
                        previous_entry_hash: row.get(12)?,
                        entry_hash: row.get(13)?,
                        encoded_bytes: row.get(14)?,
                        change_set_json: row.get(15)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (index, row) in raw.iter().enumerate() {
                if row.entry_hash.starts_with("legacy:") {
                    continue;
                }
                let content_hash = blake3::hash(row.change_set_json.as_bytes())
                    .to_hex()
                    .to_string();
                let expected = change_ledger_entry_hash(
                    row.project_id,
                    &row.source_ref,
                    row.source_modified_ms,
                    &row.observed_at,
                    &content_hash,
                    row.previous_entry_hash.as_deref(),
                );
                if content_hash != row.content_hash || expected != row.entry_hash {
                    return Err(DbError::FileRead(format!(
                        "Stored review evidence {} failed its integrity check.",
                        row.id
                    )));
                }
                if index > 0
                    && row.previous_entry_hash.as_deref()
                        != Some(raw[index - 1].entry_hash.as_str())
                {
                    return Err(DbError::FileRead(format!(
                        "Stored review evidence {} is not linked to the prior retained entry.",
                        row.id
                    )));
                }
            }
            let mut raw = raw;
            raw.sort_by(|left, right| {
                right
                    .source_modified_ms
                    .cmp(&left.source_modified_ms)
                    .then_with(|| right.id.cmp(&left.id))
            });
            raw.truncate(limit.clamp(1, 500));
            raw.into_iter()
                .map(|row| {
                    let change_set =
                        serde_json::from_str(&row.change_set_json).map_err(|error| {
                            DbError::FileRead(format!(
                                "Stored review evidence {} is unreadable: {error}",
                                row.id
                            ))
                        })?;
                    Ok(ReviewLedgerEntry {
                        id: row.id,
                        project_id: row.project_id,
                        node_id: row.node_id,
                        source_kind: row.source_kind,
                        source_ref: row.source_ref,
                        source_modified_ms: (row.source_modified_ms >= 0)
                            .then_some(row.source_modified_ms),
                        observed_at: row.observed_at,
                        origin: row.origin,
                        session_id: row.session_id,
                        before_hash: row.before_hash,
                        after_hash: row.after_hash,
                        previous_entry_hash: row.previous_entry_hash,
                        entry_hash: row.entry_hash,
                        encoded_bytes: row.encoded_bytes.max(0) as u64,
                        change_set,
                    })
                })
                .collect()
        })
    }

    pub fn project_nav_tree(&self, project_id: i64) -> DbResult<Vec<NavItem>> {
        self.with_read_conn(|conn| {
            let items = load_nav_items(conn, "WHERE project_id = ?1", params![project_id])?;
            Ok(build_tree(items))
        })
    }

    pub fn project_nav_children(
        &self,
        project_id: i64,
        parent_nav_id: Option<i64>,
        limit: usize,
        offset: usize,
    ) -> DbResult<NavChildrenPage> {
        self.with_read_conn(|conn| {
            let total: i64 = match parent_nav_id {
                Some(parent) => conn.query_row(
                    "SELECT COUNT(*) FROM nav_item WHERE project_id = ?1 AND parent_nav_id = ?2",
                    params![project_id, parent],
                    |row| row.get(0),
                )?,
                None => conn.query_row(
                    "SELECT COUNT(*) FROM nav_item WHERE project_id = ?1 AND parent_nav_id IS NULL",
                    params![project_id],
                    |row| row.get(0),
                )?,
            };
            let clause = match parent_nav_id {
                Some(_) => "WHERE project_id = ?1 AND parent_nav_id = ?2 ORDER BY priority, sort_key, nav_item.id LIMIT ?3 OFFSET ?4",
                None => "WHERE project_id = ?1 AND parent_nav_id IS NULL ORDER BY priority, sort_key, nav_item.id LIMIT ?2 OFFSET ?3",
            };
            let items = match parent_nav_id {
                Some(parent) => load_nav_items_unordered(
                    conn,
                    clause,
                    params![project_id, parent, limit as i64, offset as i64],
                )?,
                None => load_nav_items_unordered(
                    conn,
                    clause,
                    params![project_id, limit as i64, offset as i64],
                )?,
            };
            Ok(NavChildrenPage {
                has_more: (offset as i64 + items.len() as i64) < total,
                items,
                total,
            })
        })
    }

    /// Returns the smallest root-to-item chain needed to reveal one node in a
    /// paged navigation tree. This deliberately avoids loading the full project
    /// tree for a single reveal action.
    pub fn project_nav_path(&self, project_id: i64, node_id: i64) -> DbResult<Vec<NavItem>> {
        self.with_read_conn(|conn| {
            let mut target = load_nav_items_unordered(
                conn,
                "WHERE nav_item.project_id = ?1 AND nav_item.node_id = ?2 ORDER BY nav_item.id LIMIT 1",
                params![project_id, node_id],
            )?;
            let Some(mut current) = target.pop() else {
                return Ok(Vec::new());
            };
            let mut path = Vec::new();
            loop {
                let parent_nav_id = current.parent_nav_id;
                path.push(current);
                let Some(parent_nav_id) = parent_nav_id else {
                    break;
                };
                if path.len() >= 256 || path.iter().any(|item| item.id == parent_nav_id) {
                    break;
                }
                let mut parents = load_nav_items_unordered(
                    conn,
                    "WHERE nav_item.project_id = ?1 AND nav_item.id = ?2 LIMIT 1",
                    params![project_id, parent_nav_id],
                )?;
                let Some(parent) = parents.pop() else {
                    break;
                };
                current = parent;
            }
            path.reverse();
            Ok(path)
        })
    }

    pub fn folder_explanation(&self, nav_id: i64) -> DbResult<Option<FolderExplanation>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, project_id, COALESCE(display_path, path), display_name, item_kind,
                        child_count, aggregate_apparent_bytes, aggregate_allocated_bytes,
                        aggregate_physical_bytes, aggregate_bytes_partial, protected_level,
                        fully_scanned, scan_error, collapse_default, is_context, is_sensitive
                 FROM nav_item
                 WHERE id = ?1",
                params![nav_id],
                |row| {
                    let item_kind: String = row.get(4)?;
                    let display_path: String = row.get(2)?;
                    let display_name: String = row.get(3)?;
                    let protected_level = row.get::<_, Option<String>>(10)?;
                    let fully_scanned = row.get::<_, i64>(11)? == 1;
                    let scan_error = row.get::<_, Option<String>>(12)?;
                    let collapse_default = row.get::<_, i64>(13)? == 1;
                    let is_context = row.get::<_, i64>(14)? == 1;
                    let is_sensitive = row.get::<_, i64>(15)? == 1;
                    let footprint_partial = row.get::<_, i64>(9)? == 1
                        || !fully_scanned
                        || scan_error.is_some();
                    let (classification, confidence, summary) = explain_folder_classification(
                        &display_name,
                        &display_path,
                        &item_kind,
                        protected_level.as_deref(),
                        is_context,
                        is_sensitive,
                    );
                    let mut signals = vec![format!("{} direct child entries", row.get::<_, i64>(5)?)];
                    if collapse_default {
                        signals.push("Starts collapsed because it is likely noisy or large.".to_string());
                    }
                    if is_context {
                        signals.push("Contains prioritized project context.".to_string());
                    }
                    if is_sensitive {
                        signals.push("Matches sensitive-file naming rules.".to_string());
                    }
                    if let Some(level) = &protected_level {
                        signals.push(format!("Protected Zone policy level: {level}."));
                    }
                    let mut caveats = Vec::new();
                    if footprint_partial {
                        caveats.push(
                            "Footprint is incomplete because this subtree is partial or had a scan error."
                                .to_string(),
                        );
                    }
                    if let Some(error) = &scan_error {
                        caveats.push(format!("Scan status: {error}."));
                    }
                    if classification == "unknown" {
                        caveats.push("Code Hangar cannot classify this relationship.".to_string());
                    }
                    Ok(FolderExplanation {
                        nav_id: row.get(0)?,
                        project_id: row.get(1)?,
                        display_path,
                        display_name,
                        item_kind,
                        classification,
                        confidence,
                        summary,
                        signals,
                        caveats,
                        child_count: row.get(5)?,
                        apparent_bytes: row
                            .get::<_, Option<i64>>(6)?
                            .map(|value| value.max(0) as u64),
                        allocated_bytes: row
                            .get::<_, Option<i64>>(7)?
                            .map(|value| value.max(0) as u64),
                        physical_bytes: row
                            .get::<_, Option<i64>>(8)?
                            .map(|value| value.max(0) as u64),
                        footprint_partial,
                        protected_level,
                        fully_scanned,
                        scan_error,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    /// Build the investigation report for an (ad-hoc) root: the project node id for the
    /// Gate-3 actions, the contents footprint, whether it carries a `.git`, and the
    /// reverse lookup — which registered (non-adhoc) projects contain or sit inside this
    /// folder. The caller fills in the folder explanation.
    pub fn investigation_report(&self, root_id: i64) -> DbResult<FolderInvestigation> {
        self.with_read_conn(|conn| {
            let path: Option<String> = conn
                .query_row(
                    "SELECT path FROM scan_root WHERE id = ?1",
                    params![root_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(path) = path else {
                return Ok(FolderInvestigation {
                    root_id,
                    ..Default::default()
                });
            };
            let root_node_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM node WHERE kind = 'project' AND path = ?1",
                    params![&path],
                    |row| row.get(0),
                )
                .optional()?;

            // Count the folder's files through the project's nav_item rows, not a path-prefix
            // match on node.path. The scanner canonicalizes the walk root (on Windows that
            // yields an extended-length `\\?\C:\…` prefix), so the indexed file nodes do not
            // share scan_root.path's textual prefix and a `LIKE` match silently returned zero.
            // The nav_item.project_id relationship is exactly how every other view counts a
            // project's files, so it is robust to that path-shape difference.
            let (file_count, total_bytes, has_git): (i64, i64, bool) =
                if let Some(node_id) = root_node_id {
                    let (file_count, total_bytes): (i64, i64) = conn.query_row(
                        "SELECT COUNT(*), COALESCE(SUM(COALESCE(n.size_apparent, 0)), 0)
                         FROM nav_item ni
                         JOIN node n ON n.id = ni.node_id
                         WHERE ni.project_id = ?1 AND ni.item_kind = 'file' AND n.present = 1",
                        params![node_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    let has_git: bool = conn.query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM nav_item ni
                             JOIN node n ON n.id = ni.node_id
                             WHERE ni.project_id = ?1 AND n.name = '.git'
                         )",
                        params![node_id],
                        |row| Ok(row.get::<_, i64>(0)? == 1),
                    )?;
                    (file_count, total_bytes, has_git)
                } else {
                    (0, 0, false)
                };

            let mut owners = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT n.name, n.path FROM node n
                     LEFT JOIN scan_root sr ON sr.path = n.path
                     WHERE n.kind = 'project' AND n.present = 1
                       AND COALESCE(sr.adhoc, 0) = 0 AND n.path <> ?1",
                )?;
                let rows = stmt.query_map(params![&path], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (name, project_path) = row?;
                    let relation = if path_is_inside(&path, &project_path) {
                        "inside-project"
                    } else if path_is_inside(&project_path, &path) {
                        "contains-project"
                    } else {
                        continue;
                    };
                    owners.push(InvestigationOwner {
                        relation: relation.to_string(),
                        name,
                        path: display_path_for_path(&project_path),
                    });
                }
            }
            let is_orphan = owners.is_empty();

            Ok(FolderInvestigation {
                root_id,
                root_node_id,
                path: display_path_for_path(&path),
                explanation: None,
                owners,
                is_orphan,
                file_count: file_count.max(0) as u64,
                total_bytes: total_bytes.max(0) as u64,
                has_git,
            })
        })
    }

    pub fn node_path(&self, node_id: i64) -> DbResult<Option<String>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT path FROM node WHERE id = ?1 AND present = 1",
                params![node_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    /// Resolve an AI-assist target only through the local inventory. The caller must
    /// still run the content secret scanner, but it must never accept a raw webview
    /// path: the node has to be a present file in a present registered project.
    /// Available in the Local (mutation) edition too, where the in-app editor resolves a
    /// node to a protected, registered-project file through this before writing it.
    #[cfg(feature = "mutation")]
    pub fn ai_explain_target(&self, node_id: i64) -> DbResult<Option<AiExplainTarget>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT n.path,
                        MAX(ni.is_sensitive),
                        MAX(ni.protected_level),
                        n.is_reparse,
                        n.reparse_kind
                 FROM node n
                 JOIN nav_item ni ON ni.node_id = n.id
                 JOIN node project ON project.id = ni.project_id
                 WHERE n.id = ?1
                   AND n.present = 1
                   AND project.present = 1
                   AND project.kind = 'project'
                   AND ni.item_kind <> 'directory'
                 GROUP BY n.path, n.is_reparse, n.reparse_kind",
                params![node_id],
                |row| {
                    Ok(AiExplainTarget {
                        path: row.get(0)?,
                        is_sensitive: row.get::<_, i64>(1)? != 0,
                        protected_level: row.get(2)?,
                        is_reparse: row.get::<_, i64>(3)? != 0,
                        reparse_kind: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    #[cfg(feature = "mutation")]
    pub fn ai_explain_project_paths(&self, node_id: i64) -> DbResult<Vec<String>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT project.path
                 FROM nav_item ni
                 JOIN node project ON project.id = ni.project_id
                 WHERE ni.node_id = ?1
                   AND project.present = 1
                   AND project.kind = 'project'
                 ORDER BY project.path COLLATE NOCASE",
            )?;
            let rows = stmt.query_map(params![node_id], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(DbError::from)
        })
    }

    pub fn project_context_files(&self, project_id: i64) -> DbResult<Vec<ContextFile>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, node_id, project_id, path, display_name, priority, is_sensitive, protected_level
                 FROM nav_item
                 WHERE project_id = ?1
                   AND (
                     is_context = 1
                     OR lower(path) LIKE '.claude/%.md'
                     OR lower(path) LIKE '%/.claude/%.md'
                     OR lower(path) LIKE 'commands/%.md'
                     OR lower(path) LIKE '%/commands/%.md'
                     OR lower(path) LIKE 'instructions/%.md'
                     OR lower(path) LIKE '%/instructions/%.md'
                   )
                 ORDER BY
                   CASE
                     WHEN lower(path) IN ('readme.md', 'agents.md', 'claude.md', 'gemini.md') THEN 0
                     WHEN lower(path) GLOB '.cursor/rules/*' THEN 4
                     WHEN lower(path) GLOB 'docs/readme.md' OR lower(path) GLOB 'docs/index.md' OR lower(path) GLOB 'docs/overview.md' THEN 8
                     WHEN lower(path) GLOB 'docs/*' THEN 14
                     WHEN lower(path) LIKE '.claude/%.md' OR lower(path) LIKE '%/.claude/%.md' THEN 18
                     WHEN lower(path) GLOB 'prompts/*' THEN 20
                     WHEN lower(path) LIKE 'commands/%.md' OR lower(path) LIKE '%/commands/%.md' THEN 22
                     WHEN lower(path) LIKE 'instructions/%.md' OR lower(path) LIKE '%/instructions/%.md' THEN 23
                     WHEN instr(path, '/') = 0 AND lower(display_name) IN ('package.json', 'pyproject.toml', 'cargo.toml', 'go.mod', 'requirements.txt') THEN 24
                     WHEN lower(display_name) = 'readme.md' THEN 60 + (length(path) - length(replace(path, '/', '')))
                     WHEN lower(path) LIKE '%/docs/%' THEN 72
                     WHEN lower(path) LIKE '%/prompts/%' THEN 78
                     WHEN lower(display_name) IN ('package.json', 'pyproject.toml', 'cargo.toml', 'go.mod', 'requirements.txt') THEN 86
                     ELSE 100 + priority
                   END,
                   CASE WHEN is_sensitive = 1 OR protected_level IS NOT NULL THEN 1 ELSE 0 END,
                   priority,
                   sort_key,
                   id
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project_id, PROJECT_CONTEXT_LIMIT], |row| {
                let path: String = row.get(3)?;
                let display_name: String = row.get(4)?;
                let rank = context_recommendation_rank(&path, &display_name);
                let (context_group, recommendation_reason) =
                    context_recommendation_metadata(&path, &display_name);
                Ok(ContextFile {
                    nav_id: row.get(0)?,
                    node_id: row.get(1)?,
                    project_id: row.get(2)?,
                    path,
                    display_name,
                    priority: row.get(5)?,
                    context_rank: rank,
                    context_group,
                    recommendation_reason,
                    recommended: rank < 60,
                    is_sensitive: row.get::<_, i64>(6)? == 1,
                    protected_level: row.get(7)?,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn file_preview(
        &self,
        node_id: i64,
        mode: PreviewMode,
        record_recent: bool,
    ) -> DbResult<FilePreview> {
        self.file_preview_with_policy(node_id, mode, record_recent, PreviewPolicy::default())
    }

    pub fn file_reveal(&self, node_id: i64, mode: PreviewMode) -> DbResult<FilePreview> {
        self.file_reveal_with_policy(
            node_id,
            mode,
            PreviewPolicy {
                allow_sensitive_reveal: true,
                relax_non_strong_protected_preview: false,
            },
        )
    }

    pub fn file_preview_with_policy(
        &self,
        node_id: i64,
        mode: PreviewMode,
        record_recent: bool,
        policy: PreviewPolicy,
    ) -> DbResult<FilePreview> {
        self.file_preview_internal(node_id, mode, false, record_recent, policy)
    }

    pub fn file_reveal_with_policy(
        &self,
        node_id: i64,
        mode: PreviewMode,
        policy: PreviewPolicy,
    ) -> DbResult<FilePreview> {
        self.file_preview_internal(node_id, mode, true, true, policy)
    }

    fn file_preview_internal(
        &self,
        node_id: i64,
        mode: PreviewMode,
        reveal: bool,
        record_recent: bool,
        policy: PreviewPolicy,
    ) -> DbResult<FilePreview> {
        let Some(record) = self.load_preview_record(node_id)? else {
            return Ok(missing_file_preview(node_id, mode));
        };
        let recent_project_id = record.project_id;
        let recent_item_kind = record.item_kind.clone();
        // A sensitive file only ever reaches a Ready preview through an explicit
        // reveal (the default preview path blocks it). Writing that to the
        // persistent recent_item table would leave a durable trail of which
        // secrets the user opened — exactly the audit trail the "reveal stays
        // transient, never persisted" invariant forbids. So never record a recent
        // item for a sensitive file; non-sensitive previews are unaffected.
        let is_sensitive_reveal = record.is_sensitive;
        let preview = build_file_preview_from_record(node_id, mode, reveal, policy, record);
        if record_recent && preview.state == PreviewState::Ready && !is_sensitive_reveal {
            self.queue_recent_for_preview(node_id, recent_project_id, &recent_item_kind)?;
        }
        Ok(preview)
    }

    fn load_preview_record(&self, node_id: i64) -> DbResult<Option<PreviewRecord>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT n.path, COALESCE(ni.display_path, ni.path), COALESCE(n.name, ni.display_name),
                        n.attributes, ni.project_id, ni.is_sensitive, ni.protected_level,
                        ni.is_markdown, ni.is_context, ni.item_kind, n.size_apparent,
                        n.is_reparse, n.reparse_kind
                 FROM node n
                 JOIN nav_item ni ON ni.node_id = n.id
                 WHERE n.id = ?1
                 LIMIT 1",
                params![node_id],
                |row| {
                    Ok(PreviewRecord {
                        path: row.get(0)?,
                        display_path: row.get(1)?,
                        display_name: row.get(2)?,
                        attributes: row.get(3)?,
                        project_id: row.get(4)?,
                        is_sensitive: row.get::<_, i64>(5)? == 1,
                        protected_level: row.get(6)?,
                        is_markdown: row.get::<_, i64>(7)? == 1,
                        is_context: row.get::<_, i64>(8)? == 1,
                        item_kind: row.get(9)?,
                        size_bytes: row.get::<_, Option<i64>>(10)?.map(|value| value as u64),
                        is_reparse: row.get::<_, i64>(11)? == 1,
                        reparse_kind: row.get(12)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    fn insert_recent_for_preview(
        &self,
        node_id: i64,
        project_id: i64,
        item_kind: &str,
    ) -> DbResult<()> {
        self.with_writer(|conn| insert_recent(conn, node_id, project_id, item_kind))
    }

    fn queue_recent_for_preview(
        &self,
        node_id: i64,
        project_id: i64,
        item_kind: &str,
    ) -> DbResult<()> {
        if self.path.is_none() {
            return self.insert_recent_for_preview(node_id, project_id, item_kind);
        }
        let db = self.clone();
        let item_kind = item_kind.to_string();
        thread::spawn(move || {
            let _ = db.insert_recent_for_preview(node_id, project_id, &item_kind);
        });
        Ok(())
    }

    pub fn quick_open(&self, query: &str, limit: usize) -> DbResult<Vec<QuickOpenResult>> {
        let normalized_query = query.trim().to_ascii_lowercase();
        if normalized_query.len() < 2 {
            return Ok(Vec::new());
        }
        self.with_read_conn(|conn| {
            let tokens = normalized_query.split_whitespace().collect::<Vec<_>>();
            let any_file_match = tokens
                .iter()
                .map(|_| {
                    "(instr(lower(nav_item.display_name), ?) > 0 OR instr(lower(nav_item.path), ?) > 0)"
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            let all_context_match = tokens
                .iter()
                .map(|_| {
                    "(instr(lower(nav_item.display_name), ?) > 0
                      OR instr(lower(nav_item.path), ?) > 0
                      OR instr(lower(project.name), ?) > 0
                      OR instr(lower(project.path), ?) > 0)"
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "SELECT nav_item.node_id, nav_item.project_id, nav_item.display_name,
                        nav_item.path, nav_item.item_kind,
                        COALESCE(project.name, ''), COALESCE(project.path, '')
                 FROM nav_item
                 JOIN node AS project ON project.id = nav_item.project_id
                 WHERE nav_item.node_id IS NOT NULL
                   AND ({any_file_match})
                   AND ({all_context_match})
                 ORDER BY nav_item.priority, nav_item.sort_key, nav_item.id
                 LIMIT 5000"
            );
            let mut search_terms = Vec::with_capacity(tokens.len() * 6);
            for token in &tokens {
                search_terms.extend([*token, *token]);
            }
            for token in &tokens {
                search_terms.extend([*token, *token, *token, *token]);
            }
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(search_terms), quick_open_row(query))?;
            let mut results = collect_rows(rows)?
                .into_iter()
                .filter(|result| result.score > 0)
                .collect::<Vec<_>>();
            results = sort_quick_open(results);
            results.dedup_by_key(|result| result.node_id);
            results.truncate(limit);
            Ok(results)
        })
    }

    pub fn search_documents(&self, query: &str, limit: usize) -> DbResult<Vec<DocumentHit>> {
        Ok(self
            .search_documents_filtered(DocumentSearchOptions {
                query,
                project_id: None,
                indexed_kind: None,
                path_filter: None,
                name_filter: None,
                include_fixture_projects: true,
                limit,
            })?
            .hits)
    }

    pub fn search_documents_filtered(
        &self,
        options: DocumentSearchOptions<'_>,
    ) -> DbResult<DocumentSearchResult> {
        let DocumentSearchOptions {
            query,
            project_id,
            indexed_kind,
            path_filter,
            name_filter,
            include_fixture_projects,
            limit,
        } = options;
        let started = Instant::now();
        let fts_query = fts_query_for(query);
        let normalized_kind = indexed_kind.unwrap_or("all").to_ascii_lowercase();
        let normalized_path_filter = like_filter(path_filter);
        let normalized_name_filter = like_filter(name_filter);
        if fts_query.is_empty() || query.trim().chars().count() < 2 {
            return Ok(DocumentSearchResult {
                hits: Vec::new(),
                truncated: false,
                duration_ms: 0,
            });
        }
        self.with_read_conn(|conn| {
            let unlimited = limit == 0;
            let capped_limit = if unlimited { 0 } else { limit.clamp(1, 500) };
            let fetch_limit = if unlimited {
                -1_i64
            } else {
                capped_limit as i64 + 1
            };
            let mut stmt = conn.prepare(
                "SELECT document_fts.node_id, document_fts.project_id, COALESCE(di.title, ni.display_name), ni.path,
                        snippet(document_fts, -1, '', '', ' ... ', 12)
                 FROM document_fts
                 JOIN nav_item ni ON ni.node_id = document_fts.node_id
                 JOIN node project ON project.id = document_fts.project_id AND project.kind = 'project'
                 LEFT JOIN document_index di ON di.node_id = document_fts.node_id
                 WHERE document_fts MATCH ?1
                   AND ni.is_sensitive = 0
                   AND ni.protected_level IS NULL
                   AND (?3 IS NULL OR document_fts.project_id = ?3)
                   AND (?4 != 'context' OR ni.is_context = 1)
                   AND (?4 != 'markdown' OR ni.is_markdown = 1)
                   AND (?5 IS NULL OR lower(ni.path) LIKE ?5)
                   AND (?6 IS NULL OR lower(COALESCE(di.title, ni.display_name)) LIKE ?6)
                   AND (?7 = 1 OR COALESCE(json_extract(project.attributes, '$.source'), 'fixture') <> 'fixture')
                 ORDER BY bm25(document_fts), ni.priority, ni.sort_key
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![
                    fts_query,
                    fetch_limit,
                    project_id,
                    normalized_kind,
                    normalized_path_filter,
                    normalized_name_filter,
                    include_fixture_projects
                ],
                |row| {
                Ok(DocumentHit {
                    node_id: row.get(0)?,
                    project_id: row.get(1)?,
                    title: row.get(2)?,
                    path: row.get(3)?,
                    snippet: row.get(4)?,
                })
                },
            )?;
            let mut hits = collect_rows(rows)?;
            let truncated = !unlimited && hits.len() > capped_limit;
            if !unlimited {
                hits.truncate(capped_limit);
            }
            Ok(DocumentSearchResult {
                hits,
                truncated,
                duration_ms: started.elapsed().as_millis() as u64,
            })
        })
    }

    pub fn resolve_local_link(
        &self,
        project_id: i64,
        from_node_id: i64,
        target: &str,
    ) -> DbResult<Option<i64>> {
        let Some(resolved_path) = self.with_read_conn(|conn| {
            let from_path: Option<String> = conn
                .query_row(
                    "SELECT path FROM nav_item WHERE project_id = ?1 AND node_id = ?2 LIMIT 1",
                    params![project_id, from_node_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(from_path.and_then(|path| resolve_relative_path(&path, target)))
        })?
        else {
            return Ok(None);
        };

        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT node_id FROM nav_item
                 WHERE project_id = ?1 AND path = ?2 AND node_id IS NOT NULL
                 LIMIT 1",
                params![project_id, resolved_path],
                |row| row.get(0),
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    pub fn node_relationships(&self, node_id: i64) -> DbResult<NodeRelationships> {
        self.with_read_conn(|conn| {
            Ok(NodeRelationships {
                node_id,
                outgoing: load_relationships(conn, node_id, true)?,
                incoming: load_relationships(conn, node_id, false)?,
                issues: load_relationship_issues(conn, node_id)?,
            })
        })
    }

    pub fn project_graph_map(&self, project_id: i64, limit: usize) -> DbResult<GraphMap> {
        self.with_writer(|conn| {
            let key = workflow_graph_setting_key(project_id);
            if setting_value(conn, &key)?.is_none() {
                rebuild_project_workflow_edges(conn, project_id)?;
            }
            Ok(())
        })?;
        self.with_read_conn(|conn| load_project_graph_map(conn, project_id, limit))
    }

    pub fn graph_orphans(&self, limit: usize) -> DbResult<OrphanCandidates> {
        self.orphan_asset_candidates(OrphanAssetSearchOptions {
            min_size_bytes: None,
            project_id: None,
            asset_kind: None,
            min_confidence: None,
            include_partial: false,
            include_fixture_projects: true,
            limit,
        })
    }

    pub fn orphan_asset_candidates(
        &self,
        options: OrphanAssetSearchOptions<'_>,
    ) -> DbResult<OrphanCandidates> {
        self.with_read_conn(|conn| load_orphan_asset_candidates(conn, &options))
    }

    pub fn node_orphan_status(&self, node_id: i64) -> DbResult<OrphanStatus> {
        self.with_read_conn(|conn| load_node_orphan_status(conn, node_id))
    }

    pub fn lost_project_candidates(
        &self,
        options: LostProjectSearchOptions<'_>,
    ) -> DbResult<LostProjectCandidates> {
        self.with_read_conn(|conn| {
            load_lost_project_candidates(
                conn,
                LostProjectLoadOptions {
                    min_size_bytes: options.min_size_bytes.unwrap_or(0),
                    project_id: options.project_id,
                    stale_preset: options.stale_preset.unwrap_or("any"),
                    requested_signals: options.signals,
                    keyword: options.keyword.unwrap_or(""),
                    include_partial: options.include_partial,
                    include_fixture_projects: options.include_fixture_projects,
                    limit: options.limit,
                },
            )
        })
    }

    pub fn duplicate_candidates(&self, limit: usize) -> DbResult<DuplicateCandidates> {
        self.duplicate_candidates_filtered(
            Some(DUPLICATE_MIN_SIZE_BYTES),
            None,
            None,
            None,
            true,
            limit,
        )
    }

    pub fn duplicate_candidates_filtered(
        &self,
        min_size_bytes: Option<u64>,
        project_id: Option<i64>,
        file_kind: Option<&str>,
        current_file_node_id: Option<i64>,
        include_fixture_projects: bool,
        limit: usize,
    ) -> DbResult<DuplicateCandidates> {
        self.with_read_conn(|conn| {
            if let Some(node_id) = current_file_node_id {
                load_duplicate_candidates_for_node(
                    conn,
                    node_id,
                    limit,
                    min_size_bytes.unwrap_or(0),
                    file_kind.unwrap_or("all"),
                    include_fixture_projects,
                )
            } else {
                load_duplicate_candidates(
                    conn,
                    limit,
                    min_size_bytes.unwrap_or(DUPLICATE_MIN_SIZE_BYTES),
                    project_id,
                    file_kind.unwrap_or("all"),
                    include_fixture_projects,
                )
            }
        })
    }

    /// On-demand full-hash confirmation for the duplicate-candidate group that
    /// contains `node_id`. Read-only: it only reads file bytes to hash them and
    /// never writes, deletes, or marks anything for deletion. Returns byte-identical
    /// confirmed groups with reclaimable bytes; partial-hash-only collisions are
    /// dropped. `partial=true` if any candidate failed to hash and was skipped.
    pub fn confirm_duplicate_group(&self, node_id: i64) -> DbResult<DuplicateConfirmation> {
        self.with_read_conn(|conn| confirm_duplicate_group(conn, node_id))
    }

    /// Full-hash confirm with cooperative cancellation + progress reporting (the on-demand job
    /// path). Returns `Ok(None)` when cancelled mid-hash; `progress` is called before the first
    /// hash and after each file. Read-only, exactly like [`Self::confirm_duplicate_group`].
    pub fn confirm_duplicate_group_interruptible(
        &self,
        node_id: i64,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(DuplicateConfirmProgress),
    ) -> DbResult<Option<DuplicateConfirmation>> {
        self.with_read_conn(|conn| {
            confirm_duplicate_group_inner(conn, node_id, cancel, &mut *progress)
        })
    }

    pub fn recent_items_list(&self, limit: usize) -> DbResult<Vec<RecentItem>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT r.node_id, r.project_id, r.item_kind, COALESCE(n.path, ''), r.opened_at
                 FROM recent_item r
                 LEFT JOIN node n ON n.id = r.node_id
                 ORDER BY r.opened_at DESC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| {
                Ok(RecentItem {
                    node_id: row.get(0)?,
                    project_id: row.get(1)?,
                    item_kind: row.get(2)?,
                    path: row.get(3)?,
                    opened_at: row.get(4)?,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn pinned_items_list(&self) -> DbResult<Vec<PinnedItem>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.node_id, p.project_id, p.item_kind, COALESCE(n.path, ''), p.pinned_at
                 FROM pinned_item p
                 LEFT JOIN node n ON n.id = p.node_id
                 ORDER BY p.pinned_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(PinnedItem {
                    node_id: row.get(0)?,
                    project_id: row.get(1)?,
                    item_kind: row.get(2)?,
                    path: row.get(3)?,
                    pinned_at: row.get(4)?,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn pin_item(&self, node_id: i64, item_kind: &str) -> DbResult<()> {
        self.with_conn(|conn| {
            let project_id: Option<i64> = conn
                .query_row(
                    "SELECT project_id FROM nav_item WHERE node_id = ?1 ORDER BY project_id LIMIT 1",
                    params![node_id],
                    |row| row.get(0),
                )
                .optional()?;
            conn.execute(
                "INSERT OR IGNORE INTO pinned_item(node_id, project_id, item_kind, pinned_at)
                 VALUES(?1, ?2, ?3, ?4)",
                params![node_id, project_id, item_kind, now()],
            )?;
            conn.execute(
                "UPDATE nav_item SET pinned = 1 WHERE node_id = ?1",
                params![node_id],
            )?;
            Ok(())
        })
    }

    pub fn unpin_item(&self, node_id: i64, item_kind: &str) -> DbResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM pinned_item WHERE node_id = ?1 AND item_kind = ?2",
                params![node_id, item_kind],
            )?;
            conn.execute(
                "UPDATE nav_item SET pinned = 0 WHERE node_id = ?1",
                params![node_id],
            )?;
            Ok(())
        })
    }

    /// Attach a comment to a project/folder/file node. `author`/`source` default
    /// to "user"; a later phase passes a connected agent identity here. Body is
    /// trimmed and rejected when empty so agents are constrained the same way.
    pub fn comment_add(
        &self,
        node_id: i64,
        body: &str,
        author: &str,
        source: &str,
    ) -> DbResult<Comment> {
        let body = body.trim();
        if body.is_empty() {
            return Err(DbError::FileRead("A comment cannot be empty.".to_string()));
        }
        let body = body.to_string();
        let author = normalize_comment_field(author);
        let source = normalize_comment_field(source);
        // A single transaction so a busy-retry can never re-run a committed INSERT and create a
        // duplicate comment (with_conn re-invokes the whole closure on SQLITE_BUSY/LOCKED; an
        // INSERT-then-read-back without a transaction would duplicate the row).
        self.with_writer(|conn| {
            let tx = conn.transaction()?;
            // A non-"user" add is an AI write: it requires the global AI write-mode
            // toggle. The local user (source "user") is never gated.
            if source != "user" && !comment_write_enabled(&tx)? {
                return Err(DbError::FileRead(
                    "AI write mode is off. Enable it in Settings before an AI app can add comments."
                        .to_string(),
                ));
            }
            // Resolve the owning project: a nav_item points each node at its
            // project; a project node owns itself.
            let project_id: Option<i64> = tx
                .query_row(
                    "SELECT project_id FROM nav_item WHERE node_id = ?1 ORDER BY project_id LIMIT 1",
                    params![node_id],
                    |row| row.get(0),
                )
                .optional()?;
            let project_id = match project_id {
                Some(id) => Some(id),
                None => tx
                    .query_row(
                        "SELECT id FROM node WHERE id = ?1 AND kind = 'project'",
                        params![node_id],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
            let ts = now();
            tx.execute(
                "INSERT INTO comment(node_id, project_id, body, author, source, created_at, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![node_id, project_id, body, author, source, ts],
            )?;
            let comment = read_comment(&tx, tx.last_insert_rowid())?;
            tx.commit()?;
            Ok(comment)
        })
    }

    pub fn comments_for_node(&self, node_id: i64) -> DbResult<Vec<Comment>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, node_id, project_id, body, author, source, created_at, updated_at
                 FROM comment
                 WHERE node_id = ?1 AND deleted_at IS NULL
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![node_id], comment_from_row)?;
            collect_rows(rows)
        })
    }

    pub fn comments_count_for_node(&self, node_id: i64) -> DbResult<i64> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM comment WHERE node_id = ?1 AND deleted_at IS NULL",
                params![node_id],
                |row| row.get(0),
            )
            .map_err(DbError::from)
        })
    }

    /// Resolve the project a live comment belongs to, so a connected AI app's edit
    /// can be re-checked against its current project scope. Returns `None` for an
    /// unknown, already-deleted, or project-less comment.
    pub fn comment_project_id(&self, comment_id: i64) -> DbResult<Option<i64>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT project_id FROM comment WHERE id = ?1 AND deleted_at IS NULL",
                params![comment_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|outer| outer.flatten())
            .map_err(DbError::from)
        })
    }

    /// Read a single comment by id (any state), e.g. to back it up before a
    /// user-approved change. Returns None if it does not exist.
    pub fn comment_get(&self, comment_id: i64) -> DbResult<Option<Comment>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, node_id, project_id, body, author, source, created_at, updated_at
                 FROM comment WHERE id = ?1",
                params![comment_id],
                comment_from_row,
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    /// Record a connected AI app's pending request for an action it cannot perform
    /// itself. The agent only ever inserts a `pending` row; a human resolves it.
    /// File a pending agent request. Comment kinds set `target_comment_id` +
    /// `proposed_body`; other kinds set the generic `target_kind`/`target_id`/
    /// `project_id`/`payload_json` and `cross_scope`. The agent only ever inserts a
    /// 'pending' row here; a human resolves it.
    pub fn agent_request_create(&self, request: &NewAgentRequest) -> DbResult<AgentActionRequest> {
        // Transaction so a busy-retry can't re-run the committed INSERT and duplicate the request.
        self.with_writer(|conn| {
            let tx = conn.transaction()?;
            let ts = now();
            tx.execute(
                "INSERT INTO agent_request(agent_id, agent_name, kind, target_comment_id, proposed_body, detail, target_kind, target_id, project_id, payload_json, cross_scope, status, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', ?12)",
                params![
                    request.agent_id,
                    request.agent_name,
                    request.kind,
                    request.target_comment_id,
                    request.proposed_body,
                    request.detail,
                    request.target_kind,
                    request.target_id,
                    request.project_id,
                    request.payload_json,
                    request.cross_scope,
                    ts
                ],
            )?;
            let created = read_agent_request(&tx, tx.last_insert_rowid())?;
            tx.commit()?;
            Ok(created)
        })
    }

    /// The pending requests awaiting a human decision, newest first, each enriched
    /// with the target comment's present body/source so the reviewer sees context.
    pub fn agent_requests_pending(&self) -> DbResult<Vec<AgentActionRequest>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(AGENT_REQUEST_SELECT_PENDING)?;
            let rows = stmt.query_map([], agent_request_from_row)?;
            collect_rows(rows)
        })
    }

    /// The requests filed by ONE agent, across every status (pending/approved/
    /// rejected/processing), newest first. Strictly scoped to `agent_id` so the
    /// caller can only ever see its OWN requests — never another app's. Backs the
    /// read-only `list_my_requests` tool that closes the total-control request loop.
    pub fn agent_requests_for_agent(&self, agent_id: i64) -> DbResult<Vec<AgentActionRequest>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(AGENT_REQUEST_SELECT_FOR_AGENT)?;
            let rows = stmt.query_map(params![agent_id], agent_request_from_row)?;
            collect_rows(rows)
        })
    }

    pub fn agent_request_get(&self, request_id: i64) -> DbResult<Option<AgentActionRequest>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                AGENT_REQUEST_SELECT_ONE,
                params![request_id],
                agent_request_from_row,
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    /// Mark a request resolved (`approved` or `rejected`). Only a still-`pending`
    /// row transitions, so a double resolve is a no-op (returns false).
    pub fn agent_request_set_status(&self, request_id: i64, status: &str) -> DbResult<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE agent_request SET status = ?2, resolved_at = ?3 WHERE id = ?1 AND status = 'pending'",
                params![request_id, status, now()],
            )?;
            Ok(changed > 0)
        })
    }

    /// Atomic `from -> to` status transition; returns true only if exactly one row
    /// was in `from`. Used to CLAIM a request (pending -> processing) so concurrent
    /// approvals can't both execute, and to finalize (processing -> approved) or
    /// release it (processing -> pending). `resolved_at` is stamped only for terminal
    /// states (anything other than pending/processing).
    pub fn agent_request_transition(
        &self,
        request_id: i64,
        from: &str,
        to: &str,
    ) -> DbResult<bool> {
        self.with_conn(|conn| {
            let resolved_at = if to == "pending" || to == "processing" {
                None
            } else {
                Some(now())
            };
            let changed = conn.execute(
                "UPDATE agent_request SET status = ?3, resolved_at = ?4 WHERE id = ?1 AND status = ?2",
                params![request_id, from, to, resolved_at],
            )?;
            Ok(changed > 0)
        })
    }

    /// Record the outcome of an approved request (e.g. the created backup id) so the
    /// durable, agent-attributed `agent_request` row also links forward to what the
    /// app actually did on the user's behalf.
    pub fn agent_request_set_result(&self, request_id: i64, result_json: &str) -> DbResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_request SET result_json = ?2 WHERE id = ?1",
                params![request_id, result_json],
            )?;
            Ok(())
        })
    }

    /// Edit a comment. `actor` is the caller's identity class ("user" for the local
    /// user via the app, or an agent name via the connected-AI-app server). The human/AI safety
    /// boundary in `guard_comment_actor` refuses any agent edit of a human comment.
    pub fn comment_edit(&self, comment_id: i64, body: &str, actor: &str) -> DbResult<Comment> {
        let body = body.trim();
        if body.is_empty() {
            return Err(DbError::FileRead("A comment cannot be empty.".to_string()));
        }
        let body = body.to_string();
        self.with_conn(|conn| {
            guard_comment_actor(conn, comment_id, actor, "edit")?;
            let changed = conn.execute(
                "UPDATE comment SET body = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
                params![comment_id, body, now()],
            )?;
            if changed == 0 {
                return Err(DbError::FileRead("Comment not found.".to_string()));
            }
            read_comment(conn, comment_id)
        })
    }

    /// Soft-delete a comment. Subject to the same human/AI boundary as `comment_edit`:
    /// an AI app may only delete a comment it wrote itself, never a human record.
    pub fn comment_delete(&self, comment_id: i64, actor: &str) -> DbResult<()> {
        self.with_conn(|conn| {
            guard_comment_actor(conn, comment_id, actor, "delete")?;
            conn.execute(
                "UPDATE comment SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![comment_id, now()],
            )?;
            Ok(())
        })
    }

    /// Whether AI apps are allowed to write comments (default false). The settings
    /// UI flips this behind a strong warning; the connected-app write tools also need it.
    pub fn comment_write_enabled_value(&self) -> DbResult<bool> {
        self.with_read_conn(comment_write_enabled)
    }

    pub fn set_comment_write_enabled(&self, enabled: bool) -> DbResult<()> {
        self.with_conn(|conn| {
            set_setting(
                conn,
                COMMENT_WRITE_ENABLED_KEY,
                if enabled { "1" } else { "0" },
            )
        })
    }

    /// The user's configured AI Assist provider (default: mode `off`, so nothing leaves the
    /// machine until the user configures a provider). The API key is NOT stored here — it lives
    /// only in the OS keychain (see `hangar-ai`).
    pub fn ai_provider_config(&self) -> DbResult<AiProviderConfig> {
        self.with_read_conn(ai_provider_config)
    }

    /// Persist the AI provider configuration (mode/base_url/model/format). Never the key.
    /// All four settings commit atomically in a single transaction so a crash or a busy-retry
    /// can never leave a torn config (e.g. a new mode pointing at the previous endpoint).
    pub fn set_ai_provider_config(&self, config: &AiProviderConfig) -> DbResult<()> {
        self.with_writer(|conn| {
            let tx = conn.transaction()?;
            set_setting(&tx, AI_PROVIDER_MODE_KEY, &config.mode)?;
            set_setting(&tx, AI_PROVIDER_BASE_URL_KEY, &config.base_url)?;
            set_setting(&tx, AI_PROVIDER_MODEL_KEY, &config.model)?;
            set_setting(&tx, AI_PROVIDER_FORMAT_KEY, &config.format)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Personal glossary persistence is explicitly opt-in. The caller supplies
    /// only terms from Code Hangar's canonical seed dictionary; this layer still
    /// bounds every field and stores no source text or path.
    pub fn ai_glossary_enabled_value(&self) -> DbResult<bool> {
        self.with_read_conn(|conn| {
            Ok(setting_value(conn, AI_GLOSSARY_ENABLED_KEY)?.as_deref() == Some("1"))
        })
    }

    pub fn set_ai_glossary_enabled(&self, enabled: bool) -> DbResult<()> {
        self.with_conn(|conn| {
            set_setting(
                conn,
                AI_GLOSSARY_ENABLED_KEY,
                if enabled { "1" } else { "0" },
            )
        })
    }

    pub fn ai_glossary_entries(&self) -> DbResult<Vec<AiGlossaryEntry>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT term, definition, seen_count FROM ai_glossary
                 ORDER BY seen_count DESC, term COLLATE NOCASE ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(AiGlossaryEntry {
                    term: row.get(0)?,
                    definition: row.get(1)?,
                    count: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn ai_glossary_record(&self, term: &str, definition: &str) -> DbResult<AiGlossaryEntry> {
        let term = term.trim();
        let definition = definition.trim();
        if term.is_empty()
            || term.chars().count() > 48
            || definition.is_empty()
            || definition.chars().count() > 240
        {
            return Err(DbError::FileRead(
                "That glossary entry is outside the supported bounds.".to_string(),
            ));
        }
        self.with_writer(|conn| {
            let tx = conn.transaction()?;
            if setting_value(&tx, AI_GLOSSARY_ENABLED_KEY)?.as_deref() != Some("1") {
                return Err(DbError::FileRead(
                    "Personal glossary persistence is off.".to_string(),
                ));
            }
            tx.execute(
                "INSERT INTO ai_glossary(term, definition, seen_count, updated_at)
                 VALUES(?1, ?2, 1, ?3)
                 ON CONFLICT(term) DO UPDATE SET
                   definition = excluded.definition,
                   seen_count = ai_glossary.seen_count + 1,
                   updated_at = excluded.updated_at",
                params![term, definition, now()],
            )?;
            tx.execute(
                "DELETE FROM ai_glossary WHERE term IN (
                   SELECT term FROM ai_glossary
                   ORDER BY seen_count ASC, updated_at ASC
                   LIMIT MAX(0, (SELECT COUNT(*) FROM ai_glossary) - ?1)
                 )",
                params![GLOSSARY_MAX_ENTRIES],
            )?;
            let entry = tx.query_row(
                "SELECT term, definition, seen_count FROM ai_glossary WHERE term = ?1",
                params![term],
                |row| {
                    Ok(AiGlossaryEntry {
                        term: row.get(0)?,
                        definition: row.get(1)?,
                        count: row.get::<_, i64>(2)?.max(0) as u64,
                    })
                },
            )?;
            tx.commit()?;
            Ok(entry)
        })
    }

    pub fn code_annotation_add(
        &self,
        node_id: i64,
        snippet_hash: &str,
        line_start: u64,
        line_end: u64,
        snippet: &str,
        note: &str,
    ) -> DbResult<CodeAnnotation> {
        let note = note.trim();
        if note.is_empty()
            || note.chars().count() > ANNOTATION_MAX_NOTE_CHARS
            || snippet.is_empty()
            || snippet.len() > ANNOTATION_MAX_SNIPPET_BYTES
            || snippet_hash.is_empty()
            || line_start == 0
            || line_end < line_start
        {
            return Err(DbError::FileRead(
                "That anchored note is outside the supported bounds.".to_string(),
            ));
        }
        self.with_writer(|conn| {
            let tx = conn.transaction()?;
            let node_exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM node WHERE id = ?1)",
                params![node_id],
                |row| row.get::<_, i64>(0),
            )? == 1;
            if !node_exists {
                return Err(DbError::FileRead("The file is no longer inventoried.".to_string()));
            }
            let ts = now();
            tx.execute(
                "INSERT INTO code_annotation(
                   node_id, snippet_hash, line_start, line_end, snippet_text, note, created_at, updated_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![node_id, snippet_hash, line_start as i64, line_end as i64, snippet, note, ts],
            )?;
            let id = tx.last_insert_rowid();
            let annotation = tx.query_row(
                "SELECT id, node_id, snippet_hash, line_start, line_end, note, created_at, updated_at
                 FROM code_annotation WHERE id = ?1",
                params![id],
                code_annotation_from_row,
            )?;
            tx.commit()?;
            Ok(annotation)
        })
    }

    pub fn code_annotations_for_node(&self, node_id: i64) -> DbResult<Vec<StoredCodeAnnotation>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, node_id, snippet_hash, line_start, line_end, note,
                        created_at, updated_at, snippet_text
                 FROM code_annotation WHERE node_id = ?1
                 ORDER BY line_start ASC, created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![node_id], |row| {
                Ok(StoredCodeAnnotation {
                    annotation: CodeAnnotation {
                        id: row.get(0)?,
                        node_id: row.get(1)?,
                        snippet_hash: row.get(2)?,
                        line_start: row.get::<_, i64>(3)?.max(0) as u64,
                        line_end: row.get::<_, i64>(4)?.max(0) as u64,
                        note: row.get(5)?,
                        anchor_state: "unchecked".to_string(),
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    },
                    snippet: row.get(8)?,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn code_annotation_delete(&self, annotation_id: i64, node_id: i64) -> DbResult<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "DELETE FROM code_annotation WHERE id = ?1 AND node_id = ?2",
                params![annotation_id, node_id],
            )? == 1)
        })
    }

    /// The "AI total control" tier toggle (default false). Flipped only behind a strong,
    /// accountable warning in the AI-App-Integration settings panel.
    pub fn mcp_full_control_enabled_value(&self) -> DbResult<bool> {
        self.with_read_conn(mcp_full_control_enabled)
    }

    pub fn set_mcp_full_control_enabled(&self, enabled: bool) -> DbResult<()> {
        self.with_conn(|conn| {
            set_setting(conn, MCP_FULL_CONTROL_KEY, if enabled { "1" } else { "0" })
        })
    }

    /// Whether the irreversible "Final remove" action is enabled (default false). The user-facing
    /// opt-in that supersedes the supervised-QA env var. The backend still enforces every Gate-3
    /// safety (verified backup, confirmation token, protected/sensitive refusal) regardless of this
    /// flag — it only controls whether final removal is OFFERED at all.
    pub fn final_remove_enabled_value(&self) -> DbResult<bool> {
        self.with_read_conn(final_remove_enabled)
    }

    pub fn set_final_remove_enabled(&self, enabled: bool) -> DbResult<()> {
        self.with_conn(|conn| {
            set_setting(
                conn,
                FINAL_REMOVE_ENABLED_KEY,
                if enabled { "1" } else { "0" },
            )
        })
    }

    pub fn wsl_scan_enabled_value(&self) -> DbResult<bool> {
        self.with_read_conn(wsl_scan_enabled)
    }

    pub fn set_wsl_scan_enabled(&self, enabled: bool) -> DbResult<()> {
        self.with_conn(|conn| {
            set_setting(conn, WSL_SCAN_ENABLED_KEY, if enabled { "1" } else { "0" })
        })
    }

    /// The connector read-only "panic switch" (default false). When on, EVERY
    /// connector write/mutation is refused — both when an agent tries to file/run one
    /// and when the user tries to approve a queued one — regardless of the other
    /// toggles. Reads still work. A single freeze the user can flip at any time.
    pub fn mcp_read_only_mode_value(&self) -> DbResult<bool> {
        self.with_read_conn(mcp_read_only_mode)
    }

    pub fn set_mcp_read_only_mode(&self, enabled: bool) -> DbResult<()> {
        self.with_conn(|conn| set_setting(conn, MCP_READ_ONLY_KEY, if enabled { "1" } else { "0" }))
    }

    pub fn roots_list(&self) -> DbResult<Vec<ScanRoot>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, path, enabled, last_scanned_at FROM scan_root WHERE adhoc = 0 ORDER BY path COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ScanRoot {
                    id: row.get(0)?,
                    path: display_path_for_path(&row.get::<_, String>(1)?),
                    enabled: row.get::<_, i64>(2)? == 1,
                    last_scanned_at: row.get(3)?,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn node_watch_fingerprint(&self, node_id: i64) -> DbResult<Option<NodeWatchFingerprint>> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT n.id, ni.project_id, COALESCE(n.path, ''),
                        COALESCE(ni.display_name, n.name, n.path, ''),
                        COALESCE(ni.is_markdown, 0), COALESCE(ni.is_context, 0),
                        n.mtime, n.size_apparent
                 FROM node n
                 LEFT JOIN nav_item ni ON ni.node_id = n.id
                 WHERE n.id = ?1
                 ORDER BY ni.id
                 LIMIT 1",
                params![node_id],
                |row| {
                    let stored_size = row
                        .get::<_, Option<i64>>(7)?
                        .and_then(|value| (value >= 0).then_some(value as u64));
                    Ok(NodeWatchFingerprint {
                        node_id: row.get(0)?,
                        project_id: row.get(1)?,
                        path: row.get(2)?,
                        display_name: row.get(3)?,
                        is_markdown: row.get::<_, i64>(4)? == 1,
                        is_context: row.get::<_, i64>(5)? == 1,
                        stored_mtime: row.get(6)?,
                        stored_size,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    pub fn project_context_watch_fingerprints(
        &self,
        project_id: i64,
        limit: i64,
    ) -> DbResult<Vec<NodeWatchFingerprint>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT n.id, ni.project_id, COALESCE(n.path, ''),
                        COALESCE(ni.display_name, n.name, n.path, ''),
                        COALESCE(ni.is_markdown, 0), COALESCE(ni.is_context, 0),
                        n.mtime, n.size_apparent
                 FROM nav_item ni
                 JOIN node n ON n.id = ni.node_id
                 WHERE ni.project_id = ?1
                   AND ni.node_id IS NOT NULL
                   AND (ni.is_context = 1 OR ni.is_markdown = 1)
                 ORDER BY ni.priority, ni.sort_key, ni.id
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project_id, limit.max(0)], |row| {
                let stored_size = row
                    .get::<_, Option<i64>>(7)?
                    .and_then(|value| (value >= 0).then_some(value as u64));
                Ok(NodeWatchFingerprint {
                    node_id: row.get(0)?,
                    project_id: row.get(1)?,
                    path: row.get(2)?,
                    display_name: row.get(3)?,
                    is_markdown: row.get::<_, i64>(4)? == 1,
                    is_context: row.get::<_, i64>(5)? == 1,
                    stored_mtime: row.get(6)?,
                    stored_size,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn roots_add(&self, path: &str) -> DbResult<ScanRoot> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO scan_root(path, enabled) VALUES(?1, 1)",
                params![path],
            )?;
            upsert_project(conn, path, &display_name_for_path(path), "scan")?;
            conn.query_row(
                "SELECT id, path, enabled, last_scanned_at FROM scan_root WHERE path = ?1",
                params![path],
                |row| {
                    Ok(ScanRoot {
                        id: row.get(0)?,
                        path: display_path_for_path(&row.get::<_, String>(1)?),
                        enabled: row.get::<_, i64>(2)? == 1,
                        last_scanned_at: row.get(3)?,
                    })
                },
            )
            .map_err(DbError::from)
        })
    }

    /// Add a folder to investigate without it showing up as one of your projects. It is a
    /// normal indexed project INTERNALLY (so scan, folder explanation, the Gate-3
    /// backup/move/delete pipeline and discard all work unchanged), but its scan_root is
    /// flagged `adhoc = 1`, which excludes it from the projects list, discovery and the
    /// scan-root settings. If the path is already a registered (non-adhoc) root it stays
    /// registered — the investigation report then surfaces that it is already known.
    pub fn roots_add_adhoc(&self, path: &str) -> DbResult<ScanRoot> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO scan_root(path, enabled, adhoc) VALUES(?1, 1, 1)",
                params![path],
            )?;
            upsert_project(conn, path, &display_name_for_path(path), "investigate")?;
            conn.query_row(
                "SELECT id, path, enabled, last_scanned_at FROM scan_root WHERE path = ?1",
                params![path],
                |row| {
                    Ok(ScanRoot {
                        id: row.get(0)?,
                        path: display_path_for_path(&row.get::<_, String>(1)?),
                        enabled: row.get::<_, i64>(2)? == 1,
                        last_scanned_at: row.get(3)?,
                    })
                },
            )
            .map_err(DbError::from)
        })
    }

    /// True if the scan root is an ad-hoc investigation root (not a registered project).
    pub fn root_is_adhoc(&self, root_id: i64) -> DbResult<bool> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT adhoc FROM scan_root WHERE id = ?1",
                params![root_id],
                |row| Ok(row.get::<_, i64>(0)? == 1),
            )
            .optional()
            .map(|found| found.unwrap_or(false))
            .map_err(DbError::from)
        })
    }

    pub fn roots_set_enabled(&self, root_id: i64, enabled: bool) -> DbResult<ScanRoot> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE scan_root SET enabled = ?2 WHERE id = ?1",
                params![root_id, bool_to_i64(enabled)],
            )?;
            conn.query_row(
                "SELECT id, path, enabled, last_scanned_at FROM scan_root WHERE id = ?1",
                params![root_id],
                |row| {
                    Ok(ScanRoot {
                        id: row.get(0)?,
                        path: display_path_for_path(&row.get::<_, String>(1)?),
                        enabled: row.get::<_, i64>(2)? == 1,
                        last_scanned_at: row.get(3)?,
                    })
                },
            )
            .map_err(DbError::from)
        })
    }

    pub fn roots_unregister(&self, root_id: i64) -> DbResult<()> {
        self.with_writer(|conn| {
            let path: Option<String> = conn
                .query_row(
                    "SELECT path FROM scan_root WHERE id = ?1",
                    params![root_id],
                    |row| row.get(0),
                )
                .optional()?;
            conn.execute("DELETE FROM scan_root WHERE id = ?1", params![root_id])?;
            if let Some(path) = path {
                if let Some(project_id) = conn
                    .query_row(
                        "SELECT id FROM node WHERE kind = 'project' AND path = ?1",
                        params![path],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                {
                    let mut node_ids = {
                        let mut stmt = conn.prepare(
                            "SELECT node_id FROM nav_item
                             WHERE project_id = ?1 AND node_id IS NOT NULL",
                        )?;
                        let rows =
                            stmt.query_map(params![project_id], |row| row.get::<_, i64>(0))?;
                        collect_rows(rows)?
                    };
                    node_ids.push(project_id);
                    node_ids.sort_unstable();
                    node_ids.dedup();

                    conn.execute(
                        "DELETE FROM document_fts WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    conn.execute(
                        "DELETE FROM document_index WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    conn.execute(
                        "DELETE FROM recent_item WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    conn.execute(
                        "DELETE FROM pinned_item WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    conn.execute(
                        "DELETE FROM git_repo WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    delete_by_node_ids(conn, "duplicate_member", "node_id", &node_ids)?;
                    delete_by_node_ids(conn, "edge", "src", &node_ids)?;
                    delete_by_node_ids(conn, "edge", "dst", &node_ids)?;
                    delete_by_node_ids(conn, "relationship_issue", "node_id", &node_ids)?;
                    conn.execute(
                        "DELETE FROM relationship_issue WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    conn.execute(
                        "UPDATE nav_item SET parent_nav_id = NULL WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    conn.execute(
                        "DELETE FROM nav_item WHERE project_id = ?1",
                        params![project_id],
                    )?;
                    // Delete scan_cache rows BEFORE the node rows they reference. scan_cache.node_id
                    // REFERENCES node(id) and foreign_keys=ON, so deleting the node rows first
                    // aborts the whole unregister/discard with a FK violation on any folder that
                    // has real (identity-bearing) files — the common case. Clear by node_id (the
                    // referencing rows) AND by path (descendant rows) up front, then delete the
                    // now-unreferenced node rows.
                    delete_by_node_ids(conn, "scan_cache", "node_id", &node_ids)?;
                    conn.execute(
                        "DELETE FROM scan_cache
                         WHERE path = ?1 OR path LIKE ?2 OR path LIKE ?3",
                        params![path, format!("{path}\\%"), format!("{path}/%")],
                    )?;
                    delete_unreferenced_node_ids(conn, &node_ids)?;
                }
            }
            Ok(())
        })
    }

    /// Remove a project's local inventory by its project id, even when no
    /// scan_root remains (an "orphaned" project left behind by an older,
    /// partially-failed unregister). Files on disk are never touched.
    pub fn project_unregister(&self, project_id: i64) -> DbResult<()> {
        self.with_writer(|conn| {
            let path: Option<String> = conn
                .query_row(
                    "SELECT path FROM node WHERE id = ?1 AND kind = 'project'",
                    params![project_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(ref p) = path {
                conn.execute("DELETE FROM scan_root WHERE path = ?1", params![p])?;
            }
            let mut node_ids = {
                let mut stmt = conn.prepare(
                    "SELECT node_id FROM nav_item
                     WHERE project_id = ?1 AND node_id IS NOT NULL",
                )?;
                let rows = stmt.query_map(params![project_id], |row| row.get::<_, i64>(0))?;
                collect_rows(rows)?
            };
            node_ids.push(project_id);
            node_ids.sort_unstable();
            node_ids.dedup();

            conn.execute(
                "DELETE FROM document_fts WHERE project_id = ?1",
                params![project_id],
            )?;
            conn.execute(
                "DELETE FROM document_index WHERE project_id = ?1",
                params![project_id],
            )?;
            conn.execute(
                "DELETE FROM recent_item WHERE project_id = ?1",
                params![project_id],
            )?;
            conn.execute(
                "DELETE FROM pinned_item WHERE project_id = ?1",
                params![project_id],
            )?;
            conn.execute(
                "DELETE FROM git_repo WHERE project_id = ?1",
                params![project_id],
            )?;
            delete_by_node_ids(conn, "duplicate_member", "node_id", &node_ids)?;
            delete_by_node_ids(conn, "edge", "src", &node_ids)?;
            delete_by_node_ids(conn, "edge", "dst", &node_ids)?;
            delete_by_node_ids(conn, "relationship_issue", "node_id", &node_ids)?;
            conn.execute(
                "DELETE FROM relationship_issue WHERE project_id = ?1",
                params![project_id],
            )?;
            conn.execute(
                "UPDATE nav_item SET parent_nav_id = NULL WHERE project_id = ?1",
                params![project_id],
            )?;
            conn.execute(
                "DELETE FROM nav_item WHERE project_id = ?1",
                params![project_id],
            )?;
            // Delete scan_cache (whose node_id REFERENCES node) BEFORE the node rows, by node_id
            // and by path, or the node DELETE aborts with a foreign-key violation on any folder
            // with real identity-bearing files (see roots_unregister).
            delete_by_node_ids(conn, "scan_cache", "node_id", &node_ids)?;
            if let Some(ref p) = path {
                conn.execute(
                    "DELETE FROM scan_cache
                     WHERE path = ?1 OR path LIKE ?2 OR path LIKE ?3",
                    params![p, format!("{p}\\%"), format!("{p}/%")],
                )?;
            }
            delete_unreferenced_node_ids(conn, &node_ids)?;
            Ok(())
        })
    }

    /// Unregister every scan root and every real (non-demo) project, *freeing the
    /// disk space they used*. Built-in demo/fixture projects are restored fresh.
    /// Project files on disk are never touched. Returns how many real projects
    /// were removed.
    ///
    /// This backs Settings ▸ "Reset all". For the real, file-backed database it
    /// deletes and recreates the whole SQLCipher file: this is instant regardless
    /// of size (no slow, page-zeroing row deletes) and — the whole point of a tool
    /// that reclaims disk — it actually returns the space instead of leaving
    /// hidden rows behind. The in-memory test database uses a scoped delete that
    /// keeps its demo rows.
    /// Reclaim the disk space a large re-scan freed inside the database file.
    ///
    /// SQLite (and SQLCipher) release pages back to a free-list when rows are deleted but never
    /// shrink the file on their own, so after the build-dir skip removes ~10^5 rows the encrypted
    /// inventory can stay >1 GB on disk and keep cold reads slow. `VACUUM` rewrites the database
    /// compactly (re-encrypting transparently); the bracketing `wal_checkpoint(TRUNCATE)` folds in
    /// and then resets the WAL so the whole on-disk footprint actually shrinks. Runs on the single
    /// persistent write connection, which holds no open transaction between calls, so `VACUUM`
    /// (which forbids an active transaction) is legal here.
    pub fn compact(&self) -> DbResult<()> {
        self.with_conn(|conn| {
            conn.execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )?;
            Ok(())
        })
    }

    pub fn reset_local_inventory(&self) -> DbResult<u64> {
        const REAL_PROJECT_FILTER: &str = "kind = 'project' \
             AND COALESCE(json_extract(attributes, '$.source'), 'fixture') <> 'fixture'";
        const FIXTURE_PROJECT_IDS: &str = "SELECT id FROM node WHERE kind = 'project' \
             AND COALESCE(json_extract(attributes, '$.source'), 'fixture') = 'fixture'";

        // Count on a fresh read connection rather than the shared persistent one:
        // a reset is triggered while the rest of the app is live, and blocking on
        // the persistent connection's mutex (held by a watcher or dashboard query)
        // would stall the whole reset before it does any work.
        let removed: i64 = self.with_read_conn(|conn| {
            Ok(conn.query_row(
                &format!("SELECT COUNT(*) FROM node WHERE {REAL_PROJECT_FILTER} AND present = 1"),
                [],
                |row| row.get(0),
            )?)
        })?;

        if let Some(path) = self.path.as_deref() {
            // File-backed: clearing a multi-gigabyte, encrypted full-text index in
            // place is unavoidably slow — both DROP and row-by-row DELETE have to
            // walk and decrypt the whole index b-tree, which crawls for minutes on
            // a real inventory. Deleting the database *file* is O(1) instead, but
            // Windows keeps the handle locked while any connection is open. So
            // schedule the wipe: drop a sentinel and let the next startup delete
            // the file before any connection opens (see [`wipe_pending_reset`]).
            // The caller restarts to apply it immediately. Project files on disk
            // are never touched and the demo projects return fresh.
            fs::write(reset_sentinel_path(path), b"reset-pending")
                .map_err(|err| DbError::FileRead(format!("Failed to schedule reset: {err}")))?;
        } else {
            // In-memory (tests): there is no file to wipe, so scope-delete in
            // place, keeping the demo/fixture rows.
            self.with_writer(|conn| {
                let tx = conn.transaction()?;
                tx.execute("DELETE FROM scan_root", [])?;
                for table in [
                    "document_fts",
                    "document_index",
                    "recent_item",
                    "pinned_item",
                    "git_repo",
                    "relationship_issue",
                    "nav_item",
                ] {
                    tx.execute(
                        &format!(
                            "DELETE FROM {table} WHERE project_id NOT IN ({FIXTURE_PROJECT_IDS})"
                        ),
                        [],
                    )?;
                }
                tx.execute(
                    "DELETE FROM node
                     WHERE kind <> 'project'
                       AND NOT EXISTS (SELECT 1 FROM nav_item WHERE nav_item.node_id = node.id)",
                    [],
                )?;
                tx.execute(&format!("DELETE FROM node WHERE {REAL_PROJECT_FILTER}"), [])?;
                tx.execute(
                    "DELETE FROM edge
                     WHERE src NOT IN (SELECT id FROM node) OR dst NOT IN (SELECT id FROM node)",
                    [],
                )?;
                tx.execute(
                    "DELETE FROM duplicate_member WHERE node_id NOT IN (SELECT id FROM node)",
                    [],
                )?;
                tx.execute("DELETE FROM scan_cache", [])?;
                tx.commit()?;
                Ok(())
            })?;
        }

        Ok(removed.max(0) as u64)
    }

    pub fn root_paths_for_ids(&self, root_ids: &[i64]) -> DbResult<Vec<String>> {
        Ok(self
            .scan_targets_for_ids(root_ids)?
            .into_iter()
            .map(|target| target.raw_path)
            .collect())
    }

    pub fn scan_targets_for_ids(&self, root_ids: &[i64]) -> DbResult<Vec<ScanTarget>> {
        self.with_read_conn(|conn| {
            if root_ids.is_empty() {
                let mut stmt = conn.prepare(
                    "SELECT id, path FROM scan_root WHERE enabled = 1 AND adhoc = 0 ORDER BY path",
                )?;
                let rows = stmt.query_map([], |row| {
                    let raw_path: String = row.get(1)?;
                    Ok(ScanTarget {
                        root_id: row.get(0)?,
                        display_path: display_path_for_path(&raw_path),
                        raw_path,
                    })
                })?;
                return collect_rows(rows);
            }

            let mut targets = Vec::new();
            for id in root_ids {
                if let Some((root_id, raw_path)) = conn
                    .query_row(
                        "SELECT id, path FROM scan_root WHERE id = ?1 AND enabled = 1",
                        params![id],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?
                {
                    targets.push(ScanTarget {
                        root_id,
                        display_path: display_path_for_path(&raw_path),
                        raw_path,
                    });
                }
            }
            Ok(targets)
        })
    }

    pub fn scan_estimate_for_roots(&self, root_ids: &[i64]) -> DbResult<Option<u64>> {
        self.with_read_conn(|conn| {
            let mut total = 0_u64;
            for root_id in root_ids {
                let count = conn.query_row(
                    "SELECT COUNT(*)
                     FROM nav_item ni
                     JOIN node project ON project.id = ni.project_id
                     LEFT JOIN node item ON item.id = ni.node_id
                     WHERE project.kind = 'project'
                       AND project.path = (SELECT path FROM scan_root WHERE id = ?1)
                       AND (ni.node_id IS NULL OR item.present = 1)",
                    params![root_id],
                    |row| row.get::<_, i64>(0),
                )?;
                total = total.saturating_add(count.max(0) as u64);
            }
            Ok((total > 0).then_some(total))
        })
    }

    pub fn complete_scan_estimate_for_roots(&self, root_ids: &[i64]) -> DbResult<Option<u64>> {
        if root_ids.is_empty() {
            return Ok(None);
        }
        self.with_read_conn(|conn| {
            let mut total = 0_u64;
            for root_id in root_ids {
                let count = conn.query_row(
                    "SELECT COUNT(*)
                     FROM nav_item ni
                     JOIN node project ON project.id = ni.project_id
                     LEFT JOIN node item ON item.id = ni.node_id
                     WHERE project.kind = 'project'
                       AND project.path = (SELECT path FROM scan_root WHERE id = ?1)
                       AND (ni.node_id IS NULL OR item.present = 1)",
                    params![root_id],
                    |row| row.get::<_, i64>(0),
                )?;
                if count <= 0 {
                    return Ok(None);
                }
                total = total.saturating_add(count as u64);
            }
            Ok(Some(total))
        })
    }

    pub fn root_is_enabled(&self, root_id: i64) -> DbResult<bool> {
        self.with_read_conn(|conn| root_enabled(conn, root_id))
    }

    pub fn subtree_scan_target(&self, nav_id: i64) -> DbResult<SubtreeScanTarget> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT ni.id, ni.project_id, ni.path, n.path, p.path, sr.id
                 FROM nav_item ni
                 JOIN node n ON n.id = ni.node_id
                 JOIN node p ON p.id = ni.project_id
                 JOIN scan_root sr ON sr.path = p.path
                 WHERE ni.id = ?1 AND ni.item_kind = 'directory' AND sr.enabled = 1",
                params![nav_id],
                |row| {
                    let root_path: String = row.get(4)?;
                    Ok(SubtreeScanTarget {
                        nav_id: row.get(0)?,
                        project_id: row.get(1)?,
                        relative_path: row.get(2)?,
                        absolute_path: row.get(3)?,
                        display_root_path: display_path_for_path(&root_path),
                        root_path,
                        root_id: row.get(5)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                DbError::FileRead("Directory is not inside an enabled scan root.".to_string())
            })
        })
    }

    pub fn scan_estimate_for_subtree(&self, nav_id: i64) -> DbResult<Option<u64>> {
        self.with_read_conn(|conn| {
            let count = conn.query_row(
                "WITH RECURSIVE subtree(id, node_id) AS (
                   SELECT id, node_id FROM nav_item WHERE id = ?1
                   UNION ALL
                   SELECT child.id, child.node_id
                   FROM nav_item child
                   JOIN subtree parent ON child.parent_nav_id = parent.id
                 )
                 SELECT COUNT(*)
                 FROM subtree
                 LEFT JOIN node item ON item.id = subtree.node_id
                 WHERE subtree.node_id IS NULL OR item.present = 1",
                params![nav_id],
                |row| row.get::<_, i64>(0),
            )?;
            Ok((count > 0).then_some(count as u64))
        })
    }

    pub fn load_scanned_root(
        &self,
        root_path: &str,
        files: &[ScannedFile],
        git: Option<&GitRepoSummary>,
    ) -> DbResult<(u64, u64)> {
        self.with_writer(|conn| {
            let project_id =
                upsert_project(conn, root_path, &display_name_for_path(root_path), "scan")?;
            rebuild_project_nav(conn, project_id)?;
            for chunk in files.chunks(2_000) {
                let tx = conn.transaction()?;
                insert_files_for_project(&tx, project_id, chunk)?;
                tx.commit()?;
            }
            insert_git_metadata(conn, project_id, git)?;
            recalculate_child_counts(conn, project_id)?;
            recalculate_nav_aggregates(conn, project_id)?;
            rebuild_project_markdown_edges(conn, project_id)?;
            rebuild_project_workflow_edges(conn, project_id)?;
            conn.execute(
                "UPDATE scan_root SET last_scanned_at = ?2 WHERE path = ?1",
                params![root_path, now()],
            )?;
            let indexed = files.iter().filter(|file| file.body.is_some()).count() as u64;
            Ok((files.len() as u64, indexed))
        })
    }

    pub fn zones_list(&self) -> DbResult<Vec<hangar_core::ProtectedZone>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, pattern_type, pattern, level, source FROM protected_zone ORDER BY id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(hangar_core::ProtectedZone {
                    id: row.get(0)?,
                    pattern_type: row.get(1)?,
                    pattern: row.get(2)?,
                    level: row.get(3)?,
                    source: row.get(4)?,
                })
            })?;
            collect_rows(rows)
        })
    }

    pub fn project_git_status(&self, project_id: i64) -> DbResult<GitRepoSummary> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT project_id, current_branch, head_ref, origin_url, metadata_error
                 FROM git_repo WHERE project_id = ?1",
                params![project_id],
                |row| {
                    Ok(GitRepoSummary {
                        project_id: row.get(0)?,
                        has_git: true,
                        current_branch: row.get(1)?,
                        head_ref: row.get(2)?,
                        origin_url: row.get(3)?,
                        metadata_error: row.get(4)?,
                    })
                },
            )
            .optional()
            .map(|value| {
                value.unwrap_or(GitRepoSummary {
                    project_id,
                    has_git: false,
                    current_branch: None,
                    head_ref: None,
                    origin_url: None,
                    metadata_error: None,
                })
            })
            .map_err(DbError::from)
        })
    }

    pub fn dashboard_summary(&self) -> DbResult<DashboardSummary> {
        self.dashboard_summary_filtered(true)
    }

    pub fn dashboard_summary_filtered(
        &self,
        include_fixture_projects: bool,
    ) -> DbResult<DashboardSummary> {
        self.with_read_conn(|conn| {
            let include_fixture_projects = include_fixture_projects as i64;
            let total_projects = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM node p
                 WHERE p.kind = 'project'
                   AND p.present = 1
                   AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
                include_fixture_projects,
            )?;
            let total_items = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM nav_item ni
                 JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
                 WHERE ?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture'",
                include_fixture_projects,
            )?;
            let context_files = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM nav_item ni
                 JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
                 WHERE ni.is_context = 1
                   AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
                include_fixture_projects,
            )?;
            let indexed_documents = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM document_index di
                 JOIN node p ON p.id = di.project_id AND p.kind = 'project'
                 WHERE ?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture'",
                include_fixture_projects,
            )?;
            let non_indexed_items = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM nav_item ni
                 JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
                 WHERE ni.node_id IS NOT NULL
                   AND ni.item_kind <> 'directory'
                   AND ni.node_id NOT IN (SELECT node_id FROM document_index)
                   AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
                include_fixture_projects,
            )?;
            let partial_items = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM nav_item ni
                 JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
                 WHERE (ni.fully_scanned = 0 OR ni.scan_error IS NOT NULL)
                   AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
                include_fixture_projects,
            )?;
            let git_projects = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM git_repo gr
                 JOIN node p ON p.id = gr.project_id AND p.kind = 'project'
                 WHERE ?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture'",
                include_fixture_projects,
            )?;
            let sensitive_files = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM nav_item ni
                 JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
                 WHERE ni.is_sensitive = 1
                   AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
                include_fixture_projects,
            )?;
            let protected_files = count_i64_with_visibility(
                conn,
                "SELECT COUNT(*)
                 FROM nav_item ni
                 JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
                 WHERE ni.protected_level IS NOT NULL
                   AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
                include_fixture_projects,
            )?;
            let scan_roots = count_i64(
                conn,
                "SELECT COUNT(*) FROM scan_root WHERE enabled = 1 AND adhoc = 0",
            )?;

            Ok(DashboardSummary {
                total_projects: total_projects as u64,
                total_items: total_items as u64,
                context_files: context_files as u64,
                indexed_documents: indexed_documents as u64,
                non_indexed_items: non_indexed_items as u64,
                partial_items: partial_items as u64,
                git_projects: git_projects as u64,
                sensitive_files: sensitive_files as u64,
                protected_files: protected_files as u64,
                scan_roots: scan_roots as u64,
                largest_projects: project_footprint_summaries(
                    conn,
                    5,
                    include_fixture_projects != 0,
                )?,
                stale_or_dirty: stale_disk_summary(conn)?,
                adapters_needing_review: count_i64(
                    conn,
                    "SELECT COUNT(*) FROM adapter WHERE enabled = 0",
                )? as u64,
            })
        })
    }

    pub fn project_recoverable_summary(&self, project_id: i64) -> DbResult<RecoverableSummary> {
        self.with_read_conn(|conn| {
            hangar_accounting::project_recoverable_summary(conn, project_id).map_err(DbError::from)
        })
    }

    pub fn node_recoverable_summary(&self, node_id: i64) -> DbResult<RecoverableSummary> {
        self.with_read_conn(|conn| {
            hangar_accounting::node_recoverable_summary(conn, node_id).map_err(DbError::from)
        })
    }

    pub fn operation_plan_build(
        &self,
        target_node_id: i64,
        action_label: &str,
    ) -> DbResult<OperationPlan> {
        self.with_read_conn(|conn| {
            hangar_plan::build_operation_plan(conn, target_node_id, action_label)
                .map_err(plan_error_to_db_error)
        })
    }

    pub fn operation_plan_build_interruptible(
        &self,
        target_node_id: i64,
        action_label: &str,
        cancel: Arc<AtomicBool>,
    ) -> DbResult<OperationPlan> {
        if cancel.load(Ordering::Relaxed) {
            return Err(DbError::FileRead("Cancelled".to_string()));
        }
        if let Some(path) = &self.path {
            let cipher_key = self.cipher_key.as_deref().ok_or_else(|| {
                DbError::FileRead("File-backed database is missing its SQLCipher key.".to_string())
            })?;
            let conn = Connection::open(path.as_ref())?;
            configure_file_read_connection(&conn, cipher_key)?;
            let plan_cancel = Arc::clone(&cancel);
            return run_interruptible_read(&conn, cancel, |conn| {
                hangar_plan::build_operation_plan_with_cancel(
                    conn,
                    target_node_id,
                    action_label,
                    plan_cancel.as_ref(),
                )
                .map_err(plan_error_to_db_error)
            });
        }

        self.with_conn(|conn| {
            if cancel.load(Ordering::Relaxed) {
                return Err(DbError::FileRead("Cancelled".to_string()));
            }
            hangar_plan::build_operation_plan_with_cancel(
                conn,
                target_node_id,
                action_label,
                cancel.as_ref(),
            )
            .map_err(plan_error_to_db_error)
        })
    }

    pub fn risk_report_build(&self, plan: &OperationPlan) -> DbResult<RiskReport> {
        Ok(hangar_plan::build_risk_report(plan))
    }

    pub fn risk_report_build_for_target(
        &self,
        target_node_id: i64,
        action_label: &str,
    ) -> DbResult<RiskReport> {
        self.with_read_conn(|conn| {
            hangar_plan::build_risk_report_for_target(conn, target_node_id, action_label)
                .map_err(plan_error_to_db_error)
        })
    }

    pub fn adapters_list(&self) -> DbResult<Vec<AdapterSummary>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, version, type, source, enabled, definition_json
                 FROM adapter
                 ORDER BY source, name",
            )?;
            let rows = stmt.query_map([], |row| {
                let definition_json = row.get::<_, String>(6)?;
                Ok(AdapterSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    version: row.get(2)?,
                    adapter_type: row.get(3)?,
                    source: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? == 1,
                    description: adapter_description(&definition_json),
                })
            })?;
            collect_rows(rows)
        })
    }
}

impl DbWriteSession {
    pub fn root_is_enabled(&self, root_id: i64) -> DbResult<bool> {
        root_enabled(&self.conn, root_id)
    }

    pub fn begin_root_scan(&mut self, root_path: &str) -> DbResult<i64> {
        let project_id = upsert_project(
            &self.conn,
            root_path,
            &display_name_for_path(root_path),
            "scan",
        )?;
        // Mark the root needs-scan (last_scanned_at = NULL) for the duration of the rebuild.
        // rebuild_project_nav below wipes the project's existing index; if the process crashes
        // between here and finish_root_scan (which restores a real timestamp only on success),
        // the watcher must report needs_scan rather than "clean" over a truncated index. The
        // running scan job, not last_scanned_at, drives the in-progress UI meanwhile.
        self.conn.execute(
            "UPDATE scan_root SET last_scanned_at = NULL WHERE path = ?1",
            params![root_path],
        )?;
        self.conn.execute(
            "DELETE FROM setting WHERE key = ?1",
            params![workflow_graph_setting_key(project_id)],
        )?;
        rebuild_project_nav(&self.conn, project_id)?;
        Ok(project_id)
    }

    pub fn persist_batch(
        &mut self,
        project_id: i64,
        files: &[ScannedFile],
    ) -> DbResult<(u64, u64)> {
        let tx = self.conn.transaction()?;
        insert_files_for_project(&tx, project_id, files)?;
        tx.commit()?;
        let indexed = files.iter().filter(|file| file.body.is_some()).count() as u64;
        Ok((files.len() as u64, indexed))
    }

    pub fn finish_root_scan(
        &mut self,
        project_id: i64,
        root_path: &str,
        git: Option<&GitRepoSummary>,
    ) -> DbResult<()> {
        self.finish_root_scan_with_progress(project_id, root_path, git, true, |_| {})
    }

    pub fn finish_root_scan_with_progress(
        &mut self,
        project_id: i64,
        root_path: &str,
        git: Option<&GitRepoSummary>,
        scan_completed: bool,
        mut on_step: impl FnMut(&str),
    ) -> DbResult<()> {
        let mut ignore_timing = |_| {};
        self.finish_root_scan_with_cancel(
            project_id,
            RootScanFinish {
                root_path,
                git,
                scan_completed,
                cancel: None,
            },
            &mut on_step,
            &mut ignore_timing,
        )
    }

    pub fn finish_root_scan_interruptible_with_progress(
        &mut self,
        project_id: i64,
        root_path: &str,
        git: Option<&GitRepoSummary>,
        scan_completed: bool,
        cancel: Arc<AtomicBool>,
        mut on_step: impl FnMut(&str),
    ) -> DbResult<()> {
        let mut ignore_timing = |_| {};
        self.finish_root_scan_with_cancel(
            project_id,
            RootScanFinish {
                root_path,
                git,
                scan_completed,
                cancel: Some(cancel),
            },
            &mut on_step,
            &mut ignore_timing,
        )
    }

    pub fn finish_root_scan_interruptible_with_progress_and_timing(
        &mut self,
        project_id: i64,
        options: RootScanFinish<'_>,
        mut on_step: impl FnMut(&str),
        mut on_aggregate_timing: impl FnMut(NavAggregateTiming),
    ) -> DbResult<()> {
        self.finish_root_scan_with_cancel(
            project_id,
            options,
            &mut on_step,
            &mut on_aggregate_timing,
        )
    }

    fn finish_root_scan_with_cancel<S, T>(
        &mut self,
        project_id: i64,
        options: RootScanFinish<'_>,
        on_step: &mut S,
        on_aggregate_timing: &mut T,
    ) -> DbResult<()>
    where
        S: FnMut(&str),
        T: FnMut(NavAggregateTiming),
    {
        finalizing_step(
            &self.conn,
            options.cancel.as_ref(),
            "Finalizing: recording local Git metadata.",
            on_step,
            |conn| insert_git_metadata(conn, project_id, options.git),
        )?;
        finalizing_step(
            &self.conn,
            options.cancel.as_ref(),
            "Finalizing: updating folder child counts.",
            on_step,
            |conn| recalculate_child_counts(conn, project_id),
        )?;
        finalizing_step(
            &self.conn,
            options.cancel.as_ref(),
            "Finalizing: recalculating folder sizes.",
            on_step,
            |conn| {
                let timing = recalculate_nav_aggregates_interruptible_timed(
                    conn,
                    project_id,
                    options.cancel.as_deref(),
                )?;
                on_aggregate_timing(timing);
                Ok(())
            },
        )?;
        finalizing_step(
            &self.conn,
            options.cancel.as_ref(),
            "Finalizing: rebuilding local Markdown links.",
            on_step,
            |conn| {
                rebuild_project_markdown_edges_interruptible(
                    conn,
                    project_id,
                    options.cancel.as_deref(),
                )
            },
        )?;
        finalizing_step(
            &self.conn,
            options.cancel.as_ref(),
            "Finalizing: resolving local workflow and model references.",
            on_step,
            |conn| {
                rebuild_project_workflow_edges_interruptible(
                    conn,
                    project_id,
                    options.cancel.as_deref(),
                )
            },
        )?;
        if options.scan_completed {
            finalizing_step(
                &self.conn,
                options.cancel.as_ref(),
                "Finalizing: marking this scan complete.",
                on_step,
                |conn| {
                    conn.execute(
                        "UPDATE scan_root SET last_scanned_at = ?2 WHERE path = ?1",
                        params![options.root_path, now()],
                    )?;
                    Ok(())
                },
            )?;
        } else {
            finalizing_step(
                &self.conn,
                options.cancel.as_ref(),
                "Finalizing: marking this scan incomplete.",
                on_step,
                |conn| {
                    conn.execute(
                        "UPDATE scan_root SET last_scanned_at = NULL WHERE path = ?1",
                        params![options.root_path],
                    )?;
                    Ok(())
                },
            )?;
        }
        Ok(())
    }

    pub fn mark_root_scan_incomplete(&mut self, root_path: &str) -> DbResult<()> {
        self.conn.execute(
            "UPDATE scan_root SET last_scanned_at = NULL WHERE path = ?1",
            params![root_path],
        )?;
        Ok(())
    }

    pub fn mark_subtree_scan_incomplete(&mut self, nav_id: i64, error: &str) -> DbResult<()> {
        self.conn.execute(
            "UPDATE nav_item
             SET fully_scanned = 0, scan_error = ?2
             WHERE id = ?1",
            params![nav_id, error],
        )?;
        Ok(())
    }

    pub fn begin_subtree_scan(&mut self, project_id: i64, nav_id: i64) -> DbResult<()> {
        self.conn.execute(
            "DELETE FROM setting WHERE key = ?1",
            params![workflow_graph_setting_key(project_id)],
        )?;
        mark_existing_descendant_nodes_absent(&self.conn, nav_id)?;
        delete_descendant_document_rows(&self.conn, nav_id)?;
        delete_descendant_nav_rows(&self.conn, nav_id)?;
        self.conn.execute(
            "UPDATE nav_item
             SET fully_scanned = 0, scan_error = 'Scan in progress'
             WHERE id = ?1 AND project_id = ?2",
            params![nav_id, project_id],
        )?;
        Ok(())
    }

    pub fn finish_subtree_scan(
        &mut self,
        project_id: i64,
        nav_id: i64,
        partial_error: Option<&str>,
    ) -> DbResult<()> {
        self.finish_subtree_scan_with_progress(project_id, nav_id, partial_error, |_| {})
    }

    pub fn finish_subtree_scan_with_progress(
        &mut self,
        project_id: i64,
        nav_id: i64,
        partial_error: Option<&str>,
        mut on_step: impl FnMut(&str),
    ) -> DbResult<()> {
        let mut ignore_timing = |_| {};
        self.finish_subtree_scan_with_cancel(
            project_id,
            nav_id,
            partial_error,
            None,
            &mut on_step,
            &mut ignore_timing,
        )
    }

    pub fn finish_subtree_scan_interruptible_with_progress(
        &mut self,
        project_id: i64,
        nav_id: i64,
        partial_error: Option<&str>,
        cancel: Arc<AtomicBool>,
        mut on_step: impl FnMut(&str),
    ) -> DbResult<()> {
        let mut ignore_timing = |_| {};
        self.finish_subtree_scan_with_cancel(
            project_id,
            nav_id,
            partial_error,
            Some(cancel),
            &mut on_step,
            &mut ignore_timing,
        )
    }

    pub fn finish_subtree_scan_interruptible_with_progress_and_timing(
        &mut self,
        project_id: i64,
        nav_id: i64,
        partial_error: Option<&str>,
        cancel: Arc<AtomicBool>,
        mut on_step: impl FnMut(&str),
        mut on_aggregate_timing: impl FnMut(NavAggregateTiming),
    ) -> DbResult<()> {
        self.finish_subtree_scan_with_cancel(
            project_id,
            nav_id,
            partial_error,
            Some(cancel),
            &mut on_step,
            &mut on_aggregate_timing,
        )
    }

    fn finish_subtree_scan_with_cancel<S, T>(
        &mut self,
        project_id: i64,
        nav_id: i64,
        partial_error: Option<&str>,
        cancel: Option<Arc<AtomicBool>>,
        on_step: &mut S,
        on_aggregate_timing: &mut T,
    ) -> DbResult<()>
    where
        S: FnMut(&str),
        T: FnMut(NavAggregateTiming),
    {
        finalizing_step(
            &self.conn,
            cancel.as_ref(),
            "Finalizing: marking subtree scan state.",
            on_step,
            |conn| {
                match partial_error {
                    Some(error) => {
                        conn.execute(
                            "UPDATE nav_item
                             SET fully_scanned = 0, scan_error = ?2
                             WHERE id = ?1",
                            params![nav_id, error],
                        )?;
                    }
                    None => {
                        conn.execute(
                            "UPDATE nav_item
                             SET fully_scanned = 1, scan_error = NULL
                             WHERE id = ?1",
                            params![nav_id],
                        )?;
                    }
                }
                Ok(())
            },
        )?;
        finalizing_step(
            &self.conn,
            cancel.as_ref(),
            "Finalizing: updating folder child counts.",
            on_step,
            |conn| recalculate_child_counts(conn, project_id),
        )?;
        finalizing_step(
            &self.conn,
            cancel.as_ref(),
            "Finalizing: recalculating folder sizes.",
            on_step,
            |conn| {
                let timing = recalculate_nav_aggregates_interruptible_timed(
                    conn,
                    project_id,
                    cancel.as_deref(),
                )?;
                on_aggregate_timing(timing);
                Ok(())
            },
        )?;
        finalizing_step(
            &self.conn,
            cancel.as_ref(),
            "Finalizing: rebuilding local Markdown links.",
            on_step,
            |conn| {
                rebuild_project_markdown_edges_interruptible(conn, project_id, cancel.as_deref())
            },
        )?;
        finalizing_step(
            &self.conn,
            cancel.as_ref(),
            "Finalizing: resolving local workflow and model references.",
            on_step,
            |conn| {
                rebuild_project_workflow_edges_interruptible(conn, project_id, cancel.as_deref())
            },
        )?;
        Ok(())
    }
}

#[derive(Debug)]
struct PreviewRecord {
    path: String,
    display_path: String,
    display_name: String,
    attributes: Option<String>,
    project_id: i64,
    is_sensitive: bool,
    protected_level: Option<String>,
    is_markdown: bool,
    is_context: bool,
    item_kind: String,
    size_bytes: Option<u64>,
    is_reparse: bool,
    reparse_kind: Option<String>,
}

fn missing_file_preview(node_id: i64, mode: PreviewMode) -> FilePreview {
    FilePreview {
        node_id,
        project_id: 0,
        path: String::new(),
        display_path: String::new(),
        display_name: "Missing file".to_string(),
        mode,
        state: PreviewState::Missing,
        file_kind: FileKind::Unsupported,
        size_bytes: None,
        truncated: false,
        preview_limit_bytes: PREVIEW_LIMIT_BYTES,
        system_error_code: None,
        was_revealed: false,
        source: None,
        rendered_html: None,
        blocked_reason: Some("The requested node is not in the navigation index.".to_string()),
        headings: Vec::new(),
        links: Vec::new(),
    }
}

fn context_recommendation_rank(path: &str, display_name: &str) -> i64 {
    let normalized = normalize_path(path).to_ascii_lowercase();
    let name = display_name.to_ascii_lowercase();
    let depth = normalized.matches('/').count() as i64;
    match normalized.as_str() {
        "readme.md" => 0,
        "agents.md" => 1,
        "claude.md" | "gemini.md" => 2,
        _ if normalized.starts_with(".cursor/rules/") => 4 + depth,
        "docs/readme.md" | "docs/index.md" | "docs/overview.md" => 8,
        _ if normalized.starts_with("docs/") => 14 + depth,
        _ if normalized.starts_with(".claude/") || normalized.contains("/.claude/") => 18 + depth,
        _ if normalized.starts_with("prompts/") => 20 + depth,
        _ if normalized.starts_with("commands/") || normalized.contains("/commands/") => 22 + depth,
        _ if normalized.starts_with("instructions/") || normalized.contains("/instructions/") => {
            23 + depth
        }
        _ if depth == 0
            && matches!(
                name.as_str(),
                "package.json" | "pyproject.toml" | "cargo.toml" | "go.mod" | "requirements.txt"
            ) =>
        {
            24
        }
        _ if name == "readme.md" => 60 + depth,
        _ if normalized.contains("/docs/") => 72 + depth,
        _ if normalized.contains("/prompts/") => 78 + depth,
        _ if matches!(
            name.as_str(),
            "package.json" | "pyproject.toml" | "cargo.toml" | "go.mod" | "requirements.txt"
        ) =>
        {
            86 + depth
        }
        _ => 100 + depth,
    }
}

fn context_recommendation_metadata(path: &str, display_name: &str) -> (String, String) {
    let normalized = normalize_path(path).to_ascii_lowercase();
    let name = display_name.to_ascii_lowercase();
    if normalized == "readme.md" {
        return (
            "Project overview".to_string(),
            "Root README usually gives the fastest project overview.".to_string(),
        );
    }
    if matches!(normalized.as_str(), "agents.md" | "claude.md" | "gemini.md")
        || normalized.starts_with(".cursor/rules/")
        || normalized.starts_with(".claude/")
    {
        return (
            "Agent instructions".to_string(),
            "Local assistant or editor rules explain how the project should be handled."
                .to_string(),
        );
    }
    if normalized.starts_with("commands/") || normalized.contains("/commands/") {
        return (
            "Local commands".to_string(),
            "Command Markdown files often describe reusable project workflows.".to_string(),
        );
    }
    if normalized.starts_with("instructions/") || normalized.contains("/instructions/") {
        return (
            "Instructions".to_string(),
            "Instruction Markdown files usually explain how local work should be handled."
                .to_string(),
        );
    }
    if normalized.starts_with("docs/") {
        return (
            "Documentation".to_string(),
            "Documentation near the root is usually higher signal than repeated package READMEs."
                .to_string(),
        );
    }
    if normalized.starts_with("prompts/") {
        return (
            "Prompt/workflow context".to_string(),
            "Prompt and workflow files describe how local AI work is organized.".to_string(),
        );
    }
    if matches!(
        name.as_str(),
        "package.json" | "pyproject.toml" | "cargo.toml" | "go.mod" | "requirements.txt"
    ) {
        return (
            "Project manifest".to_string(),
            "Manifest files identify runtimes, dependencies and project shape.".to_string(),
        );
    }
    if name == "readme.md" {
        return (
            "Nested README".to_string(),
            "Nested READMEs can be useful, but repeated package READMEs are lower priority."
                .to_string(),
        );
    }
    (
        "Additional context".to_string(),
        "Available as context, but lower priority than root docs and project instructions."
            .to_string(),
    )
}

fn build_file_preview_from_record(
    node_id: i64,
    mode: PreviewMode,
    reveal: bool,
    policy: PreviewPolicy,
    record: PreviewRecord,
) -> FilePreview {
    let file_kind = file_kind_for_record(&record);
    if record.item_kind == "directory" {
        return FilePreview {
            node_id,
            project_id: record.project_id,
            path: record.path,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Unsupported,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: false,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: None,
            was_revealed: reveal,
            source: None,
            rendered_html: None,
            blocked_reason: Some(
                "Directory preview is metadata-only. Expand it in the tree to inspect children."
                    .to_string(),
            ),
            headings: Vec::new(),
            links: Vec::new(),
        };
    }

    // Never read through a reparse point. A cloud placeholder (online-only OneDrive/Dropbox
    // file) has is_reparse=0 but reparse_kind='cloud_placeholder' — opening it would silently
    // hydrate it (network egress). A genuine FILE symlink/junction has is_reparse=1 — opening it
    // FOLLOWS the link and would read a TARGET outside the scanned tree (e.g. a symlink to
    // ~/.ssh/id_rsa). Both are blocked to metadata-only, matching the graph/duplicate read gates
    // and SECURITY_INVARIANTS.md ("reparse points ... never opened").
    if record.is_reparse || record.reparse_kind.as_deref() == Some("cloud_placeholder") {
        return FilePreview {
            node_id,
            project_id: record.project_id,
            path: record.path,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Blocked,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: false,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: None,
            was_revealed: false,
            source: None,
            rendered_html: None,
            blocked_reason: Some(
                "This file is stored online-only (a cloud placeholder). Code Hangar will not download it to preview — open it in its owning app to materialize it locally first."
                    .to_string(),
            ),
            headings: Vec::new(),
            links: Vec::new(),
        };
    }

    let protected_or_sensitive = record.is_sensitive || record.protected_level.is_some();
    let strong_protected = is_strong_protected_path(&record.display_path);
    // Auto-preview ("relax") is a convenience layer on top of the explicit
    // reveal consent, never an independent way to expose content. Without
    // allow_sensitive_reveal it has no effect, so relaxing the preview block
    // can never surface sensitive or protected text on its own.
    let relaxed_preview = protected_or_sensitive
        && !reveal
        && policy.relax_non_strong_protected_preview
        && policy.allow_sensitive_reveal;

    if protected_or_sensitive && !reveal && (!relaxed_preview || strong_protected) {
        return FilePreview {
            node_id,
            project_id: record.project_id,
            path: record.path,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Blocked,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: false,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: None,
            was_revealed: false,
            source: None,
            rendered_html: None,
            blocked_reason: Some("Preview blocked by sensitive-file or Protected Zone policy. Enable temporary local visibility to reveal non-strong protected text.".to_string()),
            headings: Vec::new(),
            links: Vec::new(),
        };
    }

    if reveal && !policy.allow_sensitive_reveal && protected_or_sensitive {
        return FilePreview {
            node_id,
            project_id: record.project_id,
            path: record.path,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Blocked,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: false,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: None,
            was_revealed: false,
            source: None,
            rendered_html: None,
            blocked_reason: Some(
                "Temporary sensitive reveal is disabled for this session.".to_string(),
            ),
            headings: Vec::new(),
            links: Vec::new(),
        };
    }

    if (reveal || relaxed_preview) && strong_protected {
        return FilePreview {
            node_id,
            project_id: record.project_id,
            path: record.path,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Blocked,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: false,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: None,
            was_revealed: false,
            source: None,
            rendered_html: None,
            blocked_reason: Some(
                "Strong Protected Zone content cannot be revealed in this phase.".to_string(),
            ),
            headings: Vec::new(),
            links: Vec::new(),
        };
    }

    let read_result = match body_from_attributes(record.attributes.as_deref()) {
        Some(source) => TextReadResult::Text(TextRead {
            source,
            truncated: false,
            system_error_code: None,
        }),
        None => read_disk_text_limited(&record.path),
    };

    let was_transiently_revealed = reveal || relaxed_preview;
    let text = match read_result {
        TextReadResult::Text(value) => value,
        TextReadResult::Binary => {
            return FilePreview {
                node_id,
                project_id: record.project_id,
                path: record.path,
                display_path: record.display_path,
                display_name: record.display_name,
                mode,
                state: PreviewState::Unsupported,
                file_kind: FileKind::Binary,
                size_bytes: record.size_bytes,
                truncated: false,
                preview_limit_bytes: PREVIEW_LIMIT_BYTES,
                system_error_code: None,
                was_revealed: was_transiently_revealed,
                source: None,
                rendered_html: None,
                blocked_reason: Some(
                    "Binary or unsupported text encoding. Metadata only.".to_string(),
                ),
                headings: Vec::new(),
                links: Vec::new(),
            };
        }
        TextReadResult::Error(error) => {
            return FilePreview {
                node_id,
                project_id: record.project_id,
                path: record.path,
                display_path: record.display_path,
                display_name: record.display_name,
                mode,
                state: PreviewState::Unsupported,
                file_kind,
                size_bytes: record.size_bytes,
                truncated: false,
                preview_limit_bytes: PREVIEW_LIMIT_BYTES,
                system_error_code: error.system_error_code,
                was_revealed: was_transiently_revealed,
                source: None,
                rendered_html: None,
                blocked_reason: Some(error.message),
                headings: Vec::new(),
                links: Vec::new(),
            };
        }
    };

    match mode {
        PreviewMode::Source => FilePreview {
            node_id,
            project_id: record.project_id,
            path: record.path,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Ready,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: text.truncated,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: text.system_error_code,
            was_revealed: was_transiently_revealed,
            source: Some(text.source),
            rendered_html: None,
            blocked_reason: None,
            headings: Vec::new(),
            links: Vec::new(),
        },
        PreviewMode::Rendered if record.is_markdown => {
            let rendered = render_markdown_safe(&text.source);
            FilePreview {
                node_id,
                project_id: record.project_id,
                path: record.path,
                display_path: record.display_path,
                display_name: record.display_name,
                mode,
                state: PreviewState::Ready,
                file_kind,
                size_bytes: record.size_bytes,
                truncated: text.truncated,
                preview_limit_bytes: PREVIEW_LIMIT_BYTES,
                system_error_code: text.system_error_code,
                was_revealed: was_transiently_revealed,
                source: None,
                rendered_html: Some(rendered.html),
                blocked_reason: None,
                headings: rendered.headings,
                links: rendered.links,
            }
        }
        PreviewMode::Rendered => FilePreview {
            node_id,
            path: record.path,
            project_id: record.project_id,
            display_path: record.display_path,
            display_name: record.display_name,
            mode,
            state: PreviewState::Ready,
            file_kind,
            size_bytes: record.size_bytes,
            truncated: text.truncated,
            preview_limit_bytes: PREVIEW_LIMIT_BYTES,
            system_error_code: text.system_error_code,
            was_revealed: was_transiently_revealed,
            source: None,
            rendered_html: Some(format!(
                "<pre><code>{}</code></pre>",
                escape_html(&text.source)
            )),
            blocked_reason: None,
            headings: Vec::new(),
            links: Vec::new(),
        },
    }
}

const MIGRATION_001: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migration (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS setting (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS scan_root (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_scanned_at TEXT
);

CREATE TABLE IF NOT EXISTS node (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  path TEXT,
  name TEXT,
  protected_level TEXT,
  volume_id TEXT,
  inode_key TEXT,
  link_count INTEGER,
  is_reparse INTEGER NOT NULL DEFAULT 0,
  reparse_kind TEXT,
  size_apparent INTEGER,
  size_allocated INTEGER,
  mtime TEXT,
  attributes TEXT,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  present INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_node_kind ON node(kind);
CREATE INDEX IF NOT EXISTS idx_node_path ON node(path);
CREATE INDEX IF NOT EXISTS idx_node_present ON node(present);

CREATE TABLE IF NOT EXISTS git_repo (
  project_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  current_branch TEXT,
  head_ref TEXT,
  origin_url TEXT,
  metadata_error TEXT,
  indexed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS edge (
  id INTEGER PRIMARY KEY,
  src INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  dst INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  confidence TEXT NOT NULL,
  evidence TEXT,
  UNIQUE(src, dst, kind)
);

CREATE INDEX IF NOT EXISTS idx_edge_src ON edge(src, kind);
CREATE INDEX IF NOT EXISTS idx_edge_dst ON edge(dst, kind);

CREATE TABLE IF NOT EXISTS relationship_issue (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  kind TEXT NOT NULL,
  confidence TEXT NOT NULL,
  target TEXT NOT NULL,
  evidence TEXT,
  UNIQUE(node_id, kind, target)
);

CREATE INDEX IF NOT EXISTS idx_relationship_issue_node ON relationship_issue(node_id, kind);

CREATE TABLE IF NOT EXISTS duplicate_group (
  id INTEGER PRIMARY KEY,
  size INTEGER NOT NULL,
  hash_partial TEXT,
  hash_full TEXT,
  confirmed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS duplicate_member (
  group_id INTEGER NOT NULL REFERENCES duplicate_group(id) ON DELETE CASCADE,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  PRIMARY KEY (group_id, node_id)
);

CREATE TABLE IF NOT EXISTS protected_zone (
  id INTEGER PRIMARY KEY,
  pattern_type TEXT NOT NULL,
  pattern TEXT NOT NULL,
  level TEXT NOT NULL,
  source TEXT NOT NULL,
  UNIQUE(pattern_type, pattern, source)
);

CREATE TABLE IF NOT EXISTS adapter (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  type TEXT NOT NULL,
  platforms TEXT NOT NULL,
  definition_json TEXT NOT NULL,
  source TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  schema_version INTEGER NOT NULL,
  UNIQUE(name, version, source)
);

CREATE TABLE IF NOT EXISTS nav_item (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  node_id INTEGER REFERENCES node(id) ON DELETE CASCADE,
  parent_nav_id INTEGER REFERENCES nav_item(id),
  path TEXT NOT NULL,
  display_path TEXT,
  display_name TEXT NOT NULL,
  item_kind TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  sort_key TEXT NOT NULL,
  is_context INTEGER NOT NULL DEFAULT 0,
  is_markdown INTEGER NOT NULL DEFAULT 0,
  is_sensitive INTEGER NOT NULL DEFAULT 0,
  protected_level TEXT,
  child_count INTEGER NOT NULL DEFAULT 0,
  fully_scanned INTEGER NOT NULL DEFAULT 1,
  collapse_default INTEGER NOT NULL DEFAULT 0,
  scan_error TEXT,
  aggregate_apparent_bytes INTEGER,
  aggregate_allocated_bytes INTEGER,
  aggregate_physical_bytes INTEGER,
  aggregate_bytes_partial INTEGER NOT NULL DEFAULT 0,
  last_opened_at TEXT,
  pinned INTEGER NOT NULL DEFAULT 0,
  UNIQUE(project_id, path)
);

CREATE INDEX IF NOT EXISTS idx_nav_project_parent ON nav_item(project_id, parent_nav_id, priority, sort_key);
CREATE INDEX IF NOT EXISTS idx_nav_context ON nav_item(project_id, is_context, priority);
CREATE INDEX IF NOT EXISTS idx_nav_parent ON nav_item(parent_nav_id);
CREATE INDEX IF NOT EXISTS idx_nav_node ON nav_item(node_id);

CREATE TABLE IF NOT EXISTS document_index (
  node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  title TEXT,
  headings_json TEXT,
  links_json TEXT,
  backlinks_dirty INTEGER NOT NULL DEFAULT 1,
  preview_cache_key TEXT,
  preview_safe INTEGER NOT NULL DEFAULT 1,
  preview_blocked_reason TEXT,
  language TEXT,
  text_size INTEGER,
  indexed_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS document_fts USING fts5(
  node_id UNINDEXED,
  project_id UNINDEXED,
  path,
  title,
  headings,
  body
);

CREATE TABLE IF NOT EXISTS recent_item (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  item_kind TEXT NOT NULL,
  opened_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pinned_item (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  item_kind TEXT NOT NULL,
  pinned_at TEXT NOT NULL,
  UNIQUE(node_id, item_kind)
);

CREATE TABLE IF NOT EXISTS project_review_checkpoint (
  project_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  reviewed_at TEXT NOT NULL,
  session_cutoff_ms INTEGER NOT NULL,
  git_fingerprint TEXT,
  git_head TEXT
);

CREATE TABLE IF NOT EXISTS project_check_approval (
  project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  check_id TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  approved_at TEXT NOT NULL,
  PRIMARY KEY(project_id, check_id)
);

CREATE TABLE IF NOT EXISTS review_evidence (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  source_kind TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  source_modified_ms INTEGER NOT NULL DEFAULT -1,
  observed_at TEXT NOT NULL,
  change_set_json TEXT NOT NULL,
  UNIQUE(project_id, source_ref, source_modified_ms)
);
CREATE INDEX IF NOT EXISTS idx_review_evidence_project
  ON review_evidence(project_id, source_modified_ms DESC, id DESC);

CREATE TABLE IF NOT EXISTS change_ledger (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  node_id INTEGER REFERENCES node(id) ON DELETE SET NULL,
  source_kind TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  source_modified_ms INTEGER NOT NULL DEFAULT -1,
  observed_at TEXT NOT NULL,
  origin TEXT,
  session_id TEXT,
  before_hash TEXT,
  after_hash TEXT,
  content_hash TEXT NOT NULL,
  previous_entry_hash TEXT,
  entry_hash TEXT NOT NULL UNIQUE,
  encoded_bytes INTEGER NOT NULL,
  change_set_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_change_ledger_project
  ON change_ledger(project_id, source_modified_ms DESC, id DESC);

CREATE TABLE IF NOT EXISTS ai_glossary (
  term TEXT PRIMARY KEY COLLATE NOCASE,
  definition TEXT NOT NULL,
  seen_count INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS code_annotation (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  snippet_hash TEXT NOT NULL,
  line_start INTEGER NOT NULL,
  line_end INTEGER NOT NULL,
  snippet_text TEXT NOT NULL,
  note TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_code_annotation_node
  ON code_annotation(node_id, line_start, id);

CREATE TABLE IF NOT EXISTS comment (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id) ON DELETE CASCADE,
  body TEXT NOT NULL,
  author TEXT NOT NULL DEFAULT 'user',
  source TEXT NOT NULL DEFAULT 'user',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_comment_node ON comment(node_id, deleted_at, created_at);
CREATE INDEX IF NOT EXISTS idx_comment_project ON comment(project_id);

-- A connected AI app's REQUEST for an action it may not perform itself (total
-- control). The app never executes on the agent's say-so; a human approves or
-- rejects each row, and on approval the app performs it AS the user. The agent
-- only ever inserts a 'pending' row here.
CREATE TABLE IF NOT EXISTS agent_request (
  id INTEGER PRIMARY KEY,
  agent_id INTEGER,
  agent_name TEXT NOT NULL,
  kind TEXT NOT NULL,
  target_comment_id INTEGER REFERENCES comment(id) ON DELETE CASCADE,
  proposed_body TEXT,
  detail TEXT,
  status TEXT NOT NULL DEFAULT 'pending',
  created_at TEXT NOT NULL,
  resolved_at TEXT,
  -- Total-control extension: generic request kinds (not just comments). Comment
  -- rows keep target_comment_id/proposed_body; other kinds use these and leave the
  -- comment columns NULL. project_id is the target's resolved owning project (NULL
  -- for project-less targets); cross_scope=1 marks a target outside the agent's
  -- grants (allowed, but needs an extra in-app authorization on approval).
  target_kind TEXT,
  target_id INTEGER,
  project_id INTEGER,
  payload_json TEXT,
  result_json TEXT,
  cross_scope INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_agent_request_status ON agent_request(status, created_at);

CREATE TABLE IF NOT EXISTS scan_cache (
  path TEXT PRIMARY KEY,
  mtime TEXT NOT NULL,
  size INTEGER NOT NULL,
  dir_signature TEXT,
  node_id INTEGER REFERENCES node(id)
);

INSERT OR IGNORE INTO schema_migration(version, applied_at) VALUES(1, datetime('now'));
"#;

fn ensure_phase1b_columns(conn: &Connection) -> DbResult<()> {
    let node_columns = [
        ("volume_id", "TEXT"),
        ("inode_key", "TEXT"),
        ("link_count", "INTEGER"),
        ("is_reparse", "INTEGER NOT NULL DEFAULT 0"),
        ("reparse_kind", "TEXT"),
        ("size_apparent", "INTEGER"),
        ("size_allocated", "INTEGER"),
        ("mtime", "TEXT"),
    ];

    for (name, definition) in node_columns {
        ensure_column(conn, "node", name, definition)?;
    }
    let nav_columns = [
        ("display_path", "TEXT"),
        ("child_count", "INTEGER NOT NULL DEFAULT 0"),
        ("fully_scanned", "INTEGER NOT NULL DEFAULT 1"),
        ("collapse_default", "INTEGER NOT NULL DEFAULT 0"),
        ("scan_error", "TEXT"),
        ("aggregate_apparent_bytes", "INTEGER"),
        ("aggregate_allocated_bytes", "INTEGER"),
        ("aggregate_physical_bytes", "INTEGER"),
        ("aggregate_bytes_partial", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, definition) in nav_columns {
        ensure_column(conn, "nav_item", name, definition)?;
    }
    // Ad-hoc roots are folders the user investigates by path. They are scanned/indexed so
    // the explanation + Gate-3 backup/move/delete pipeline work, but are excluded from the
    // projects list, discovery, and the scan-roots settings (they are never "registered").
    ensure_column(conn, "scan_root", "adhoc", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(conn, "project_review_checkpoint", "git_head", "TEXT")?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_node_inode ON node(volume_id, inode_key)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_nav_parent ON nav_item(parent_nav_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_nav_node ON nav_item(node_id)",
        [],
    )?;
    // Total-control extension: generalize agent_request beyond comments. Additive
    // columns; existing comment rows keep their comment fields and leave these NULL.
    let agent_request_columns = [
        ("target_kind", "TEXT"),
        ("target_id", "INTEGER"),
        ("project_id", "INTEGER"),
        ("payload_json", "TEXT"),
        ("result_json", "TEXT"),
        ("cross_scope", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, definition) in agent_request_columns {
        ensure_column(conn, "agent_request", name, definition)?;
    }
    Ok(())
}

fn ensure_change_ledger_schema(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS change_ledger (
           id INTEGER PRIMARY KEY,
           project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           node_id INTEGER REFERENCES node(id) ON DELETE SET NULL,
           source_kind TEXT NOT NULL,
           source_ref TEXT NOT NULL,
           source_modified_ms INTEGER NOT NULL DEFAULT -1,
           observed_at TEXT NOT NULL,
           origin TEXT,
           session_id TEXT,
           before_hash TEXT,
           after_hash TEXT,
           content_hash TEXT NOT NULL,
           previous_entry_hash TEXT,
           entry_hash TEXT NOT NULL UNIQUE,
           encoded_bytes INTEGER NOT NULL,
           change_set_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_change_ledger_project
           ON change_ledger(project_id, source_modified_ms DESC, id DESC);
         INSERT OR IGNORE INTO change_ledger(
           project_id, source_kind, source_ref, source_modified_ms, observed_at,
           content_hash, entry_hash, encoded_bytes, change_set_json
         )
         SELECT project_id, source_kind, source_ref, source_modified_ms, observed_at,
                'legacy:' || id, 'legacy:' || id, length(change_set_json), change_set_json
         FROM review_evidence;
         DELETE FROM review_evidence;",
    )?;
    Ok(())
}

#[cfg(feature = "agent_automation")]
fn ensure_automation_schema(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS automation_agent (
           id INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           token_hash TEXT NOT NULL UNIQUE,
           scopes_json TEXT NOT NULL,
           project_ids_json TEXT NOT NULL,
           enabled INTEGER NOT NULL DEFAULT 1,
           created_at TEXT NOT NULL,
           last_seen_at TEXT
         );
         CREATE TABLE IF NOT EXISTS automation_read_grant (
           id INTEGER PRIMARY KEY,
           agent_id INTEGER NOT NULL REFERENCES automation_agent(id) ON DELETE CASCADE,
           node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           expires_at_ms INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           revoked_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_automation_grant_agent_node
           ON automation_read_grant(agent_id, node_id, expires_at_ms);
         CREATE TABLE IF NOT EXISTS automation_activity (
           id INTEGER PRIMARY KEY,
           agent_id INTEGER REFERENCES automation_agent(id) ON DELETE SET NULL,
           method TEXT NOT NULL,
           status TEXT NOT NULL,
           detail TEXT NOT NULL,
           created_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_automation_activity_agent
           ON automation_activity(agent_id, id DESC);",
    )?;
    Ok(())
}

#[cfg(feature = "agent_automation")]
fn automation_agent_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationAgentSummary> {
    let scopes_json: String = row.get(2)?;
    let projects_json: String = row.get(3)?;
    Ok(AutomationAgentSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        scopes: serde_json::from_str(&scopes_json).unwrap_or_default(),
        project_ids: serde_json::from_str(&projects_json).unwrap_or_default(),
        enabled: row.get(4)?,
        created_at: row.get(5)?,
        last_seen_at: row.get(6)?,
    })
}

/// True if `inner` is strictly inside `outer` (case/separator-insensitive on Windows).
fn path_is_inside(inner: &str, outer: &str) -> bool {
    let norm = |p: &str| {
        p.replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    let inner = norm(inner);
    let outer = norm(outer);
    inner.len() > outer.len() && inner.starts_with(&format!("{outer}/"))
}

fn ensure_column(conn: &Connection, table: &str, name: &str, definition: &str) -> DbResult<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = collect_rows(stmt.query_map([], |row| row.get::<_, String>(1))?)?;
    if !columns.iter().any(|column| column == name) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn delete_by_node_ids(
    conn: &Connection,
    table: &str,
    column: &str,
    node_ids: &[i64],
) -> DbResult<()> {
    for chunk in node_ids.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        conn.execute(
            &format!("DELETE FROM {table} WHERE {column} IN ({placeholders})"),
            params_from_iter(chunk.iter()),
        )?;
    }
    Ok(())
}

fn delete_unreferenced_node_ids(conn: &Connection, node_ids: &[i64]) -> DbResult<()> {
    for chunk in node_ids.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        conn.execute(
            &format!(
                "DELETE FROM node
                 WHERE id IN ({placeholders})
                   AND NOT EXISTS (
                     SELECT 1 FROM nav_item
                     WHERE nav_item.node_id = node.id
                   )"
            ),
            params_from_iter(chunk.iter()),
        )?;
    }
    Ok(())
}

fn configure_file_connection(conn: &Connection, cipher_key: &str) -> DbResult<()> {
    apply_cipher_key(conn, cipher_key)?;
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA wal_autocheckpoint=1000;
         PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;",
    )?;
    validate_cipher_key(conn)?;
    Ok(())
}

/// Reclaim a WAL left by a crashed/force-killed previous run. This belongs only on the primary
/// startup connection: `configure_file_connection` is also used by every short-lived writer,
/// scan session and recovery operation, where checkpointing on every open adds avoidable
/// contention and latency.
fn checkpoint_stale_wal(conn: &Connection) {
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
}

fn configure_file_read_connection(conn: &Connection, cipher_key: &str) -> DbResult<()> {
    apply_cipher_key(conn, cipher_key)?;
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.execute_batch(
        "PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;",
    )?;
    validate_cipher_key(conn)?;
    Ok(())
}

fn configure_memory_connection(conn: &Connection) -> DbResult<()> {
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.execute_batch(
        "PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

fn apply_cipher_key(conn: &Connection, cipher_key: &str) -> DbResult<()> {
    conn.pragma_update(None, "key", cipher_key)?;
    Ok(())
}

fn validate_cipher_key(conn: &Connection) -> DbResult<()> {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(())
}

fn key_path_for_database(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("codehangar.sqlite3");
    path.with_file_name(format!("{file_name}.key.dpapi"))
}

/// Re-wrap the DB key with DPAPI entropy and replace the blob atomically: write a
/// sibling temp, fsync it, then rename over the original (atomic on NTFS). A crash
/// at any point leaves either the intact original or the fully-written new blob —
/// never a truncated one — so the sole key file can never be corrupted by the
/// upgrade write.
fn atomic_rewrap_key_blob(path: &Path, key: &[u8]) -> DbResult<()> {
    use std::io::Write;
    let reprotected = protect_key_material(key)?;
    let mut temp_path: std::ffi::OsString = path.as_os_str().to_os_string();
    temp_path.push(".rewrap.tmp");
    let temp_path = PathBuf::from(temp_path);
    {
        let mut file = fs::File::create(&temp_path).map_err(|err| {
            DbError::FileRead(format!("Failed to create key re-wrap temp: {err}"))
        })?;
        file.write_all(&reprotected)
            .map_err(|err| DbError::FileRead(format!("Failed to write key re-wrap temp: {err}")))?;
        file.sync_all()
            .map_err(|err| DbError::FileRead(format!("Failed to flush key re-wrap temp: {err}")))?;
    }
    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(DbError::FileRead(format!(
            "Failed to promote the re-wrapped key blob: {err}"
        )));
    }
    Ok(())
}

fn load_or_create_database_key(path: &Path) -> DbResult<String> {
    if path.exists() {
        let protected = fs::read(path).map_err(|err| {
            DbError::FileRead(format!(
                "Failed to read encrypted database key material: {err}"
            ))
        })?;
        let (key, was_legacy) = unprotect_key_material(&protected)?;
        if key.len() != DB_KEY_BYTES {
            return Err(DbError::FileRead(
                "Encrypted database key material has an invalid length.".to_string(),
            ));
        }
        // A blob written by an older build (no DPAPI entropy) still opens; re-wrap it
        // with entropy so the upgrade is transparent. This must be CRASH-SAFE: this
        // file is the sole copy of the key blob, so the re-wrap writes a temp and
        // atomically renames over the original — a crash mid-write can never leave a
        // truncated/corrupt blob that would brick the database. Best-effort: the
        // legacy blob is already valid, so any failure leaves it untouched and falls
        // through to return the key.
        if was_legacy {
            let _ = atomic_rewrap_key_blob(path, &key);
        }
        return Ok(hex_encode(&key));
    }

    let key = generate_key_material()?;
    let protected = protect_key_material(&key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            DbError::FileRead(format!(
                "Failed to create encrypted database key directory: {err}"
            ))
        })?;
    }
    fs::write(path, protected).map_err(|err| {
        DbError::FileRead(format!(
            "Failed to write encrypted database key material: {err}"
        ))
    })?;
    Ok(hex_encode(&key))
}

fn is_plaintext_sqlite_database(path: &Path) -> DbResult<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut file = fs::File::open(path)
        .map_err(|err| DbError::FileRead(format!("Failed to inspect database header: {err}")))?;
    let mut header = [0_u8; 16];
    let read = file
        .read(&mut header)
        .map_err(|err| DbError::FileRead(format!("Failed to inspect database header: {err}")))?;
    Ok(read == SQLITE_HEADER.len() && header.as_slice() == SQLITE_HEADER)
}

fn file_is_nonempty(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

/// Reconcile a plaintext->encrypted migration that a crash (power loss / kill)
/// interrupted, so it can never (a) lose the user's data or (b) leave a readable
/// plaintext copy of the whole database (`*.plaintext-migrating`) lying on disk
/// past startup. Runs before every open and is a no-op unless a migration
/// artifact is actually present, so the steady-state (already-encrypted) path
/// pays only two `metadata` probes.
///
/// The interrupted states and their resolution:
/// - canonical is already a valid encrypted DB -> migration finished before the
///   cleanup step; the leftover plaintext copy is a pure leak -> delete it.
/// - a complete encrypted temp exists but was never promoted -> finish the
///   promotion, then delete the plaintext copy.
/// - neither -> restore the plaintext DB to the canonical path so the normal
///   migration re-runs on this open (no data loss).
fn reconcile_crashed_migration(path: &Path, cipher_key: &str) -> DbResult<()> {
    let migrating = path.with_extension("sqlite3.plaintext-migrating");
    let encrypted_temp = path.with_extension("sqlite3.encrypted.tmp");

    if file_is_nonempty(&migrating) {
        // Short-circuiting keeps validate (which would CREATE an empty encrypted DB
        // for a missing path) from ever running on a non-existent/empty file.
        let canonical_encrypted = file_is_nonempty(path)
            && !is_plaintext_sqlite_database(path).unwrap_or(false)
            && validate_encrypted_database(path, cipher_key).is_ok();
        if canonical_encrypted {
            remove_database_file_set(&migrating)?;
        } else if file_is_nonempty(&encrypted_temp)
            && validate_encrypted_database(&encrypted_temp, cipher_key).is_ok()
        {
            remove_database_file_set(path)?;
            fs::rename(&encrypted_temp, path).map_err(|err| {
                DbError::FileRead(format!(
                    "Failed to promote the encrypted database during recovery: {err}"
                ))
            })?;
            remove_database_file_set(&migrating)?;
        } else {
            remove_database_file_set(path)?;
            fs::rename(&migrating, path).map_err(|err| {
                DbError::FileRead(format!(
                    "Failed to restore the database for re-migration: {err}"
                ))
            })?;
        }
    }

    // Clear any stale/empty artifacts left behind so nothing readable survives.
    remove_database_file_set(&encrypted_temp)?;
    if migrating.exists() {
        remove_database_file_set(&migrating)?;
    }
    Ok(())
}

fn migrate_plaintext_database(path: &Path, cipher_key: &str) -> DbResult<()> {
    let encrypted_temp = path.with_extension("sqlite3.encrypted.tmp");
    remove_database_file_set(&encrypted_temp)?;

    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(5_000))?;
    let _ = conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA wal_checkpoint(FULL);");
    conn.execute(
        "ATTACH DATABASE ?1 AS encrypted KEY ?2",
        params![encrypted_temp.to_string_lossy().as_ref(), cipher_key],
    )?;
    conn.execute_batch("SELECT sqlcipher_export('encrypted'); DETACH DATABASE encrypted;")?;
    drop(conn);

    validate_encrypted_database(&encrypted_temp, cipher_key)?;

    let plaintext_backup = path.with_extension("sqlite3.plaintext-migrating");
    remove_database_file_set(&plaintext_backup)?;
    fs::rename(path, &plaintext_backup).map_err(|err| {
        DbError::FileRead(format!(
            "Failed to move plaintext database out of the active path: {err}"
        ))
    })?;
    if let Err(err) = fs::rename(&encrypted_temp, path) {
        let _ = fs::rename(&plaintext_backup, path);
        return Err(DbError::FileRead(format!(
            "Failed to promote encrypted database: {err}"
        )));
    }

    validate_encrypted_database(path, cipher_key)?;
    remove_database_file_set(&encrypted_temp)?;
    remove_database_file_set(&plaintext_backup)?;
    for sidecar in database_sidecar_paths(path) {
        remove_file_if_exists(&sidecar)?;
    }
    Ok(())
}

fn validate_encrypted_database(path: &Path, cipher_key: &str) -> DbResult<()> {
    let conn = Connection::open(path)?;
    configure_file_connection(&conn, cipher_key)
}

fn remove_database_file_set(path: &Path) -> DbResult<()> {
    remove_file_if_exists(path)?;
    for sidecar in database_sidecar_paths(path) {
        remove_file_if_exists(&sidecar)?;
    }
    Ok(())
}

fn database_sidecar_paths(path: &Path) -> Vec<PathBuf> {
    let path_text = path.to_string_lossy();
    vec![
        PathBuf::from(format!("{path_text}-wal")),
        PathBuf::from(format!("{path_text}-shm")),
    ]
}

fn remove_file_if_exists(path: &Path) -> DbResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(DbError::FileRead(format!(
            "Failed to remove plaintext database artifact {}: {err}",
            path.display()
        ))),
    }
}

/// Marker file written by a "Reset all" request. While it is present, the next
/// time the database is opened it is wiped first (see [`wipe_pending_reset`]).
pub fn reset_sentinel_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.reset-pending", db_path.to_string_lossy()))
}

/// Delete the database file and all of its sidecars (-wal, -shm, -journal),
/// reclaiming the disk space. Safe to call only when no connection is open —
/// this is why a reset defers the wipe to startup rather than doing it in place.
pub fn wipe_database_files(db_path: &Path) -> DbResult<()> {
    remove_database_file_set(db_path)?;
    remove_file_if_exists(&PathBuf::from(format!(
        "{}-journal",
        db_path.to_string_lossy()
    )))?;
    Ok(())
}

/// If a reset was requested, wipe the database now (before any connection opens)
/// and clear the marker. Returns true if the wipe succeeded.
///
/// A restart spawns the new process before the old one has fully exited, so the
/// outgoing process may still hold the file handle for a moment. We retry with
/// backoff and only clear the marker once the wipe actually succeeds — if it
/// never does, the marker is left in place so the wipe is retried on the next
/// startup rather than silently lost.
pub fn wipe_pending_reset(db_path: &Path) -> bool {
    let sentinel = reset_sentinel_path(db_path);
    if !sentinel.exists() {
        return false;
    }
    for attempt in 0..40 {
        if wipe_database_files(db_path).is_ok() {
            let _ = fs::remove_file(&sentinel);
            return true;
        }
        sleep(Duration::from_millis((50 * (attempt + 1)).min(500)));
    }
    false
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(windows)]
fn generate_key_material() -> DbResult<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };

    let mut key = vec![0_u8; DB_KEY_BYTES];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            key.as_mut_ptr(),
            key.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(DbError::FileRead(format!(
            "Windows CNG failed to generate database key material: NTSTATUS {status:#x}"
        )));
    }
    Ok(key)
}

#[cfg(not(windows))]
fn generate_key_material() -> DbResult<Vec<u8>> {
    Err(DbError::FileRead(
        "Encrypted file-backed databases require Windows CNG key generation in this phase."
            .to_string(),
    ))
}

/// Application-specific secondary entropy mixed into the DPAPI wrap of the
/// database key. It binds the wrapped key to *this app*, so a generic same-user
/// "try CryptUnprotectData on every .dpapi blob" sweep cannot recover it without
/// also knowing this value. MUST never change once shipped (it is required to
/// unwrap existing blobs); legacy no-entropy blobs are still accepted on read and
/// transparently re-wrapped with this entropy.
#[cfg(windows)]
const DPAPI_ENTROPY: &[u8] = b"code-hangar/db-key/v1";

#[cfg(windows)]
fn protect_key_material(key: &[u8]) -> DbResult<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: key.len() as u32,
        pbData: key.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: DPAPI_ENTROPY.len() as u32,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(DbError::FileRead(
            "Windows DPAPI failed to protect database key material.".to_string(),
        ));
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(protected)
}

#[cfg(not(windows))]
fn protect_key_material(_key: &[u8]) -> DbResult<Vec<u8>> {
    Err(DbError::FileRead(
        "Encrypted file-backed databases require Windows DPAPI in this phase.".to_string(),
    ))
}

#[cfg(windows)]
fn dpapi_unprotect(protected: &[u8], use_entropy: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: protected.len() as u32,
        pbData: protected.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: DPAPI_ENTROPY.len() as u32,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let entropy_ptr = if use_entropy {
        &entropy as *const CRYPT_INTEGER_BLOB
    } else {
        std::ptr::null()
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            entropy_ptr,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return None;
    }
    let key = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Some(key)
}

/// Returns `(key, was_legacy)`. Tries the app-specific entropy first; if that
/// fails, falls back to a legacy no-entropy blob (written by older builds) so
/// existing installs keep opening. `was_legacy = true` signals the caller to
/// re-wrap the blob with entropy.
#[cfg(windows)]
fn unprotect_key_material(protected: &[u8]) -> DbResult<(Vec<u8>, bool)> {
    if let Some(key) = dpapi_unprotect(protected, true) {
        return Ok((key, false));
    }
    if let Some(key) = dpapi_unprotect(protected, false) {
        return Ok((key, true));
    }
    Err(DbError::FileRead(
        "Windows DPAPI failed to unprotect database key material.".to_string(),
    ))
}

#[cfg(not(windows))]
fn unprotect_key_material(_protected: &[u8]) -> DbResult<(Vec<u8>, bool)> {
    Err(DbError::FileRead(
        "Encrypted file-backed databases require Windows DPAPI in this phase.".to_string(),
    ))
}

fn insert_default_zones(conn: &Connection) -> DbResult<()> {
    let zones = [
        ("glob", "**/.git/**", "no_preview", "builtin"),
        ("glob", "**/.ssh/**", "no_preview", "builtin"),
        ("glob", "**/.env", "no_preview", "builtin"),
        ("glob", "**/*credential*", "no_preview", "builtin"),
        ("glob", "**/*token*", "no_preview", "builtin"),
    ];

    for (pattern_type, pattern, level, source) in zones {
        conn.execute(
            "INSERT OR IGNORE INTO protected_zone(pattern_type, pattern, level, source)
             VALUES(?1, ?2, ?3, ?4)",
            params![pattern_type, pattern, level, source],
        )?;
    }
    Ok(())
}

fn seed_builtin_adapters(conn: &Connection) -> DbResult<()> {
    let adapters = [
        (
            "generic_markdown_context",
            "context",
            json!({
                "description": "Classifies local Markdown and agent context files without network access.",
                "patterns": ["README.md", "AGENTS.md", "docs/**/*.md", "prompts/**/*.md"],
                "relationships": ["markdown_links_to"]
            }),
        ),
        (
            "generic_git_project",
            "project_metadata",
            json!({
                "description": "Reads local .git metadata passively and never invokes remote Git commands.",
                "patterns": [".git/config", ".git/HEAD"],
                "relationships": ["belongs_to"]
            }),
        ),
        (
            "generic_model_workflow_assets",
            "asset_classifier",
            json!({
                "description": "Labels common local model, workflow and generated asset files for review.",
                "model_extensions": ["gguf", "safetensors", "ckpt", "onnx", "pt", "pth"],
                "workflow_extensions": ["json", "workflow.json"],
                "relationships": ["referenced_by", "stored_in"]
            }),
        ),
    ];
    for (name, adapter_type, definition) in adapters {
        conn.execute(
            "INSERT OR IGNORE INTO adapter(name, version, type, platforms, definition_json, source, enabled, schema_version)
             VALUES(?1, '1.0.0', ?2, ?3, ?4, 'builtin', 1, 1)",
            params![
                name,
                adapter_type,
                json!(["windows", "local"]).to_string(),
                definition.to_string()
            ],
        )?;
    }
    Ok(())
}

fn load_fixtures_if_empty(conn: &Connection) -> DbResult<()> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM node WHERE kind = 'project'",
        [],
        |row| row.get(0),
    )?;
    if count > 0 {
        return Ok(());
    }

    for fixture in fixture_projects() {
        let project_id = upsert_project(conn, fixture.root, fixture.name, "fixture")?;
        let files = fixture
            .files
            .iter()
            .map(|file| {
                let normalized = normalize_path(file.path);
                let is_sensitive = is_sensitive_path(&normalized);
                let protected_level = protected_level_for_path(&normalized);
                ScannedFile {
                    absolute_path: format!("fixture://{}/{}", fixture.id, normalized),
                    relative_path: normalized.clone(),
                    display_path: display_path_for_path(&normalized),
                    display_name: display_name_for_path(&normalized),
                    item_kind: "file".to_string(),
                    is_markdown: is_markdown_path(&normalized),
                    is_context: is_context_path(&normalized),
                    is_sensitive,
                    protected_level,
                    child_count: 0,
                    fully_scanned: true,
                    collapse_default: false,
                    scan_error: None,
                    identity: Some(FileIdentity {
                        size_apparent: Some(file.body.len() as u64),
                        size_allocated: None,
                        modified_at: None,
                        readonly: false,
                        is_symlink: false,
                        is_reparse: false,
                        reparse_kind: None,
                        volume_id: None,
                        inode_key: None,
                        link_count: None,
                        inaccessible: false,
                        error: None,
                    }),
                    body: (!is_sensitive && should_index_body(&normalized))
                        .then(|| file.body.to_string()),
                }
            })
            .collect::<Vec<_>>();
        insert_files_for_project(conn, project_id, &files)?;
        if fixture.id == "git-like-project" {
            insert_git_metadata(
                conn,
                project_id,
                Some(&GitRepoSummary {
                    project_id,
                    has_git: true,
                    current_branch: Some("main".to_string()),
                    head_ref: Some("ref: refs/heads/main".to_string()),
                    origin_url: Some("https://example.invalid/passive-only.git".to_string()),
                    metadata_error: None,
                }),
            )?;
        }
        recalculate_child_counts(conn, project_id)?;
        recalculate_nav_aggregates(conn, project_id)?;
        rebuild_project_markdown_edges(conn, project_id)?;
        rebuild_project_workflow_edges(conn, project_id)?;
    }
    Ok(())
}

fn upsert_project(conn: &Connection, path: &str, name: &str, source: &str) -> DbResult<i64> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM node WHERE kind = 'project' AND path = ?1",
            params![path],
            |row| row.get(0),
        )
        .optional()?
    {
        conn.execute(
            "UPDATE node SET name = ?2, attributes = ?3, last_seen_at = ?4, present = 1 WHERE id = ?1",
            params![id, name, json!({"source": source}).to_string(), now()],
        )?;
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO node(kind, path, name, attributes, first_seen_at, last_seen_at, present)
         VALUES('project', ?1, ?2, ?3, ?4, ?4, 1)",
        params![path, name, json!({"source": source}).to_string(), now()],
    )?;
    Ok(conn.last_insert_rowid())
}

fn rebuild_project_nav(conn: &Connection, project_id: i64) -> DbResult<()> {
    conn.execute(
        "DELETE FROM document_fts WHERE project_id = ?1",
        params![project_id],
    )?;
    conn.execute(
        "DELETE FROM document_index WHERE project_id = ?1",
        params![project_id],
    )?;
    conn.execute(
        "DELETE FROM git_repo WHERE project_id = ?1",
        params![project_id],
    )?;
    conn.execute(
        "UPDATE node SET present = 0, last_seen_at = ?2
         WHERE id IN (SELECT node_id FROM nav_item WHERE project_id = ?1 AND node_id IS NOT NULL)",
        params![project_id, now()],
    )?;
    conn.execute(
        "DELETE FROM nav_item WHERE project_id = ?1",
        params![project_id],
    )?;
    Ok(())
}

fn root_enabled(conn: &Connection, root_id: i64) -> DbResult<bool> {
    conn.query_row(
        "SELECT enabled FROM scan_root WHERE id = ?1",
        params![root_id],
        |row| Ok(row.get::<_, i64>(0)? == 1),
    )
    .optional()
    .map(|value| value.unwrap_or(false))
    .map_err(DbError::from)
}

fn finalizing_step(
    conn: &Connection,
    cancel: Option<&Arc<AtomicBool>>,
    message: &str,
    on_step: &mut impl FnMut(&str),
    f: impl FnOnce(&Connection) -> DbResult<()>,
) -> DbResult<()> {
    on_step(message);
    run_interruptible_step(conn, cancel, f)
}

fn recalculate_child_counts(conn: &Connection, project_id: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE nav_item
         SET child_count = (
           SELECT COUNT(*) FROM nav_item child WHERE child.project_id = ?1 AND child.parent_nav_id = nav_item.id
         )
         WHERE project_id = ?1",
        params![project_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
struct AggregateSource {
    node_id: Option<i64>,
    parent_nav_id: Option<i64>,
    size_apparent: u64,
    size_allocated: Option<u64>,
    volume_id: Option<String>,
    inode_key: Option<String>,
    partial: bool,
}

#[derive(Debug, Clone)]
struct NavAggregate {
    apparent_bytes: u64,
    allocated_bytes: Option<u64>,
    physical_bytes: u64,
    partial: bool,
}

#[derive(Debug, Default)]
struct FootprintAccumulator {
    apparent_bytes: u64,
    allocated_bytes: u64,
    has_allocated_bytes: bool,
    physical_bytes: u64,
    partial: bool,
    identities: HashMap<String, u64>,
}

impl FootprintAccumulator {
    fn add_source(&mut self, nav_id: i64, source: &AggregateSource) {
        self.apparent_bytes = self.apparent_bytes.saturating_add(source.size_apparent);
        if let Some(allocated) = source.size_allocated {
            self.allocated_bytes = self.allocated_bytes.saturating_add(allocated);
            self.has_allocated_bytes = true;
        }
        self.add_physical_identity(
            physical_identity_key(nav_id, source),
            source.size_allocated.unwrap_or(source.size_apparent),
        );
        self.partial |= source.partial;
    }

    fn merge(&mut self, child: FootprintAccumulator) {
        self.apparent_bytes = self.apparent_bytes.saturating_add(child.apparent_bytes);
        self.allocated_bytes = self.allocated_bytes.saturating_add(child.allocated_bytes);
        self.has_allocated_bytes |= child.has_allocated_bytes;
        self.partial |= child.partial;
        for (key, bytes) in child.identities {
            self.add_physical_identity(key, bytes);
        }
    }

    fn add_physical_identity(&mut self, key: String, bytes: u64) {
        match self.identities.get_mut(&key) {
            Some(existing) if bytes > *existing => {
                self.physical_bytes = self
                    .physical_bytes
                    .saturating_sub(*existing)
                    .saturating_add(bytes);
                *existing = bytes;
            }
            Some(_) => {}
            None => {
                self.identities.insert(key, bytes);
                self.physical_bytes = self.physical_bytes.saturating_add(bytes);
            }
        }
    }

    fn aggregate(&self) -> NavAggregate {
        NavAggregate {
            apparent_bytes: self.apparent_bytes,
            allocated_bytes: self.has_allocated_bytes.then_some(self.allocated_bytes),
            physical_bytes: self.physical_bytes,
            partial: self.partial,
        }
    }
}

fn physical_identity_key(nav_id: i64, source: &AggregateSource) -> String {
    match (&source.volume_id, &source.inode_key) {
        (Some(volume_id), Some(inode_key)) => format!("inode:{volume_id}:{inode_key}"),
        _ => source
            .node_id
            .map(|node_id| format!("node:{node_id}"))
            .unwrap_or_else(|| format!("nav:{nav_id}")),
    }
}

fn recalculate_nav_aggregates(conn: &Connection, project_id: i64) -> DbResult<()> {
    recalculate_nav_aggregates_interruptible(conn, project_id, None)
}

fn recalculate_nav_aggregates_interruptible(
    conn: &Connection,
    project_id: i64,
    cancel: Option<&AtomicBool>,
) -> DbResult<()> {
    recalculate_nav_aggregates_interruptible_timed(conn, project_id, cancel).map(|_| ())
}

fn recalculate_nav_aggregates_interruptible_timed(
    conn: &Connection,
    project_id: i64,
    cancel: Option<&AtomicBool>,
) -> DbResult<NavAggregateTiming> {
    check_cancelled(cancel)?;
    let select_started = Instant::now();
    let mut stmt = conn.prepare(
        "SELECT ni.id, ni.parent_nav_id, ni.node_id, ni.fully_scanned, ni.scan_error,
                n.size_apparent, n.size_allocated, n.volume_id, n.inode_key
         FROM nav_item ni
         LEFT JOIN node n ON n.id = ni.node_id
         WHERE ni.project_id = ?1",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        let id = row.get::<_, i64>(0)?;
        let parent_nav_id = row.get::<_, Option<i64>>(1)?;
        let fully_scanned = row.get::<_, i64>(3)? == 1;
        let scan_error = row.get::<_, Option<String>>(4)?;
        Ok((
            id,
            AggregateSource {
                node_id: row.get(2)?,
                parent_nav_id,
                size_apparent: row.get::<_, Option<i64>>(5)?.unwrap_or_default().max(0) as u64,
                size_allocated: row
                    .get::<_, Option<i64>>(6)?
                    .map(|value| value.max(0) as u64),
                volume_id: row.get(7)?,
                inode_key: row.get(8)?,
                partial: !fully_scanned || scan_error.is_some(),
            },
        ))
    })?;
    let mut sources = Vec::new();
    for row in rows {
        if sources.len() % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        sources.push(row?);
    }
    let select_ms = elapsed_ms(select_started);

    let compute_started = Instant::now();
    let mut records = HashMap::new();
    let mut children_by_parent: HashMap<Option<i64>, Vec<i64>> = HashMap::new();
    for (id, source) in sources {
        if records.len() % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        children_by_parent
            .entry(source.parent_nav_id)
            .or_default()
            .push(id);
        records.insert(id, source);
    }

    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    let mut results = HashMap::new();
    for id in children_by_parent.get(&None).cloned().unwrap_or_default() {
        check_cancelled(cancel)?;
        accumulate_nav_aggregate(
            id,
            &records,
            &children_by_parent,
            &mut results,
            &mut visited,
            &mut visiting,
        );
    }
    for id in records.keys().copied().collect::<Vec<_>>() {
        if !visited.contains(&id) {
            check_cancelled(cancel)?;
            accumulate_nav_aggregate(
                id,
                &records,
                &children_by_parent,
                &mut results,
                &mut visited,
                &mut visiting,
            );
        }
    }
    let compute_ms = elapsed_ms(compute_started);

    let update_started = Instant::now();
    let mut update = conn.prepare(
        "UPDATE nav_item
         SET aggregate_apparent_bytes = ?2,
             aggregate_allocated_bytes = ?3,
             aggregate_physical_bytes = ?4,
             aggregate_bytes_partial = ?5
         WHERE id = ?1",
    )?;
    for (id, aggregate) in results {
        if id % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        update.execute(params![
            id,
            u64_to_i64(aggregate.apparent_bytes),
            aggregate.allocated_bytes.map(u64_to_i64),
            u64_to_i64(aggregate.physical_bytes),
            bool_to_i64(aggregate.partial)
        ])?;
    }
    let update_ms = elapsed_ms(update_started);

    Ok(NavAggregateTiming {
        select_ms,
        compute_ms,
        update_ms,
    })
}

fn accumulate_nav_aggregate(
    id: i64,
    records: &HashMap<i64, AggregateSource>,
    children_by_parent: &HashMap<Option<i64>, Vec<i64>>,
    results: &mut HashMap<i64, NavAggregate>,
    visited: &mut HashSet<i64>,
    visiting: &mut HashSet<i64>,
) -> FootprintAccumulator {
    if visited.contains(&id) {
        return FootprintAccumulator::default();
    }
    if !visiting.insert(id) {
        return FootprintAccumulator {
            partial: true,
            ..Default::default()
        };
    }

    let mut accumulator = FootprintAccumulator::default();
    if let Some(source) = records.get(&id) {
        accumulator.add_source(id, source);
    }
    for child_id in children_by_parent
        .get(&Some(id))
        .cloned()
        .unwrap_or_default()
    {
        let child = accumulate_nav_aggregate(
            child_id,
            records,
            children_by_parent,
            results,
            visited,
            visiting,
        );
        accumulator.merge(child);
    }

    results.insert(id, accumulator.aggregate());
    visiting.remove(&id);
    visited.insert(id);
    accumulator
}

fn backfill_missing_nav_aggregates(conn: &Connection) -> DbResult<()> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT project_id
         FROM nav_item
         WHERE aggregate_apparent_bytes IS NULL
            OR aggregate_physical_bytes IS NULL",
    )?;
    let projects = collect_rows(stmt.query_map([], |row| row.get::<_, i64>(0))?)?;
    for project_id in projects {
        recalculate_child_counts(conn, project_id)?;
        recalculate_nav_aggregates(conn, project_id)?;
    }
    Ok(())
}

/// Reapply the current context policy to rows produced by older builds.
///
/// Context rules deliberately evolve as real inventories expose generated trees
/// that contain README files. Keeping those stale rows in FTS would make a fixed
/// scanner look broken until every project was manually rescanned. This pass is
/// bounded to rows already marked as context and only removes derived metadata.
fn reconcile_stale_context_classification(conn: &Connection) -> DbResult<()> {
    let stale_rows = {
        let mut stmt = conn.prepare(
            "SELECT id, node_id, path
             FROM nav_item
             WHERE is_context = 1 AND node_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        collect_rows(rows)?
            .into_iter()
            .filter(|(_, _, path)| !is_context_path(path))
            .collect::<Vec<_>>()
    };
    if stale_rows.is_empty() {
        return Ok(());
    }

    let mut affected_nodes = HashSet::new();
    for (nav_id, node_id, _) in stale_rows {
        conn.execute(
            "UPDATE nav_item SET is_context = 0, collapse_default = 1 WHERE id = ?1",
            params![nav_id],
        )?;
        affected_nodes.insert(node_id);
    }

    for node_id in affected_nodes {
        let remaining_context_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nav_item WHERE node_id = ?1 AND is_context = 1",
            params![node_id],
            |row| row.get(0),
        )?;
        if remaining_context_rows > 0 {
            continue;
        }
        conn.execute(
            "DELETE FROM document_fts WHERE node_id = ?1",
            params![node_id],
        )?;
        conn.execute(
            "DELETE FROM document_index WHERE node_id = ?1",
            params![node_id],
        )?;
        conn.execute(
            "DELETE FROM edge WHERE src = ?1 AND kind = 'markdown_links_to'",
            params![node_id],
        )?;
        conn.execute(
            "DELETE FROM relationship_issue
             WHERE node_id = ?1
               AND kind IN ('unresolved_markdown_link', 'ambiguous_markdown_link')",
            params![node_id],
        )?;
    }
    Ok(())
}

fn stale_disk_summary(conn: &Connection) -> DbResult<String> {
    let fingerprinted = count_i64(
        conn,
        "SELECT COUNT(*) FROM node
         WHERE kind = 'file'
           AND present = 1
           AND path NOT LIKE 'fixture://%'
           AND mtime IS NOT NULL
           AND size_apparent IS NOT NULL",
    )?;
    if fingerprinted == 0 {
        return Ok(
            "No live disk check needed for fixture data or projects without file fingerprints."
                .to_string(),
        );
    }

    let roots_needing_scan = count_i64(
        conn,
        "SELECT COUNT(*)
         FROM scan_root
         WHERE enabled = 1 AND adhoc = 0 AND last_scanned_at IS NULL",
    )?;

    Ok(if roots_needing_scan > 0 {
        format!(
            "{fingerprinted} fingerprinted files. {roots_needing_scan} enabled scan root(s) have not completed a scan yet."
        )
    } else {
        format!(
            "{fingerprinted} fingerprinted files. Live disk checks are deferred to explicit rescan to keep startup responsive."
        )
    })
}

fn adapter_description(definition_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(definition_json)
        .ok()
        .and_then(|value| {
            value
                .get("description")
                .and_then(|description| description.as_str())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "Local built-in adapter.".to_string())
}

fn project_footprint_summaries(
    conn: &Connection,
    limit: usize,
    include_fixture_projects: bool,
) -> DbResult<Vec<ProjectFootprintSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path
         FROM node p
         WHERE p.kind = 'project'
           AND p.present = 1
           AND (?1 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
    )?;
    let projects = collect_rows(stmt.query_map(
        params![include_fixture_projects as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?)?;

    let mut summaries = Vec::with_capacity(projects.len());
    for (project_id, name, path) in projects {
        let mut items = conn.prepare(
            "SELECT ni.id, ni.node_id, ni.item_kind, ni.fully_scanned, ni.scan_error,
                    ni.aggregate_bytes_partial,
                    COALESCE(ni.aggregate_apparent_bytes, n.size_apparent, 0),
                    COALESCE(ni.aggregate_allocated_bytes, n.size_allocated),
                    COALESCE(ni.aggregate_physical_bytes, n.size_allocated, n.size_apparent, 0),
                    n.volume_id, n.inode_key
             FROM nav_item ni
             LEFT JOIN node n ON n.id = ni.node_id
             WHERE ni.project_id = ?1 AND ni.parent_nav_id IS NULL",
        )?;
        let rows = items.query_map(params![project_id], |row| {
            let fully_scanned = row.get::<_, i64>(3)? == 1;
            let scan_error = row.get::<_, Option<String>>(4)?;
            let aggregate_partial = row.get::<_, i64>(5)? == 1;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(6)?.unwrap_or_default().max(0) as u64,
                row.get::<_, Option<i64>>(7)?
                    .map(|value| value.max(0) as u64),
                row.get::<_, Option<i64>>(8)?.unwrap_or_default().max(0) as u64,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                aggregate_partial || !fully_scanned || scan_error.is_some(),
            ))
        })?;
        let mut apparent_bytes = 0_u64;
        let mut allocated_bytes = 0_u64;
        let mut has_allocated_bytes = false;
        let mut physical_bytes = 0_u64;
        let mut partial = false;
        let mut physical_identities: HashMap<String, u64> = HashMap::new();
        for row in rows {
            let (
                nav_id,
                node_id,
                item_kind,
                apparent,
                allocated,
                physical,
                volume_id,
                inode_key,
                row_partial,
            ) = row?;
            apparent_bytes = apparent_bytes.saturating_add(apparent);
            if let Some(allocated) = allocated {
                allocated_bytes = allocated_bytes.saturating_add(allocated);
                has_allocated_bytes = true;
            }
            let identity = if item_kind == "file" {
                match (volume_id, inode_key) {
                    (Some(volume_id), Some(inode_key)) => format!("inode:{volume_id}:{inode_key}"),
                    _ => node_id
                        .map(|node_id| format!("node:{node_id}"))
                        .unwrap_or_else(|| format!("nav:{nav_id}")),
                }
            } else {
                format!("nav:{nav_id}")
            };
            match physical_identities.get_mut(&identity) {
                Some(existing) if physical > *existing => {
                    physical_bytes = physical_bytes
                        .saturating_sub(*existing)
                        .saturating_add(physical);
                    *existing = physical;
                }
                Some(_) => {}
                None => {
                    physical_identities.insert(identity, physical);
                    physical_bytes = physical_bytes.saturating_add(physical);
                }
            }
            partial |= row_partial;
        }
        summaries.push(ProjectFootprintSummary {
            project_id,
            name,
            path,
            apparent_bytes,
            allocated_bytes: has_allocated_bytes.then_some(allocated_bytes),
            physical_bytes: Some(physical_bytes),
            footprint_partial: partial,
        });
    }

    summaries.sort_by(|left, right| {
        right
            .apparent_bytes
            .cmp(&left.apparent_bytes)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    summaries.truncate(limit);
    Ok(summaries)
}

fn rebuild_project_markdown_edges(conn: &Connection, project_id: i64) -> DbResult<()> {
    rebuild_project_markdown_edges_interruptible(conn, project_id, None)
}

fn rebuild_project_markdown_edges_interruptible(
    conn: &Connection,
    project_id: i64,
    cancel: Option<&AtomicBool>,
) -> DbResult<()> {
    check_cancelled(cancel)?;
    conn.execute(
        "DELETE FROM edge
         WHERE kind = 'markdown_links_to'
           AND (
             src NOT IN (SELECT node_id FROM nav_item WHERE node_id IS NOT NULL)
             OR dst NOT IN (SELECT node_id FROM nav_item WHERE node_id IS NOT NULL)
           )",
        [],
    )?;
    check_cancelled(cancel)?;
    conn.execute(
        "DELETE FROM edge
         WHERE kind = 'markdown_links_to'
           AND src IN (
             SELECT node_id FROM nav_item
             WHERE project_id = ?1 AND node_id IS NOT NULL
           )",
        params![project_id],
    )?;
    check_cancelled(cancel)?;
    conn.execute(
        "DELETE FROM relationship_issue
         WHERE project_id = ?1
           AND kind IN ('unresolved_markdown_link', 'ambiguous_markdown_link')",
        params![project_id],
    )?;
    check_cancelled(cancel)?;

    let mut docs = conn.prepare(
        "SELECT di.node_id, ni.path, di.links_json
         FROM document_index di
         JOIN nav_item ni ON ni.node_id = di.node_id
         WHERE di.project_id = ?1
           AND ni.project_id = ?1",
    )?;
    let rows = docs.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?
                .unwrap_or_else(|| "[]".to_string()),
        ))
    })?;
    let docs = collect_rows(rows)?;

    let mut find_target = conn.prepare(
        "SELECT node_id FROM nav_item
         WHERE project_id = ?1 AND path = ?2 AND node_id IS NOT NULL
         LIMIT 1",
    )?;
    let mut find_bare_targets = conn.prepare(
        "SELECT node_id FROM nav_item
         WHERE project_id = ?1
           AND display_name = ?2
           AND node_id IS NOT NULL
         ORDER BY priority, sort_key, id
         LIMIT 10",
    )?;
    let mut insert_edge = conn.prepare(
        "INSERT OR REPLACE INTO edge(src, dst, kind, confidence, evidence)
         VALUES(?1, ?2, 'markdown_links_to', ?3, ?4)",
    )?;
    let mut insert_issue = conn.prepare(
        "INSERT OR REPLACE INTO relationship_issue(node_id, project_id, kind, confidence, target, evidence)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    for (doc_index, (source_node_id, source_path, links_json)) in docs.into_iter().enumerate() {
        if doc_index % 128 == 0 {
            check_cancelled(cancel)?;
        }
        let links: Vec<MarkdownLink> = serde_json::from_str(&links_json).unwrap_or_default();
        for (link_index, link) in links.into_iter().filter(|link| !link.is_remote).enumerate() {
            if link_index % 128 == 0 {
                check_cancelled(cancel)?;
            }
            if is_same_file_anchor(&link.target) {
                continue;
            }
            let evidence = markdown_link_evidence(&link);
            let Some(target_path) = resolve_relative_path(&source_path, &link.target) else {
                if should_record_unresolved_markdown_link(&link.target) {
                    let confidence = unresolved_markdown_confidence(&link.target);
                    insert_issue.execute(params![
                        source_node_id,
                        project_id,
                        "unresolved_markdown_link",
                        confidence,
                        link.target,
                        evidence
                    ])?;
                }
                continue;
            };

            if let Some(target_node_id) = find_target
                .query_row(params![project_id, target_path], |row| row.get::<_, i64>(0))
                .optional()?
            {
                if target_node_id != source_node_id {
                    insert_edge.execute(params![
                        source_node_id,
                        target_node_id,
                        "High",
                        evidence
                    ])?;
                }
                continue;
            }

            if let Some(bare_name) = bare_markdown_name(&link.target) {
                let rows = find_bare_targets
                    .query_map(params![project_id, bare_name], |row| row.get::<_, i64>(0))?;
                let target_node_ids = collect_rows(rows)?
                    .into_iter()
                    .filter(|target_node_id| *target_node_id != source_node_id)
                    .collect::<Vec<_>>();
                match target_node_ids.len() {
                    0 => {
                        insert_issue.execute(params![
                            source_node_id,
                            project_id,
                            "unresolved_markdown_link",
                            "Medium",
                            link.target,
                            evidence
                        ])?;
                    }
                    1 => {
                        insert_edge.execute(params![
                            source_node_id,
                            target_node_ids[0],
                            "Medium",
                            evidence
                        ])?;
                    }
                    _ => {
                        insert_issue.execute(params![
                            source_node_id,
                            project_id,
                            "ambiguous_markdown_link",
                            "Low",
                            link.target,
                            evidence
                        ])?;
                        for target_node_id in target_node_ids {
                            insert_edge.execute(params![
                                source_node_id,
                                target_node_id,
                                "Low",
                                evidence
                            ])?;
                        }
                    }
                }
            } else {
                let confidence = unresolved_markdown_confidence(&link.target);
                insert_issue.execute(params![
                    source_node_id,
                    project_id,
                    "unresolved_markdown_link",
                    confidence,
                    link.target,
                    evidence
                ])?;
            }
        }
    }

    Ok(())
}

fn backfill_missing_markdown_edges(conn: &Connection) -> DbResult<()> {
    if setting_value(conn, MARKDOWN_EDGE_BACKFILL_SETTING)?.is_some() {
        return Ok(());
    }

    let edge_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM edge WHERE kind = 'markdown_links_to'",
        [],
        |row| row.get(0),
    )?;
    let issue_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM relationship_issue
         WHERE kind IN ('unresolved_markdown_link', 'ambiguous_markdown_link')",
        [],
        |row| row.get(0),
    )?;
    if edge_count > 0 || issue_count > 0 {
        set_setting(conn, MARKDOWN_EDGE_BACKFILL_SETTING, "complete")?;
        return Ok(());
    }

    let document_count = count_i64(
        conn,
        "SELECT COUNT(*) FROM document_index WHERE project_id IS NOT NULL",
    )?;
    if document_count == 0 {
        set_setting(conn, MARKDOWN_EDGE_BACKFILL_SETTING, "complete")?;
        return Ok(());
    }
    if document_count > MAX_STARTUP_MARKDOWN_BACKFILL_DOCS {
        set_setting(conn, MARKDOWN_EDGE_BACKFILL_SETTING, "deferred-large")?;
        return Ok(());
    }

    let mut stmt = conn.prepare(
        "SELECT DISTINCT project_id
         FROM document_index
         WHERE project_id IS NOT NULL",
    )?;
    let project_ids = collect_rows(stmt.query_map([], |row| row.get::<_, i64>(0))?)?;
    for project_id in project_ids {
        rebuild_project_markdown_edges(conn, project_id)?;
    }
    set_setting(conn, MARKDOWN_EDGE_BACKFILL_SETTING, "complete")?;
    Ok(())
}

#[derive(Debug, Clone)]
struct ModelGraphCandidate {
    node_id: i64,
    absolute_path: String,
    relative_path: String,
}

fn rebuild_project_workflow_edges(conn: &Connection, project_id: i64) -> DbResult<()> {
    rebuild_project_workflow_edges_interruptible(conn, project_id, None)
}

fn rebuild_project_workflow_edges_interruptible(
    conn: &Connection,
    project_id: i64,
    cancel: Option<&AtomicBool>,
) -> DbResult<()> {
    check_cancelled(cancel)?;
    conn.execute(
        "DELETE FROM edge
         WHERE kind = 'workflow_references_model'
           AND src IN (
             SELECT node_id FROM nav_item
             WHERE project_id = ?1 AND node_id IS NOT NULL
           )",
        params![project_id],
    )?;
    conn.execute(
        "DELETE FROM relationship_issue
         WHERE project_id = ?1
           AND kind IN ('missing_model_reference', 'ambiguous_model_reference', 'workflow_parse_error')",
        params![project_id],
    )?;

    let models = load_model_graph_candidates(conn)?;
    let model_index = ModelNameIndex::build(&models);
    let mut workflows = conn.prepare(
        "SELECT ni.node_id, ni.path, n.path, n.size_apparent, n.attributes
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         WHERE ni.project_id = ?1
           AND ni.node_id IS NOT NULL
           AND ni.item_kind = 'file'
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND n.present = 1
           AND n.is_reparse = 0
           AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
           AND COALESCE(n.size_apparent, 0) <= ?2
           AND (
             lower(ni.path) LIKE '%.workflow.json'
             OR lower(ni.path) LIKE '%/workflow.json'
             OR (
               lower(ni.path) LIKE '%.json'
               AND (
                 lower(ni.path) LIKE 'workflows/%'
                 OR lower(ni.path) LIKE 'workflow/%'
                 OR lower(ni.path) LIKE '%/workflows/%'
                 OR lower(ni.path) LIKE '%/workflow/%'
               )
             )
           )
         ORDER BY ni.path",
    )?;
    let rows = workflows.query_map(params![project_id, MAX_WORKFLOW_JSON_BYTES as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?.unwrap_or_default().max(0) as u64,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    let workflows = collect_rows(rows)?;

    let mut insert_edge = conn.prepare(
        "INSERT OR REPLACE INTO edge(src, dst, kind, confidence, evidence)
         VALUES(?1, ?2, 'workflow_references_model', ?3, ?4)",
    )?;
    let mut insert_issue = conn.prepare(
        "INSERT OR REPLACE INTO relationship_issue(node_id, project_id, kind, confidence, target, evidence)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    for (index, (node_id, relative_path, absolute_path, size, attributes)) in
        workflows.into_iter().enumerate()
    {
        if index % 32 == 0 {
            check_cancelled(cancel)?;
        }
        if !is_workflow_candidate_path(&relative_path) {
            continue;
        }
        if size > MAX_WORKFLOW_JSON_BYTES {
            // Too large to analyze: record an issue so the workflow's model references are
            // known-uncertain rather than silently absent (which would make a model it references
            // look like a safe-to-delete orphan). The orphan query consults this below.
            insert_issue.execute(params![
                node_id,
                project_id,
                "workflow_unparsed",
                "Low",
                relative_path,
                "workflow file exceeds the analysis size limit"
            ])?;
            continue;
        }
        let Some(bytes) = read_workflow_json_bytes(&absolute_path, attributes.as_deref())? else {
            insert_issue.execute(params![
                node_id,
                project_id,
                "workflow_unparsed",
                "Low",
                relative_path,
                "workflow file could not be read (grew past the size limit or is inaccessible)"
            ])?;
            continue;
        };
        let references = match extract_workflow_model_references(&bytes) {
            Ok(references) => references,
            Err(error) => {
                insert_issue.execute(params![
                    node_id,
                    project_id,
                    "workflow_parse_error",
                    "Low",
                    relative_path,
                    error.to_string()
                ])?;
                continue;
            }
        };
        for reference in references {
            check_cancelled(cancel)?;
            let matches = resolve_model_reference(&reference, &model_index);
            match matches.as_slice() {
                [] => {
                    insert_issue.execute(params![
                        node_id,
                        project_id,
                        "missing_model_reference",
                        "Medium",
                        reference.target,
                        reference.evidence
                    ])?;
                }
                [(target_node_id, confidence)] => {
                    if *target_node_id != node_id {
                        insert_edge.execute(params![
                            node_id,
                            target_node_id,
                            confidence,
                            reference.evidence
                        ])?;
                    }
                }
                _ => {
                    insert_issue.execute(params![
                        node_id,
                        project_id,
                        "ambiguous_model_reference",
                        "Low",
                        reference.target,
                        reference.evidence.clone()
                    ])?;
                    for (target_node_id, _) in matches {
                        if target_node_id != node_id {
                            insert_edge.execute(params![
                                node_id,
                                target_node_id,
                                "Low",
                                reference.evidence
                            ])?;
                        }
                    }
                }
            }
        }
    }
    set_setting(conn, &workflow_graph_setting_key(project_id), &now())?;
    Ok(())
}

fn workflow_graph_setting_key(project_id: i64) -> String {
    // v3 excludes dependency trees and CI metadata from the model-workflow graph.
    // A versioned key makes the next scan clear stale parse issues from the older
    // broad `/workflows/` classifier without changing Safe Manage evidence inputs.
    format!("graph.workflow.v3.project.{project_id}")
}

fn load_model_graph_candidates(conn: &Connection) -> DbResult<Vec<ModelGraphCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT ni.node_id, n.path, ni.path
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         WHERE ni.node_id IS NOT NULL
           AND ni.item_kind = 'file'
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND n.present = 1
           AND n.is_reparse = 0
           AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
           AND (
             lower(ni.path) LIKE '%.safetensors'
             OR lower(ni.path) LIKE '%.ckpt'
             OR lower(ni.path) LIKE '%.pt'
             OR lower(ni.path) LIKE '%.pth'
             OR lower(ni.path) LIKE '%.gguf'
             OR lower(ni.path) LIKE '%.onnx'
             OR lower(ni.path) LIKE '%.bin'
             OR lower(ni.path) LIKE '%.engine'
             OR lower(ni.path) LIKE '%.tflite'
           )
         ORDER BY ni.node_id, ni.project_id, ni.path",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ModelGraphCandidate {
            node_id: row.get(0)?,
            absolute_path: row.get(1)?,
            relative_path: row.get(2)?,
        })
    })?;
    Ok(collect_rows(rows)?
        .into_iter()
        .filter(|candidate| is_model_path(&candidate.relative_path))
        .collect())
}

fn read_workflow_json_bytes(
    absolute_path: &str,
    attributes: Option<&str>,
) -> DbResult<Option<Vec<u8>>> {
    if absolute_path.starts_with("fixture://") {
        let body = attributes
            .and_then(|attributes| serde_json::from_str::<serde_json::Value>(attributes).ok())
            .and_then(|attributes| {
                attributes
                    .get("body")
                    .and_then(|body| body.as_str())
                    .map(str::to_owned)
            });
        return Ok(body.map(String::into_bytes));
    }
    let file = match fs::File::open(absolute_path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::new();
    file.take(MAX_WORKFLOW_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    if bytes.len() as u64 > MAX_WORKFLOW_JSON_BYTES {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// Name/stem index over the (DB-wide) model candidates so each workflow reference
/// resolves in O(matches) instead of a full scan. Every model that can satisfy a
/// reference — by exact/suffix path, file name, or stem — shares the reference's file
/// name or stem, so these two maps cover all candidates exactly while preserving the
/// original cross-project resolution semantics.
struct ModelNameIndex<'a> {
    models: &'a [ModelGraphCandidate],
    by_name: HashMap<String, Vec<usize>>,
    by_stem: HashMap<String, Vec<usize>>,
}

impl<'a> ModelNameIndex<'a> {
    fn build(models: &'a [ModelGraphCandidate]) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, model) in models.iter().enumerate() {
            let display_name = graph_file_name(&normalized_graph_path(&model.relative_path));
            let stem = graph_file_stem(&display_name);
            by_name.entry(display_name).or_default().push(index);
            if !stem.is_empty() {
                by_stem.entry(stem).or_default().push(index);
            }
        }
        Self {
            models,
            by_name,
            by_stem,
        }
    }

    fn candidate_indices(&self, target_name: &str, target_stem: &str) -> Vec<usize> {
        let mut indices = Vec::new();
        let mut seen = HashSet::new();
        for list in [self.by_name.get(target_name), self.by_stem.get(target_stem)]
            .into_iter()
            .flatten()
        {
            for &index in list {
                if seen.insert(index) {
                    indices.push(index);
                }
            }
        }
        indices
    }
}

fn resolve_model_reference(
    reference: &WorkflowReference,
    index: &ModelNameIndex,
) -> Vec<(i64, &'static str)> {
    let target = normalized_graph_path(&reference.target);
    let target_name = graph_file_name(&target);
    let target_stem = graph_file_stem(&target_name);
    let mut exact = Vec::new();
    let mut named = Vec::new();
    let mut seen_exact = HashSet::new();
    let mut seen_named = HashSet::new();

    for candidate_index in index.candidate_indices(&target_name, &target_stem) {
        let model = &index.models[candidate_index];
        let absolute = normalized_graph_path(&model.absolute_path);
        let relative = normalized_graph_path(&model.relative_path);
        let display_name = graph_file_name(&relative);
        let target_has_path = target.contains('/');
        let direct = absolute == target
            || relative == target
            || (target_has_path
                && (absolute.ends_with(&format!("/{target}"))
                    || relative.ends_with(&format!("/{target}"))));
        if direct && seen_exact.insert(model.node_id) {
            exact.push((model.node_id, "High"));
            continue;
        }
        let file_name_match = display_name == target_name;
        let stem_match = !target_stem.is_empty()
            && !target_name.contains('.')
            && graph_file_stem(&display_name) == target_stem;
        if (file_name_match || stem_match) && seen_named.insert(model.node_id) {
            named.push((model.node_id, "Medium"));
        }
    }
    if exact.is_empty() {
        named
    } else {
        exact
    }
}

fn normalized_graph_path(path: &str) -> String {
    normalize_path(path)
        .trim_start_matches("//?/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn graph_file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn graph_file_stem(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(path)
        .to_string()
}

fn load_relationships(
    conn: &Connection,
    node_id: i64,
    outgoing: bool,
) -> DbResult<Vec<NodeRelationship>> {
    let (other_column, filter_column) = if outgoing {
        ("e.dst", "e.src")
    } else {
        ("e.src", "e.dst")
    };
    let sql = format!(
        "SELECT other.node_id, other.project_id, other.path, other.display_name,
                other.item_kind, e.kind, e.confidence, e.evidence
         FROM edge e
         JOIN nav_item other ON other.node_id = {other_column}
         WHERE {filter_column} = ?1
           AND other.node_id IS NOT NULL
         ORDER BY other.priority, other.sort_key, other.id
         LIMIT 50"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![node_id], |row| {
        Ok(NodeRelationship {
            node_id: row.get(0)?,
            project_id: row.get(1)?,
            path: row.get(2)?,
            display_name: row.get(3)?,
            item_kind: row.get(4)?,
            kind: row.get(5)?,
            confidence: row.get(6)?,
            evidence: row.get(7)?,
        })
    })?;
    collect_rows(rows)
}

fn load_relationship_issues(conn: &Connection, node_id: i64) -> DbResult<Vec<RelationshipIssue>> {
    let mut stmt = conn.prepare(
        "SELECT node_id, project_id, kind, confidence, target, evidence
         FROM relationship_issue
         WHERE node_id = ?1
         ORDER BY confidence, target
         LIMIT 50",
    )?;
    let rows = stmt.query_map(params![node_id], |row| {
        Ok(RelationshipIssue {
            node_id: row.get(0)?,
            project_id: row.get(1)?,
            kind: row.get(2)?,
            confidence: row.get(3)?,
            target: row.get(4)?,
            evidence: row.get(5)?,
        })
    })?;
    collect_rows(rows)
}

const MAX_PROJECT_GRAPH_MAP_NODES: usize = 50_000;

fn project_graph_map_limit(requested: usize) -> usize {
    requested.clamp(25, MAX_PROJECT_GRAPH_MAP_NODES)
}

fn load_project_graph_map(conn: &Connection, project_id: i64, limit: usize) -> DbResult<GraphMap> {
    // The UI starts at 300 and can explicitly expand in paused batches. Keep a
    // high defensive ceiling for local IPC/MCP callers without silently forcing
    // every project to stop at the old 1,000-node boundary.
    let limit = project_graph_map_limit(limit);
    let project = conn
        .query_row(
            "SELECT id, path, name FROM node WHERE id = ?1 AND kind = 'project' AND present = 1",
            params![project_id],
            |row| {
                Ok(GraphNode {
                    node_id: row.get(0)?,
                    project_id,
                    path: row.get(1)?,
                    display_name: row.get(2)?,
                    item_kind: "project".to_string(),
                    graph_kind: "project".to_string(),
                    confidence: "High".to_string(),
                    details: Vec::new(),
                    physical_bytes: None,
                    protected_or_sensitive: false,
                    shared_project_ids: vec![project_id],
                })
            },
        )
        .optional()?
        .ok_or_else(|| DbError::FileRead(format!("Project {project_id} was not found.")))?;

    let mut nodes = vec![project];
    let mut node_ids = HashSet::from([project_id]);
    let mut graph_node_paths = HashMap::<i64, String>::new();
    let mut stmt = conn.prepare(
        "SELECT ni.node_id, ni.project_id, ni.path, n.path, ni.display_name, ni.item_kind,
                COALESCE(ni.aggregate_physical_bytes, n.size_allocated, n.size_apparent),
                ni.is_sensitive, ni.protected_level, n.is_reparse, n.reparse_kind
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         WHERE ni.project_id = ?1
           AND ni.node_id IS NOT NULL
           AND n.present = 1
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND (
             lower(ni.path) LIKE '%.safetensors'
             OR lower(ni.path) LIKE '%.ckpt'
             OR lower(ni.path) LIKE '%.pt'
             OR lower(ni.path) LIKE '%.pth'
             OR lower(ni.path) LIKE '%.gguf'
             OR lower(ni.path) LIKE '%.onnx'
             OR lower(ni.path) LIKE '%.bin'
             OR lower(ni.path) LIKE '%.engine'
             OR lower(ni.path) LIKE '%.tflite'
             OR lower(ni.path) LIKE '%.workflow.json'
             OR lower(ni.path) LIKE '%/workflow.json'
             OR (
               lower(ni.path) LIKE '%.json'
               AND (
                 lower(ni.path) LIKE 'workflows/%'
                 OR lower(ni.path) LIKE 'workflow/%'
                 OR lower(ni.path) LIKE '%/workflows/%'
                 OR lower(ni.path) LIKE '%/workflow/%'
               )
             )
             OR (ni.item_kind = 'directory' AND (
               lower(ni.path) LIKE '%/.cache'
               OR lower(ni.path) LIKE '%/cache'
               OR lower(ni.path) LIKE '%/.cache/huggingface%'
               OR lower(ni.path) LIKE '%/.cache/transformers%'
               OR lower(ni.path) LIKE '%/huggingface'
               OR lower(ni.path) LIKE '%/huggingface/hub%'
               OR lower(ni.path) LIKE '%/transformers'
               OR lower(ni.path) LIKE '%/ollama/models'
               OR lower(ni.path) LIKE '%/ollama/models/%'
               OR lower(ni.path) LIKE '%/.ollama/models'
               OR lower(ni.path) LIKE '%/.ollama/models/%'
             ))
           )
         ORDER BY ni.priority, ni.sort_key, ni.id",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<i64>>(6)?
                .map(|value| value.max(0) as u64),
            row.get::<_, i64>(7)? == 1,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, i64>(9)? == 1,
            row.get::<_, Option<String>>(10)?,
        ))
    })?;
    let rows = collect_rows(rows)?;
    drop(stmt);
    for row in rows {
        let (
            node_id,
            owner_project_id,
            path,
            absolute_path,
            display_name,
            item_kind,
            physical_bytes,
            is_sensitive,
            protected_level,
            is_reparse,
            reparse_kind,
        ) = row;
        let graph_kind = if is_model_path(&path) {
            format!("model:{}", model_category(&path))
        } else if is_workflow_candidate_path(&path) {
            "workflow".to_string()
        } else if is_cache_path(&path, &item_kind) {
            "cache".to_string()
        } else {
            continue;
        };
        if node_ids.insert(node_id) {
            // Never open reparse points / cloud placeholders for a header read — that
            // would hydrate a dehydrated file (SECURITY_INVARIANTS.md). The node stays
            // visible in the map; only its on-disk header read is suppressed.
            let header_safe = !is_reparse && reparse_kind.as_deref() != Some("cloud_placeholder");
            if header_safe {
                graph_node_paths.insert(node_id, absolute_path);
            }
            nodes.push(GraphNode {
                node_id,
                project_id: owner_project_id,
                path,
                display_name,
                item_kind,
                graph_kind,
                confidence: "High".to_string(),
                details: Vec::new(),
                physical_bytes,
                protected_or_sensitive: is_sensitive || protected_level.is_some(),
                shared_project_ids: load_node_project_ids(conn, node_id)?,
            });
        }
    }

    let mut edge_stmt = conn.prepare(
        "SELECT e.src, e.dst, e.kind, e.confidence, e.evidence
         FROM edge e
         WHERE e.kind = 'workflow_references_model'
           AND (
             e.src IN (SELECT node_id FROM nav_item WHERE project_id = ?1 AND node_id IS NOT NULL)
             OR e.dst IN (SELECT node_id FROM nav_item WHERE project_id = ?1 AND node_id IS NOT NULL)
           )
         ORDER BY e.src, e.dst",
    )?;
    let edge_rows = edge_stmt.query_map(params![project_id], |row| {
        Ok(GraphEdge {
            source_node_id: row.get(0)?,
            target_node_id: row.get(1)?,
            kind: row.get(2)?,
            confidence: row.get(3)?,
            evidence: row.get(4)?,
        })
    })?;
    let mut graph_edges = collect_rows(edge_rows)?;
    let visible_model_ids = nodes
        .iter()
        .filter(|node| node.graph_kind.starts_with("model:"))
        .map(|node| node.node_id)
        .collect::<HashSet<_>>();
    let (duplicate_edges, duplicate_issues) =
        load_duplicate_model_graph_warnings(conn, &visible_model_ids)?;
    graph_edges.extend(duplicate_edges);
    for edge in &graph_edges {
        for endpoint in [edge.source_node_id, edge.target_node_id] {
            if node_ids.contains(&endpoint) {
                continue;
            }
            if let Some(node) = load_graph_node(conn, endpoint)? {
                node_ids.insert(endpoint);
                nodes.push(node);
            }
        }
    }

    let mut edges = nodes
        .iter()
        .filter(|node| node.node_id != project_id && node.project_id == project_id)
        .map(|node| GraphEdge {
            source_node_id: project_id,
            target_node_id: node.node_id,
            kind: "project_contains".to_string(),
            confidence: "High".to_string(),
            evidence: None,
        })
        .collect::<Vec<_>>();
    edges.extend(graph_edges);

    let mut issue_stmt = conn.prepare(
        "SELECT ri.node_id, ri.project_id,
                (SELECT ni.path FROM nav_item ni
                 WHERE ni.node_id = ri.node_id AND ni.project_id = ri.project_id
                 ORDER BY ni.id LIMIT 1),
                ri.kind, ri.confidence, ri.target, ri.evidence
         FROM relationship_issue ri
         WHERE ri.project_id = ?1
           AND ri.kind IN ('missing_model_reference', 'ambiguous_model_reference', 'workflow_parse_error')
         ORDER BY ri.confidence, ri.target",
    )?;
    let issue_rows = issue_stmt.query_map(params![project_id], |row| {
        Ok(GraphIssue {
            node_id: row.get(0)?,
            project_id: row.get(1)?,
            source_path: row.get(2)?,
            kind: row.get(3)?,
            confidence: row.get(4)?,
            target: row.get(5)?,
            evidence: row.get(6)?,
        })
    })?;
    let mut issues = collect_rows(issue_rows)?;
    issues.extend(duplicate_issues);
    let total_nodes = nodes.len() as i64;
    let total_edges = edges.len() as i64;
    let partial = conn.query_row(
        "SELECT COUNT(*) FROM nav_item
         WHERE project_id = ?1 AND (fully_scanned = 0 OR scan_error IS NOT NULL)",
        params![project_id],
        |row| row.get::<_, i64>(0),
    )? > 0;

    nodes.truncate(limit);
    for node in &mut nodes {
        if node.graph_kind.starts_with("model:") {
            if let Some(absolute_path) = graph_node_paths.get(&node.node_id) {
                node.details = load_model_header_details(absolute_path, &node.path);
            }
        } else if node.graph_kind == "cache" {
            node.details = load_cache_graph_details(node);
        }
    }
    let cache_issues = load_shared_cache_graph_warnings(&nodes);
    issues.extend(cache_issues);
    let total_issues = issues.len() as i64;
    let retained = nodes
        .iter()
        .map(|node| node.node_id)
        .collect::<HashSet<_>>();
    edges.retain(|edge| {
        retained.contains(&edge.source_node_id) && retained.contains(&edge.target_node_id)
    });
    Ok(GraphMap {
        project_id,
        nodes,
        edges,
        issues,
        total_nodes,
        total_edges,
        total_issues,
        partial,
    })
}

fn load_graph_node(conn: &Connection, node_id: i64) -> DbResult<Option<GraphNode>> {
    let mut node = conn
        .query_row(
            "SELECT ni.node_id, ni.project_id, ni.path, n.path, ni.display_name, ni.item_kind,
                COALESCE(ni.aggregate_physical_bytes, n.size_allocated, n.size_apparent),
                ni.is_sensitive, ni.protected_level, n.is_reparse, n.reparse_kind
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         WHERE ni.node_id = ?1 AND n.present = 1
         ORDER BY ni.project_id, ni.id
         LIMIT 1",
            params![node_id],
            |row| {
                let path = row.get::<_, String>(2)?;
                let absolute_path = row.get::<_, String>(3)?;
                let item_kind = row.get::<_, String>(5)?;
                let is_sensitive = row.get::<_, i64>(7)? == 1;
                let protected_level = row.get::<_, Option<String>>(8)?;
                let is_reparse = row.get::<_, i64>(9)? == 1;
                let reparse_kind = row.get::<_, Option<String>>(10)?;
                let graph_kind = if is_model_path(&path) {
                    format!("model:{}", model_category(&path))
                } else if is_workflow_candidate_path(&path) {
                    "workflow".to_string()
                } else if is_cache_path(&path, &item_kind) {
                    "cache".to_string()
                } else {
                    "asset".to_string()
                };
                // Never open a sensitive / protected / reparse / cloud-placeholder file
                // for a header read (SECURITY_INVARIANTS.md). The node still resolves so
                // its relationships render; only the on-disk header read is suppressed.
                let header_safe = !is_sensitive
                    && protected_level.is_none()
                    && !is_reparse
                    && reparse_kind.as_deref() != Some("cloud_placeholder");
                let details = if is_model_path(&path) && header_safe {
                    load_model_header_details(&absolute_path, &path)
                } else {
                    Vec::new()
                };
                Ok(GraphNode {
                    node_id: row.get(0)?,
                    project_id: row.get(1)?,
                    path,
                    display_name: row.get(4)?,
                    item_kind,
                    graph_kind,
                    confidence: "High".to_string(),
                    details,
                    physical_bytes: row
                        .get::<_, Option<i64>>(6)?
                        .map(|value| value.max(0) as u64),
                    protected_or_sensitive: is_sensitive || protected_level.is_some(),
                    shared_project_ids: Vec::new(),
                })
            },
        )
        .optional()?;
    if let Some(node) = node.as_mut() {
        node.shared_project_ids = load_node_project_ids(conn, node.node_id)?;
        if node.graph_kind == "cache" {
            node.details = load_cache_graph_details(node);
        }
    }
    Ok(node)
}

fn load_node_project_ids(conn: &Connection, node_id: i64) -> DbResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT project_id FROM nav_item WHERE node_id = ?1 ORDER BY project_id",
    )?;
    let rows = stmt.query_map(params![node_id], |row| row.get::<_, i64>(0))?;
    collect_rows(rows)
}

fn load_cache_graph_details(node: &GraphNode) -> Vec<String> {
    let Some(category) = cache_category(&node.path, &node.item_kind) else {
        return Vec::new();
    };
    let mut details = vec![cache_category_label(category).to_string()];
    if cache_category_is_shared_by_default(category) {
        details.push("Usually shared by multiple projects or tools".to_string());
    }
    if node.shared_project_ids.len() > 1 {
        details.push(format!(
            "Inventoried by {} registered projects",
            node.shared_project_ids.len()
        ));
    }
    details
}

fn load_shared_cache_graph_warnings(nodes: &[GraphNode]) -> Vec<GraphIssue> {
    nodes
        .iter()
        .filter(|node| node.graph_kind == "cache")
        .filter_map(|node| {
            let category = cache_category(&node.path, &node.item_kind)?;
            let default_shared = cache_category_is_shared_by_default(category);
            let multi_project = node.shared_project_ids.len() > 1;
            if !default_shared && !multi_project {
                return None;
            }
            let reason = if default_shared && multi_project {
                format!(
                    "{} is normally shared and is currently inventoried by {} registered projects.",
                    cache_category_label(category),
                    node.shared_project_ids.len()
                )
            } else if default_shared {
                format!(
                    "{} is normally a shared tool/model cache, even if only one registered project currently references it.",
                    cache_category_label(category)
                )
            } else {
                format!(
                    "This cache folder is inventoried by {} registered projects.",
                    node.shared_project_ids.len()
                )
            };
            Some(GraphIssue {
                node_id: node.node_id,
                project_id: Some(node.project_id),
                source_path: Some(node.path.clone()),
                kind: "shared_cache_candidate".to_string(),
                confidence: if multi_project {
                    "High".to_string()
                } else {
                    "Medium".to_string()
                },
                target: node.path.clone(),
                evidence: Some(format!(
                    "{reason} Treat it as shared until ownership is reviewed."
                )),
            })
        })
        .collect()
}

fn load_model_header_details(absolute_path: &str, display_path: &str) -> Vec<String> {
    let Some(probe_bytes) = model_header_probe_bytes(display_path) else {
        return Vec::new();
    };
    let Ok(mut file) = fs::File::open(absolute_path) else {
        return Vec::new();
    };
    // Read UP TO probe_bytes. `read_exact` on the fixed buffer would reject any file
    // shorter than the probe window — fatal for the 256 KiB GGUF probe on small or
    // quantized models. A short read is fine: each summarizer validates its own
    // minimum length (GGUF needs >= 24 bytes, safetensors reads its 8-byte length next).
    let mut prefix = Vec::new();
    if file
        .by_ref()
        .take(probe_bytes as u64)
        .read_to_end(&mut prefix)
        .is_err()
    {
        return Vec::new();
    }
    let extension = Path::new(&display_path.replace('\\', "/"))
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "gguf" {
        return summarize_gguf_header(&prefix)
            .map(|summary| summary.details)
            .unwrap_or_default();
    }
    if extension != "safetensors" {
        return Vec::new();
    }
    let header_len = match safetensors_header_len(&prefix) {
        Ok(length) => length,
        Err(ModelHeaderError::HeaderTooLarge(_)) => {
            return vec![
                "Safetensors header exceeds the 4 MiB safety cap; summary skipped.".to_string(),
            ];
        }
        Err(_) => return Vec::new(),
    };
    let Ok(header_len) = usize::try_from(header_len) else {
        return Vec::new();
    };
    let mut header = vec![0u8; header_len];
    if file.read_exact(&mut header).is_err() {
        return Vec::new();
    }
    summarize_safetensors_header(&header)
        .map(|summary| summary.details)
        .unwrap_or_default()
}

fn load_duplicate_model_graph_warnings(
    conn: &Connection,
    visible_model_ids: &HashSet<i64>,
) -> DbResult<(Vec<GraphEdge>, Vec<GraphIssue>)> {
    if visible_model_ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let visible_ids = visible_model_ids.iter().copied().collect::<Vec<_>>();

    // Collect candidate sizes for the visible models. The id set is NOT bounded by the
    // graph node limit, so chunk it under SQLITE_MAX_VARIABLE_NUMBER (bundled SQLCipher),
    // per the .chunks(500) convention used elsewhere in this module. The cloud_placeholder
    // guard joins is_reparse=0 because a dehydrated file has is_reparse=0 yet must never
    // be opened/hashed (it would hydrate it).
    let mut size_set = HashSet::<i64>::new();
    for chunk in visible_ids.chunks(SQL_IN_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut size_stmt = conn.prepare(&format!(
            "SELECT DISTINCT size_apparent
             FROM node
             WHERE id IN ({placeholders})
               AND present = 1
               AND is_reparse = 0
               AND COALESCE(reparse_kind, '') <> 'cloud_placeholder'
               AND size_apparent IS NOT NULL
               AND size_apparent >= ?"
        ))?;
        let mut size_params = chunk.to_vec();
        size_params.push(DUPLICATE_MIN_SIZE_BYTES as i64);
        for size in collect_rows(
            size_stmt.query_map(params_from_iter(size_params), |row| row.get::<_, i64>(0))?,
        )? {
            size_set.insert(size);
        }
    }
    if size_set.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let sizes = size_set.into_iter().collect::<Vec<_>>();

    // Fetch the same-size model candidates, chunking the (also unbounded) size list too.
    let mut candidate_rows = Vec::new();
    for chunk in sizes.chunks(SQL_IN_CHUNK) {
        let size_placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT ni.node_id, ni.project_id, p.name AS project_name, ni.path,
                    ni.display_name, n.path AS absolute_path, n.size_apparent,
                    COALESCE(n.size_allocated, n.size_apparent) AS physical_bytes,
                    ni.aggregate_bytes_partial, n.volume_id, n.inode_key, n.attributes
             FROM nav_item ni
             JOIN node n ON n.id = ni.node_id
             JOIN node p ON p.id = ni.project_id
             WHERE ni.node_id IS NOT NULL
               AND ni.item_kind = 'file'
               AND ni.is_sensitive = 0
               AND ni.protected_level IS NULL
               AND n.present = 1
               AND n.is_reparse = 0
               AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
               AND n.size_apparent IN ({size_placeholders})
               AND (
                 lower(ni.path) LIKE '%.safetensors'
                 OR lower(ni.path) LIKE '%.ckpt'
                 OR lower(ni.path) LIKE '%.pt'
                 OR lower(ni.path) LIKE '%.pth'
                 OR lower(ni.path) LIKE '%.gguf'
                 OR lower(ni.path) LIKE '%.onnx'
                 OR lower(ni.path) LIKE '%.bin'
                 OR lower(ni.path) LIKE '%.engine'
                 OR lower(ni.path) LIKE '%.tflite'
               )
             ORDER BY n.size_apparent DESC, ni.project_id, ni.path
             LIMIT 5000"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.to_vec()), |row| {
            let volume_id = row.get::<_, Option<String>>(9)?;
            let inode_key = row.get::<_, Option<String>>(10)?;
            Ok(DuplicateCandidateRow {
                node_id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                path: row.get(3)?,
                display_name: row.get(4)?,
                absolute_path: row.get(5)?,
                size_bytes: row.get::<_, i64>(6)?.max(0) as u64,
                physical_bytes: row
                    .get::<_, Option<i64>>(7)?
                    .map(|value| value.max(0) as u64),
                footprint_partial: row.get::<_, i64>(8)? == 1,
                identity_key: volume_id
                    .zip(inode_key)
                    .map(|(volume, inode)| format!("{volume}:{inode}")),
                attributes: row.get(11)?,
            })
        })?;
        for row in collect_rows(rows)? {
            candidate_rows.push(row);
        }
    }
    // Re-apply the original global cap (largest sizes first) across the chunked fetch.
    candidate_rows.sort_by(|left, right| {
        right
            .size_bytes
            .cmp(&left.size_bytes)
            .then_with(|| left.project_id.cmp(&right.project_id))
            .then_with(|| left.path.cmp(&right.path))
    });
    candidate_rows.truncate(5000);

    let mut by_size: HashMap<u64, Vec<DuplicateCandidateRow>> = HashMap::new();
    for row in dedupe_duplicate_candidate_rows(candidate_rows, None) {
        if is_model_path(&row.path) {
            by_size.entry(row.size_bytes).or_default().push(row);
        }
    }

    let mut edges = Vec::new();
    let mut issues = Vec::new();
    let mut seen_edges = HashSet::<(i64, i64)>::new();
    for (_size, same_size) in by_size {
        if same_size.len() < 2 {
            continue;
        }
        let mut by_hash: HashMap<String, Vec<DuplicateCandidateRow>> = HashMap::new();
        for row in same_size {
            if let Some(hash) = partial_hash_for_candidate(&row) {
                by_hash.entry(hash).or_default().push(row);
            }
        }
        for (_hash, mut members) in by_hash {
            if members.len() < 2 || !has_distinct_physical_members(&members) {
                continue;
            }
            if !members
                .iter()
                .any(|member| visible_model_ids.contains(&member.node_id))
            {
                continue;
            }
            members.sort_by(|left, right| {
                visible_model_ids
                    .contains(&right.node_id)
                    .cmp(&visible_model_ids.contains(&left.node_id))
                    .then_with(|| left.project_id.cmp(&right.project_id))
                    .then_with(|| left.path.cmp(&right.path))
            });
            let member_count = members.len();
            let evidence = format!(
                "{member_count} model files share {} bytes and the first {} KiB hash. Full hash confirmation is deferred.",
                members[0].size_bytes,
                DUPLICATE_PARTIAL_HASH_BYTES / 1024
            );
            for member in members
                .iter()
                .filter(|member| visible_model_ids.contains(&member.node_id))
                .take(16)
            {
                issues.push(GraphIssue {
                    node_id: member.node_id,
                    project_id: Some(member.project_id),
                    source_path: Some(member.path.clone()),
                    kind: "duplicate_model_candidate".to_string(),
                    confidence: "Medium".to_string(),
                    target: format!("{member_count} model candidates"),
                    evidence: Some(evidence.clone()),
                });
            }
            let edge_members = members.iter().take(20).collect::<Vec<_>>();
            for (index, left) in edge_members.iter().enumerate() {
                for right in edge_members.iter().skip(index + 1) {
                    if !visible_model_ids.contains(&left.node_id)
                        && !visible_model_ids.contains(&right.node_id)
                    {
                        continue;
                    }
                    let (source, target) = if left.node_id <= right.node_id {
                        (left.node_id, right.node_id)
                    } else {
                        (right.node_id, left.node_id)
                    };
                    if seen_edges.insert((source, target)) {
                        edges.push(GraphEdge {
                            source_node_id: source,
                            target_node_id: target,
                            kind: "duplicate_model_candidate".to_string(),
                            confidence: "Medium".to_string(),
                            evidence: Some(evidence.clone()),
                        });
                    }
                }
            }
        }
    }

    Ok((edges, issues))
}

fn load_orphan_asset_candidates(
    conn: &Connection,
    options: &OrphanAssetSearchOptions<'_>,
) -> DbResult<OrphanCandidates> {
    let limit = options.limit;
    let min_size_bytes = options.min_size_bytes.unwrap_or(0);
    let project_id = options.project_id;
    let asset_kind = options.asset_kind.unwrap_or("all");
    let min_confidence = options.min_confidence.unwrap_or("Low");
    let include_partial = options.include_partial;
    let include_fixture_projects = options.include_fixture_projects;
    // Push the requested asset kind's extension set into SQL (when it is a specific kind) so the
    // scan walks only files of that kind instead of every file in the inventory. Literal,
    // code-defined extensions — no user input — so direct interpolation is safe. Rust's
    // `classify_orphan_candidate` still runs on each surviving row, so this only narrows the scan.
    let ext_filter = match asset_kind_extensions(asset_kind) {
        Some(exts) => {
            let likes = exts
                .iter()
                .map(|ext| format!("lower(ni.path) LIKE '%.{ext}'"))
                .collect::<Vec<_>>()
                .join(" OR ");
            format!("AND ({likes})")
        }
        None => String::new(),
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT ni.node_id, ni.project_id, p.name, ni.path, ni.display_name,
                COALESCE(ni.aggregate_physical_bytes, n.size_allocated, n.size_apparent, 0),
                ni.aggregate_bytes_partial
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         JOIN node p ON p.id = ni.project_id
         WHERE ni.node_id IS NOT NULL
           AND ni.item_kind = 'file'
           AND ni.is_context = 0
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND n.present = 1
           AND COALESCE(ni.aggregate_physical_bytes, n.size_allocated, n.size_apparent, 0) >= ?1
           AND (?2 IS NULL OR ni.project_id = ?2)
           AND (?3 = 1 OR ni.aggregate_bytes_partial = 0)
           AND (?4 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')
           {ext_filter}
           AND NOT EXISTS (
             SELECT 1 FROM edge e
             WHERE e.dst = ni.node_id
           )
           -- A model file is NOT a confident orphan when its project has a workflow we could not
           -- analyze (too large / unreadable): that workflow may reference this model, so deleting
           -- it could break a real dependency. Only models are protected this way; other orphans
           -- (logs, caches) are unaffected.
           AND NOT (
             EXISTS (
               SELECT 1 FROM relationship_issue ri
               WHERE ri.project_id = ni.project_id
                 AND ri.kind IN ('workflow_unparsed', 'workflow_parse_error')
             )
             AND (
               lower(ni.path) LIKE '%.safetensors' OR lower(ni.path) LIKE '%.ckpt'
               OR lower(ni.path) LIKE '%.pt' OR lower(ni.path) LIKE '%.pth'
               OR lower(ni.path) LIKE '%.gguf' OR lower(ni.path) LIKE '%.onnx'
               OR lower(ni.path) LIKE '%.bin' OR lower(ni.path) LIKE '%.engine'
               OR lower(ni.path) LIKE '%.tflite'
             )
           )
         ORDER BY ni.project_id, ni.path
         LIMIT 5000"
    ))?;
    let rows = stmt.query_map(
        params![
            min_size_bytes as i64,
            project_id,
            include_partial as i64,
            include_fixture_projects
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?
                    .map(|value| value.max(0) as u64),
                row.get::<_, i64>(6)? == 1,
            ))
        },
    )?;

    let mut candidates = Vec::new();
    for row in rows {
        let (node_id, project_id, project_name, path, display_name, physical_bytes, partial) = row?;
        let Some((candidate_kind, confidence, reason)) = classify_orphan_candidate(&path) else {
            continue;
        };
        if !asset_kind_matches(asset_kind, &candidate_kind) {
            continue;
        }
        if confidence_rank(&confidence) < confidence_rank(min_confidence) {
            continue;
        }
        candidates.push(OrphanCandidate {
            node_id,
            project_id,
            project_name,
            path,
            display_name,
            confidence,
            reason,
            physical_bytes,
            footprint_partial: partial,
        });
    }
    let total = candidates.len() as i64;
    candidates.truncate(limit);
    Ok(OrphanCandidates { candidates, total })
}

fn load_node_orphan_status(conn: &Connection, node_id: i64) -> DbResult<OrphanStatus> {
    let record = conn
        .query_row(
            "SELECT ni.node_id, ni.path, ni.item_kind, ni.is_context, ni.is_sensitive,
                    ni.protected_level, COALESCE(ni.aggregate_physical_bytes, n.size_allocated, n.size_apparent, 0),
                    ni.aggregate_bytes_partial, n.present
             FROM nav_item ni
             JOIN node n ON n.id = ni.node_id
             WHERE ni.node_id = ?1
             LIMIT 1",
            params![node_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? == 1,
                    row.get::<_, i64>(4)? == 1,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?.map(|value| value.max(0) as u64),
                    row.get::<_, i64>(7)? == 1,
                    row.get::<_, i64>(8)? == 1,
                ))
            },
        )
        .optional()?;

    let Some((
        node_id,
        path,
        item_kind,
        is_context,
        is_sensitive,
        protected_level,
        physical_bytes,
        footprint_partial,
        present,
    )) = record
    else {
        return Ok(OrphanStatus {
            node_id,
            evaluated: false,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some("File is not in the current inventory.".to_string()),
            incoming_references: 0,
            protected_or_sensitive: false,
            physical_bytes: None,
            footprint_partial: false,
        });
    };

    let incoming_references: i64 = conn.query_row(
        "SELECT COUNT(*) FROM edge WHERE dst = ?1",
        params![node_id],
        |row| row.get(0),
    )?;
    let protected_or_sensitive = is_sensitive || protected_level.is_some();
    if !present {
        return Ok(OrphanStatus {
            node_id,
            evaluated: true,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some("File is no longer present in the latest inventory.".to_string()),
            incoming_references,
            protected_or_sensitive,
            physical_bytes,
            footprint_partial,
        });
    }
    if item_kind != "file" {
        return Ok(OrphanStatus {
            node_id,
            evaluated: true,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some("Only files are evaluated as unreferenced asset candidates.".to_string()),
            incoming_references,
            protected_or_sensitive,
            physical_bytes,
            footprint_partial,
        });
    }
    if protected_or_sensitive {
        return Ok(OrphanStatus {
            node_id,
            evaluated: true,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some(
                "Sensitive or protected files are excluded from orphan candidate searches."
                    .to_string(),
            ),
            incoming_references,
            protected_or_sensitive,
            physical_bytes,
            footprint_partial,
        });
    }
    if is_context {
        return Ok(OrphanStatus {
            node_id,
            evaluated: true,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some("Priority context files are not treated as orphan assets.".to_string()),
            incoming_references,
            protected_or_sensitive,
            physical_bytes,
            footprint_partial,
        });
    }
    if incoming_references > 0 {
        return Ok(OrphanStatus {
            node_id,
            evaluated: true,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some("This file has known local references in the inventory.".to_string()),
            incoming_references,
            protected_or_sensitive,
            physical_bytes,
            footprint_partial,
        });
    }

    let Some((candidate_kind, confidence, reason)) = classify_orphan_candidate(&path) else {
        return Ok(OrphanStatus {
            node_id,
            evaluated: true,
            is_candidate: false,
            candidate_kind: None,
            confidence: None,
            reason: Some(
                "This file type is not currently classified as a reviewable orphan asset."
                    .to_string(),
            ),
            incoming_references,
            protected_or_sensitive,
            physical_bytes,
            footprint_partial,
        });
    };

    Ok(OrphanStatus {
        node_id,
        evaluated: true,
        is_candidate: true,
        candidate_kind: Some(candidate_kind),
        confidence: Some(confidence),
        reason: Some(reason),
        incoming_references,
        protected_or_sensitive,
        physical_bytes,
        footprint_partial,
    })
}

fn classify_orphan_candidate(path: &str) -> Option<(String, String, String)> {
    let normalized = normalize_path(path).to_ascii_lowercase();
    if normalized.split('/').any(|part| {
        matches!(
            part,
            ".git"
                | ".ssh"
                | "node_modules"
                | ".venv"
                | "venv"
                | "site-packages"
                | "dist-packages"
                | "target"
                | "dist"
                | "build"
                | ".cache"
                | "__pycache__"
        )
    }) {
        return None;
    }
    let extension = Path::new(&normalized)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let (asset_kind, asset_reason) = match extension {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => {
            ("image", "Unreferenced image asset")
        }
        "mp4" | "mov" | "avi" | "mkv" | "webm" => ("video", "Unreferenced video asset"),
        "wav" | "mp3" | "flac" | "ogg" => ("media", "Unreferenced audio asset"),
        "safetensors" | "ckpt" | "gguf" | "onnx" | "pt" | "pth" => {
            ("model", "Unreferenced model-like asset")
        }
        "zip" | "7z" | "tar" | "gz" | "parquet" | "csv" | "bin" | "dat" => {
            ("data", "Unreferenced data/archive asset")
        }
        _ => return None,
    };
    let confidence = if normalized.contains("unused") || normalized.contains("orphan") {
        "Medium"
    } else {
        "Low"
    };
    Some((
        asset_kind.to_string(),
        confidence.to_string(),
        asset_reason.to_string(),
    ))
}

fn asset_kind_matches(filter: &str, candidate_kind: &str) -> bool {
    match filter {
        "all" | "" => true,
        "media" => matches!(candidate_kind, "image" | "video" | "media"),
        "datasets" | "archives" | "data" => candidate_kind == "data",
        "models" => candidate_kind == "model",
        other => candidate_kind == other,
    }
}

/// File extensions whose `classify_orphan_candidate` kind satisfies `asset_kind_matches(filter, …)`.
/// Returns `None` for the unrestricted "all" filter (and for any filter we don't recognise, which
/// then keeps the previous behaviour). Used to push the orphan-scan extension filter down into SQL:
/// a kind-scoped scan (e.g. the Organize "Models" tab) otherwise sorts and subquery-checks every
/// file in the inventory — hundreds of thousands of `.venv`/`node_modules` rows that Rust then
/// discards in `classify_orphan_candidate` — which made the scan take minutes on a model-heavy disk.
/// The lists mirror the `classify_orphan_candidate` match arms exactly, so the SQL pre-filter never
/// drops a row Rust would have kept.
fn asset_kind_extensions(filter: &str) -> Option<&'static [&'static str]> {
    const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "ico"];
    const VIDEO: &[&str] = &["mp4", "mov", "avi", "mkv", "webm"];
    const MODEL: &[&str] = &["safetensors", "ckpt", "gguf", "onnx", "pt", "pth"];
    const DATA: &[&str] = &["zip", "7z", "tar", "gz", "parquet", "csv", "bin", "dat"];
    const MEDIA: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "mp4", "mov", "avi", "mkv", "webm",
        "wav", "mp3", "flac", "ogg",
    ];
    match filter {
        "media" => Some(MEDIA),
        "datasets" | "archives" | "data" => Some(DATA),
        "models" | "model" => Some(MODEL),
        "image" => Some(IMAGE),
        "video" => Some(VIDEO),
        // "all", "" and anything unrecognised keep the original unrestricted scan.
        _ => None,
    }
}

fn file_asset_kind(path: &str) -> String {
    let normalized = normalize_path(path).to_ascii_lowercase();
    let extension = Path::new(&normalized)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    match extension {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => "image",
        "mp4" | "mov" | "avi" | "mkv" | "webm" => "video",
        "wav" | "mp3" | "flac" | "ogg" => "media",
        "safetensors" | "ckpt" | "gguf" | "onnx" | "pt" | "pth" => "model",
        "zip" | "7z" | "tar" | "gz" | "parquet" | "csv" | "bin" | "dat" => "data",
        _ => "other",
    }
    .to_string()
}

fn confidence_rank(confidence: &str) -> i32 {
    match confidence.to_ascii_lowercase().as_str() {
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn load_lost_project_candidates(
    conn: &Connection,
    options: LostProjectLoadOptions<'_>,
) -> DbResult<LostProjectCandidates> {
    let mut candidates = Vec::new();
    let keyword = options.keyword.trim().to_ascii_lowercase();
    let mut project_stmt = conn.prepare(
        "SELECT p.id, COALESCE(p.name, p.path, 'Project'), COALESCE(p.path, ''),
                COALESCE(SUM(COALESCE(ni.aggregate_apparent_bytes, 0)), 0),
                COALESCE(SUM(COALESCE(ni.aggregate_physical_bytes, ni.aggregate_allocated_bytes, ni.aggregate_apparent_bytes, 0)), 0),
                MAX(COALESCE(ni.aggregate_bytes_partial, 0))
         FROM node p
         LEFT JOIN nav_item ni ON ni.project_id = p.id AND ni.parent_nav_id IS NULL
         WHERE p.kind = 'project'
           AND (?1 IS NULL OR p.id = ?1)
           AND (?2 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')
         GROUP BY p.id, p.name, p.path",
    )?;
    let project_rows = project_stmt.query_map(
        params![options.project_id, options.include_fixture_projects],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?.max(0) as u64,
                row.get::<_, i64>(4)?.max(0) as u64,
                row.get::<_, i64>(5)? == 1,
            ))
        },
    )?;
    for row in project_rows {
        let (id, name, path, apparent, physical, partial) = row?;
        if physical < options.min_size_bytes || (!options.include_partial && partial) {
            continue;
        }
        if !keyword.is_empty() && !lost_project_keyword_matches(&name, &path, &keyword) {
            continue;
        }
        let mut signals = lost_project_signals(conn, id, None, &name, &path)?;
        if partial {
            signals.push("partial_inventory".to_string());
        }
        if !keyword.is_empty() {
            signals.push("keyword_match".to_string());
        }
        if !lost_project_signal_filter_matches(
            options.stale_preset,
            options.requested_signals,
            &signals,
        ) {
            continue;
        }
        let confidence = lost_project_confidence(&signals);
        candidates.push(LostProjectCandidate {
            project_id: id,
            node_id: Some(id),
            nav_id: None,
            candidate_kind: "project".to_string(),
            display_name: name,
            path,
            confidence: confidence.clone(),
            reason: lost_project_reason(&signals, &confidence, "project"),
            signals,
            apparent_bytes: apparent,
            physical_bytes: Some(physical),
            footprint_partial: partial,
        });
    }

    let mut dir_stmt = conn.prepare(
        "SELECT ni.id, ni.project_id, ni.display_name, ni.path,
                COALESCE(ni.aggregate_apparent_bytes, 0),
                COALESCE(ni.aggregate_physical_bytes, ni.aggregate_allocated_bytes, ni.aggregate_apparent_bytes, 0),
                ni.aggregate_bytes_partial, ni.node_id
         FROM nav_item ni
         JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
         WHERE ni.item_kind = 'directory'
           AND (?1 IS NULL OR ni.project_id = ?1)
           AND COALESCE(ni.aggregate_physical_bytes, ni.aggregate_allocated_bytes, ni.aggregate_apparent_bytes, 0) >= ?2
           AND (?3 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')
         ORDER BY COALESCE(ni.aggregate_physical_bytes, ni.aggregate_allocated_bytes, ni.aggregate_apparent_bytes, 0) DESC
         LIMIT 5000",
    )?;
    let dir_rows = dir_stmt.query_map(
        params![
            options.project_id,
            options.min_size_bytes as i64,
            options.include_fixture_projects
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?.max(0) as u64,
                row.get::<_, i64>(5)?.max(0) as u64,
                row.get::<_, i64>(6)? == 1,
                row.get::<_, Option<i64>>(7)?,
            ))
        },
    )?;
    for row in dir_rows {
        let (nav_id, candidate_project_id, name, path, apparent, physical, partial, node_id) = row?;
        if !options.include_partial && partial {
            continue;
        }
        if !keyword.is_empty() && !lost_project_keyword_matches(&name, &path, &keyword) {
            continue;
        }
        let mut signals =
            lost_project_signals(conn, candidate_project_id, Some(&path), &name, &path)?;
        if partial {
            signals.push("partial_inventory".to_string());
        }
        if !keyword.is_empty() {
            signals.push("keyword_match".to_string());
        }
        if !lost_project_signal_filter_matches(
            options.stale_preset,
            options.requested_signals,
            &signals,
        ) {
            continue;
        }
        let confidence = lost_project_confidence(&signals);
        candidates.push(LostProjectCandidate {
            project_id: candidate_project_id,
            node_id,
            nav_id: Some(nav_id),
            candidate_kind: "folder".to_string(),
            display_name: name,
            path,
            confidence: confidence.clone(),
            reason: lost_project_reason(&signals, &confidence, "folder"),
            signals,
            apparent_bytes: apparent,
            physical_bytes: Some(physical),
            footprint_partial: partial,
        });
    }

    candidates.sort_by(|left, right| {
        confidence_rank(&right.confidence)
            .cmp(&confidence_rank(&left.confidence))
            .then_with(|| {
                right
                    .physical_bytes
                    .unwrap_or(0)
                    .cmp(&left.physical_bytes.unwrap_or(0))
            })
            .then_with(|| left.path.cmp(&right.path))
    });
    let total = candidates.len() as i64;
    candidates.truncate(options.limit);
    Ok(LostProjectCandidates { candidates, total })
}

fn lost_project_keyword_matches(name: &str, path: &str, keyword: &str) -> bool {
    format!("{name} {path}")
        .to_ascii_lowercase()
        .contains(keyword)
}

fn lost_project_signals(
    conn: &Connection,
    project_id: i64,
    path_prefix: Option<&str>,
    name: &str,
    path: &str,
) -> DbResult<Vec<String>> {
    let mut signals = Vec::new();
    let recent_count: i64 = if let Some(prefix) = path_prefix {
        let pattern = format!("{prefix}/%");
        conn.query_row(
            "SELECT COUNT(*) FROM recent_item r
             JOIN nav_item ni ON ni.node_id = r.node_id
             WHERE ni.project_id = ?1 AND (ni.path = ?2 OR ni.path LIKE ?3)",
            params![project_id, prefix, pattern],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM recent_item WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?
    };
    if recent_count == 0 {
        signals.push("no_recent_opens".to_string());
    }

    let context_count: i64 = if let Some(prefix) = path_prefix {
        let pattern = format!("{prefix}/%");
        conn.query_row(
            "SELECT COUNT(*) FROM nav_item
             WHERE project_id = ?1 AND is_context = 1 AND (path = ?2 OR path LIKE ?3)",
            params![project_id, prefix, pattern],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM nav_item WHERE project_id = ?1 AND is_context = 1",
            params![project_id],
            |row| row.get(0),
        )?
    };
    if context_count == 0 {
        signals.push("no_context".to_string());
    }

    let git_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM git_repo WHERE project_id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    if git_count == 0 {
        signals.push("git_absent".to_string());
    }

    if has_lost_project_name_marker(name, path) {
        signals.push("name_markers".to_string());
    }
    Ok(signals)
}

/// Matches whole alphabetic tokens, not substrings, so common names like
/// "latest", "gold" or "contest" do not falsely trigger on "test"/"old".
fn has_lost_project_name_marker(name: &str, path: &str) -> bool {
    const MARKERS: [&str; 5] = ["old", "draft", "test", "unused", "archive"];
    format!("{name}/{path}")
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|token| MARKERS.contains(&token))
}

fn lost_project_signal_filter_matches(
    stale_preset: &str,
    requested_signals: &[String],
    signals: &[String],
) -> bool {
    let has_requested = requested_signals.is_empty()
        || requested_signals
            .iter()
            .all(|requested| signals.iter().any(|signal| signal == requested));
    let stale_matches = match stale_preset {
        "quiet" | "forgotten" => signals.iter().any(|signal| signal == "no_recent_opens"),
        "unfinished" => signals
            .iter()
            .any(|signal| signal == "no_context" || signal == "name_markers"),
        "untracked" => signals.iter().any(|signal| signal == "git_absent"),
        "custom" => {
            !requested_signals.is_empty() || signals.iter().any(|signal| signal == "keyword_match")
        }
        "suspicious" => signals.len() >= 2,
        _ => true,
    };
    has_requested && stale_matches && !signals.is_empty()
}

fn lost_project_confidence(signals: &[String]) -> String {
    if signals.len() >= 3 {
        "High".to_string()
    } else if signals.len() >= 2 {
        "Medium".to_string()
    } else {
        "Low".to_string()
    }
}

fn lost_project_reason(signals: &[String], confidence: &str, candidate_kind: &str) -> String {
    format!(
        "Passive {candidate_kind}-review signal rated {confidence} based on {}.",
        signals
            .iter()
            .map(|signal| lost_project_signal_label(signal))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn lost_project_signal_label(signal: &str) -> &'static str {
    match signal {
        "no_recent_opens" => "no recent opens",
        "no_context" => "no priority context files",
        "git_absent" => "no local Git metadata",
        "name_markers" => "old/draft/test/archive naming",
        "partial_inventory" => "partial inventory",
        "keyword_match" => "keyword match",
        _ => "passive signal",
    }
}

#[derive(Debug, Clone)]
struct DuplicateCandidateRow {
    node_id: i64,
    project_id: i64,
    project_name: String,
    path: String,
    display_name: String,
    absolute_path: String,
    size_bytes: u64,
    physical_bytes: Option<u64>,
    footprint_partial: bool,
    identity_key: Option<String>,
    attributes: Option<String>,
}

fn load_duplicate_candidates(
    conn: &Connection,
    limit: usize,
    min_size_bytes: u64,
    project_id: Option<i64>,
    file_kind: &str,
    include_fixture_projects: bool,
) -> DbResult<DuplicateCandidates> {
    let mut stmt = conn.prepare(
        "WITH candidate AS (
           SELECT ni.node_id, ni.project_id, p.name AS project_name, ni.path,
                  ni.display_name, n.path AS absolute_path, n.size_apparent,
                  COALESCE(n.size_allocated, n.size_apparent) AS physical_bytes,
                  ni.aggregate_bytes_partial, n.volume_id, n.inode_key, n.attributes
           FROM nav_item ni
           JOIN node n ON n.id = ni.node_id
           JOIN node p ON p.id = ni.project_id
           WHERE ni.node_id IS NOT NULL
             AND ni.item_kind = 'file'
             AND ni.is_sensitive = 0
             AND ni.protected_level IS NULL
             AND n.present = 1
             AND n.is_reparse = 0
             AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
             AND n.size_apparent IS NOT NULL
             AND n.size_apparent >= ?1
             AND (?2 IS NULL OR ni.project_id = ?2)
             AND (?3 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')
         ),
         duplicate_size AS (
           SELECT size_apparent
           FROM candidate
           GROUP BY size_apparent
           HAVING COUNT(*) > 1
           ORDER BY size_apparent DESC
           LIMIT 250
         )
         SELECT c.node_id, c.project_id, c.project_name, c.path, c.display_name,
                c.absolute_path, c.size_apparent, c.physical_bytes,
                c.aggregate_bytes_partial, c.volume_id, c.inode_key, c.attributes
         FROM candidate c
         JOIN duplicate_size ds ON ds.size_apparent = c.size_apparent
         ORDER BY c.size_apparent DESC, c.project_id, c.path
         LIMIT 10000",
    )?;
    let rows = stmt.query_map(
        params![min_size_bytes as i64, project_id, include_fixture_projects],
        |row| {
            let volume_id = row.get::<_, Option<String>>(9)?;
            let inode_key = row.get::<_, Option<String>>(10)?;
            Ok(DuplicateCandidateRow {
                node_id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                path: row.get(3)?,
                display_name: row.get(4)?,
                absolute_path: row.get(5)?,
                size_bytes: row.get::<_, i64>(6)?.max(0) as u64,
                physical_bytes: row
                    .get::<_, Option<i64>>(7)?
                    .map(|value| value.max(0) as u64),
                footprint_partial: row.get::<_, i64>(8)? == 1,
                identity_key: volume_id
                    .zip(inode_key)
                    .map(|(volume, inode)| format!("{volume}:{inode}")),
                attributes: row.get(11)?,
            })
        },
    )?;

    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    duplicate_groups_from_rows(candidates, limit, file_kind, None)
}

fn load_duplicate_candidates_for_node(
    conn: &Connection,
    node_id: i64,
    limit: usize,
    min_size_bytes: u64,
    file_kind: &str,
    include_fixture_projects: bool,
) -> DbResult<DuplicateCandidates> {
    let target_size = conn
        .query_row(
            "SELECT n.size_apparent
             FROM nav_item ni
             JOIN node n ON n.id = ni.node_id
             JOIN node p ON p.id = ni.project_id AND p.kind = 'project'
             WHERE ni.node_id = ?1
               AND ni.item_kind = 'file'
               AND ni.is_sensitive = 0
               AND ni.protected_level IS NULL
               AND n.present = 1
               AND n.is_reparse = 0
               AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
               AND n.size_apparent IS NOT NULL
               AND (?2 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')",
            params![node_id, include_fixture_projects],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|value| value.max(0) as u64);

    let Some(target_size) = target_size else {
        return Ok(DuplicateCandidates {
            groups: Vec::new(),
            total: 0,
        });
    };
    if target_size < min_size_bytes {
        return Ok(DuplicateCandidates {
            groups: Vec::new(),
            total: 0,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT ni.node_id, ni.project_id, p.name AS project_name, ni.path,
                ni.display_name, n.path AS absolute_path, n.size_apparent,
                COALESCE(n.size_allocated, n.size_apparent) AS physical_bytes,
                ni.aggregate_bytes_partial, n.volume_id, n.inode_key, n.attributes
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         JOIN node p ON p.id = ni.project_id
         WHERE ni.node_id IS NOT NULL
           AND ni.item_kind = 'file'
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND n.present = 1
           AND n.is_reparse = 0
           AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
           AND n.size_apparent = ?1
           AND (?2 = 1 OR COALESCE(json_extract(p.attributes, '$.source'), 'fixture') <> 'fixture')
         ORDER BY ni.project_id, ni.path
         LIMIT 10000",
    )?;
    let rows = stmt.query_map(
        params![target_size as i64, include_fixture_projects],
        |row| {
            let volume_id = row.get::<_, Option<String>>(9)?;
            let inode_key = row.get::<_, Option<String>>(10)?;
            Ok(DuplicateCandidateRow {
                node_id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                path: row.get(3)?,
                display_name: row.get(4)?,
                absolute_path: row.get(5)?,
                size_bytes: row.get::<_, i64>(6)?.max(0) as u64,
                physical_bytes: row
                    .get::<_, Option<i64>>(7)?
                    .map(|value| value.max(0) as u64),
                footprint_partial: row.get::<_, i64>(8)? == 1,
                identity_key: volume_id
                    .zip(inode_key)
                    .map(|(volume, inode)| format!("{volume}:{inode}")),
                attributes: row.get(11)?,
            })
        },
    )?;

    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    let mut result = duplicate_groups_from_rows(candidates, limit, file_kind, Some(node_id))?;
    result
        .groups
        .retain(|group| group.members.iter().any(|member| member.node_id == node_id));
    result.total = result.groups.len() as i64;
    Ok(result)
}

/// Loads the candidate rows that share the target node's apparent size, applying
/// the same safety filters as the duplicate-candidate query (not sensitive, not
/// protected, present, not reparse). Overlapping roots are deduped by physical
/// path so the same file is never counted twice.
fn load_confirm_candidate_rows(
    conn: &Connection,
    node_id: i64,
) -> DbResult<Option<Vec<DuplicateCandidateRow>>> {
    let target_size = conn
        .query_row(
            "SELECT n.size_apparent
             FROM nav_item ni
             JOIN node n ON n.id = ni.node_id
             WHERE ni.node_id = ?1
               AND ni.item_kind = 'file'
               AND ni.is_sensitive = 0
               AND ni.protected_level IS NULL
               AND n.present = 1
               AND n.is_reparse = 0
               AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
               AND n.size_apparent IS NOT NULL",
            params![node_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|value| value.max(0) as u64);

    let Some(target_size) = target_size else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT ni.node_id, ni.project_id, p.name AS project_name, ni.path,
                ni.display_name, n.path AS absolute_path, n.size_apparent,
                COALESCE(n.size_allocated, n.size_apparent) AS physical_bytes,
                ni.aggregate_bytes_partial, n.volume_id, n.inode_key, n.attributes
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         JOIN node p ON p.id = ni.project_id
         WHERE ni.node_id IS NOT NULL
           AND ni.item_kind = 'file'
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND n.present = 1
           AND n.is_reparse = 0
           AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
           AND n.size_apparent = ?1
         ORDER BY ni.project_id, ni.path
         LIMIT 10000",
    )?;
    let rows = stmt.query_map(params![target_size as i64], |row| {
        let volume_id = row.get::<_, Option<String>>(9)?;
        let inode_key = row.get::<_, Option<String>>(10)?;
        Ok(DuplicateCandidateRow {
            node_id: row.get(0)?,
            project_id: row.get(1)?,
            project_name: row.get(2)?,
            path: row.get(3)?,
            display_name: row.get(4)?,
            absolute_path: row.get(5)?,
            size_bytes: row.get::<_, i64>(6)?.max(0) as u64,
            physical_bytes: row
                .get::<_, Option<i64>>(7)?
                .map(|value| value.max(0) as u64),
            footprint_partial: row.get::<_, i64>(8)? == 1,
            identity_key: volume_id
                .zip(inode_key)
                .map(|(volume, inode)| format!("{volume}:{inode}")),
            attributes: row.get(11)?,
        })
    })?;

    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    let deduped = dedupe_duplicate_candidate_rows(candidates, Some(node_id));
    Ok(Some(deduped))
}

/// Resolves the candidate group containing `node_id` (same size + same bounded
/// partial hash), then full-hashes each candidate to confirm byte-for-byte
/// identity. Hardlinks (rows sharing a non-null `identity_key`) are collapsed to a
/// single physical file. Read-only: this only reads bytes and never mutates state.
fn confirm_duplicate_group(conn: &Connection, node_id: i64) -> DbResult<DuplicateConfirmation> {
    let cancel = AtomicBool::new(false);
    let mut progress = |_: DuplicateConfirmProgress| {};
    Ok(
        confirm_duplicate_group_inner(conn, node_id, &cancel, &mut progress)?
            .expect("a non-cancellable confirmation never returns None"),
    )
}

/// The body of [`confirm_duplicate_group`] with cooperative cancellation + progress. Returns
/// `Ok(None)` if `cancel` is set before a file is hashed; otherwise `Ok(Some(..))`. The work is
/// read-only — it only reads file bytes to hash them.
fn confirm_duplicate_group_inner(
    conn: &Connection,
    node_id: i64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(DuplicateConfirmProgress),
) -> DbResult<Option<DuplicateConfirmation>> {
    let empty = || DuplicateConfirmation {
        target_node_id: node_id,
        confirmed_groups: Vec::new(),
        checked_files: 0,
        bytes_hashed: 0,
        reclaimable_bytes: 0,
        partial: false,
    };

    let Some(rows) = load_confirm_candidate_rows(conn, node_id)? else {
        return Ok(Some(empty()));
    };

    // Restrict to rows that share the target's bounded partial hash, mirroring
    // how candidate groups are formed; this is the set the UI presented.
    let target_partial = rows
        .iter()
        .find(|row| row.node_id == node_id)
        .and_then(partial_hash_for_candidate);
    let Some(target_partial) = target_partial else {
        return Ok(Some(empty()));
    };

    let mut group_rows: Vec<DuplicateCandidateRow> = Vec::new();
    let mut partial = false;
    for row in rows {
        if !duplicate_candidate_allowed_path(&row.path) {
            continue;
        }
        match partial_hash_for_candidate(&row) {
            Some(hash) if hash == target_partial => group_rows.push(row),
            Some(_) => {}
            None => partial = true,
        }
    }

    if group_rows.len() < 2 {
        return Ok(Some(DuplicateConfirmation { partial, ..empty() }));
    }

    // Denominators known up front, so the UI can show "X of Y files / N of M bytes".
    let total_files = group_rows.len() as u64;
    let total_bytes = group_rows
        .iter()
        .fold(0_u64, |acc, row| acc.saturating_add(row.size_bytes));
    let mut checked_files = 0_usize;
    let mut bytes_hashed = 0_u64;
    progress(DuplicateConfirmProgress {
        checked_files: 0,
        total_files,
        bytes_hashed: 0,
        total_bytes,
    });
    let mut by_full_hash: HashMap<String, Vec<DuplicateCandidateRow>> = HashMap::new();
    for row in group_rows {
        // Cooperative cancellation: full-hashing streams every byte of each candidate, so a large
        // model set can take a while; bail at the next file boundary when the user cancels.
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match full_hash_for_candidate_row(&row) {
            Some(hash) => {
                checked_files += 1;
                bytes_hashed = bytes_hashed.saturating_add(row.size_bytes);
                progress(DuplicateConfirmProgress {
                    checked_files: checked_files as u64,
                    total_files,
                    bytes_hashed,
                    total_bytes,
                });
                by_full_hash.entry(hash).or_default().push(row);
            }
            None => partial = true,
        }
    }

    let mut confirmed_groups = Vec::new();
    let mut total_reclaimable = 0_u64;
    for (full_hash, members) in by_full_hash {
        // A confirmed set needs at least two DISTINCT physical files. Rows sharing
        // a non-null identity_key are the same hardlinked file and collapse to one.
        if members.len() < 2 || !has_distinct_physical_members(&members) {
            continue;
        }
        let reclaimable = reclaimable_bytes_for_confirmed(&members);
        total_reclaimable = total_reclaimable.saturating_add(reclaimable);
        let size_bytes = members.first().map(|m| m.size_bytes).unwrap_or(0);
        let mut member_views: Vec<DuplicateMember> = members
            .into_iter()
            .map(|member| DuplicateMember {
                node_id: member.node_id,
                project_id: member.project_id,
                project_name: member.project_name,
                path: member.path,
                display_name: member.display_name,
                physical_bytes: member.physical_bytes,
                footprint_partial: member.footprint_partial,
            })
            .collect();
        member_views.sort_by(|a, b| a.path.cmp(&b.path));
        confirmed_groups.push(ConfirmedDuplicateGroup {
            full_hash,
            size_bytes,
            member_count: member_views.len(),
            reclaimable_bytes: reclaimable,
            confidence: "High".to_string(),
            members: member_views,
        });
    }

    confirmed_groups.sort_by(|a, b| {
        b.reclaimable_bytes
            .cmp(&a.reclaimable_bytes)
            .then_with(|| a.full_hash.cmp(&b.full_hash))
    });

    Ok(Some(DuplicateConfirmation {
        target_node_id: node_id,
        confirmed_groups,
        checked_files,
        bytes_hashed,
        reclaimable_bytes: total_reclaimable,
        partial,
    }))
}

/// Reclaimable bytes for a confirmed (byte-identical) set: the physical footprint
/// of every distinct physical file except one kept copy. Hardlinks (shared
/// identity_key) collapse to a single physical file. We keep the largest physical
/// copy (deterministic: first by path on ties) so the reported savings are
/// conservative.
fn reclaimable_bytes_for_confirmed(members: &[DuplicateCandidateRow]) -> u64 {
    let mut seen = HashSet::new();
    // (physical_bytes, path) per distinct physical file.
    let mut physical: Vec<(u64, String)> = Vec::new();
    for member in members {
        let key = member
            .identity_key
            .clone()
            .unwrap_or_else(|| format!("path:{}", member.absolute_path));
        if !seen.insert(key) {
            continue;
        }
        physical.push((member.physical_bytes.unwrap_or(0), member.path.clone()));
    }
    if physical.len() < 2 {
        return 0;
    }
    let total: u64 = physical
        .iter()
        .fold(0_u64, |acc, (bytes, _)| acc.saturating_add(*bytes));
    // Keep the largest physical copy (ties broken by smallest path for determinism).
    let kept = physical
        .iter()
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
        .map(|(bytes, _)| *bytes)
        .unwrap_or(0);
    total.saturating_sub(kept)
}

fn duplicate_groups_from_rows(
    rows: Vec<DuplicateCandidateRow>,
    limit: usize,
    file_kind: &str,
    preferred_node_id: Option<i64>,
) -> DbResult<DuplicateCandidates> {
    let mut by_size: HashMap<u64, Vec<DuplicateCandidateRow>> = HashMap::new();
    for row in dedupe_duplicate_candidate_rows(rows, preferred_node_id) {
        if !duplicate_candidate_allowed_path(&row.path) {
            continue;
        }
        if !asset_kind_matches(file_kind, &file_asset_kind(&row.path)) {
            continue;
        }
        by_size.entry(row.size_bytes).or_default().push(row);
    }

    let mut groups = Vec::new();
    for (size_bytes, same_size) in by_size {
        if same_size.len() < 2 {
            continue;
        }
        let mut by_hash: HashMap<String, Vec<DuplicateCandidateRow>> = HashMap::new();
        for row in same_size {
            if let Some(hash) = partial_hash_for_candidate(&row) {
                by_hash.entry(hash).or_default().push(row);
            }
        }
        for (hash_partial, members) in by_hash {
            if members.len() < 2 || !has_distinct_physical_members(&members) {
                continue;
            }
            let footprint_partial = members.iter().any(|member| member.footprint_partial);
            let physical_bytes = duplicate_physical_bytes(&members);
            groups.push(DuplicateGroup {
                id: 0,
                size_bytes,
                hash_partial,
                confidence: "Medium".to_string(),
                reason: format!(
                    "Same apparent size and first {} KB hash. Full hash confirmation is deferred.",
                    DUPLICATE_PARTIAL_HASH_BYTES / 1024
                ),
                member_count: members.len() as u64,
                physical_bytes,
                footprint_partial,
                members: members
                    .into_iter()
                    .map(|member| DuplicateMember {
                        node_id: member.node_id,
                        project_id: member.project_id,
                        project_name: member.project_name,
                        path: member.path,
                        display_name: member.display_name,
                        physical_bytes: member.physical_bytes,
                        footprint_partial: member.footprint_partial,
                    })
                    .collect(),
            });
        }
    }

    groups.sort_by(|left, right| {
        right
            .size_bytes
            .cmp(&left.size_bytes)
            .then_with(|| right.member_count.cmp(&left.member_count))
            .then_with(|| left.members[0].path.cmp(&right.members[0].path))
    });
    let total = groups.len() as i64;
    if limit > 0 {
        groups.truncate(limit);
    }
    for (index, group) in groups.iter_mut().enumerate() {
        group.id = index as i64 + 1;
    }
    Ok(DuplicateCandidates { groups, total })
}

fn dedupe_duplicate_candidate_rows(
    rows: Vec<DuplicateCandidateRow>,
    preferred_node_id: Option<i64>,
) -> Vec<DuplicateCandidateRow> {
    let mut by_path: HashMap<String, DuplicateCandidateRow> = HashMap::new();
    for row in rows {
        let key = normalize_path(&row.absolute_path).to_ascii_lowercase();
        match by_path.get(&key) {
            Some(existing)
                if preferred_node_id == Some(row.node_id)
                    && preferred_node_id != Some(existing.node_id) =>
            {
                by_path.insert(key, row);
            }
            Some(_) => {}
            None => {
                by_path.insert(key, row);
            }
        }
    }
    by_path.into_values().collect()
}

fn duplicate_candidate_allowed_path(path: &str) -> bool {
    let normalized = normalize_path(path).to_ascii_lowercase();
    !normalized.split('/').any(|part| {
        matches!(
            part,
            ".git"
                | ".ssh"
                | "node_modules"
                | ".venv"
                | "venv"
                | "target"
                | "dist"
                | "build"
                | ".cache"
                | "__pycache__"
        )
    })
}

fn partial_hash_for_candidate(row: &DuplicateCandidateRow) -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    if row.absolute_path.starts_with("fixture://") {
        let body = fixture_body_from_attributes(row.attributes.as_deref())?;
        let bytes = body.as_bytes();
        let take = bytes.len().min(DUPLICATE_PARTIAL_HASH_BYTES);
        if take == 0 {
            return None;
        }
        hasher.update(&bytes[..take]);
        return Some(hasher.finalize().to_hex().to_string());
    }

    let mut file = fs::File::open(&row.absolute_path).ok()?;
    let mut remaining = DUPLICATE_PARTIAL_HASH_BYTES.min(row.size_bytes as usize);
    let mut buffer = [0_u8; 8192];
    let mut read_total = 0_usize;
    while remaining > 0 {
        let request = remaining.min(buffer.len());
        let read = file.read(&mut buffer[..request]).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        read_total += read;
        remaining -= read;
    }
    (read_total > 0).then(|| hasher.finalize().to_hex().to_string())
}

/// Streaming blake3 of the whole file in bounded chunks. Returns the hex digest,
/// or `None` on any IO error or if the file's byte length no longer matches
/// `expected_size` (the file changed since it was scanned). Never loads the whole
/// file into memory: a single reused 1 MiB buffer is read in a loop. Read-only.
fn full_hash_for_candidate(absolute_path: &Path, expected_size: u64) -> Option<String> {
    let mut file = fs::File::open(absolute_path).ok()?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; FULL_HASH_CHUNK_BYTES];
    let mut read_total: u64 = 0;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        read_total = read_total.saturating_add(read as u64);
        // Bail early if the file grew past what we expected.
        if read_total > expected_size {
            return None;
        }
    }
    if read_total != expected_size {
        return None;
    }
    Some(hasher.finalize().to_hex().to_string())
}

/// Full-hashes a candidate row, honoring the `fixture://` synthetic path scheme
/// used in tests (body stored in the row attributes). Mirrors the fixture branch
/// of `partial_hash_for_candidate` so confirmation works against fixtures too.
fn full_hash_for_candidate_row(row: &DuplicateCandidateRow) -> Option<String> {
    if row.absolute_path.starts_with("fixture://") {
        let body = fixture_body_from_attributes(row.attributes.as_deref())?;
        let bytes = body.as_bytes();
        if bytes.len() as u64 != row.size_bytes {
            return None;
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(bytes);
        return Some(hasher.finalize().to_hex().to_string());
    }
    full_hash_for_candidate(Path::new(&row.absolute_path), row.size_bytes)
}

fn fixture_body_from_attributes(attributes: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(attributes?).ok()?;
    value
        .get("body")
        .and_then(|body| body.as_str())
        .map(ToString::to_string)
}

fn has_distinct_physical_members(members: &[DuplicateCandidateRow]) -> bool {
    let mut seen = HashSet::new();
    for member in members {
        seen.insert(
            member
                .identity_key
                .clone()
                .unwrap_or_else(|| format!("path:{}", member.absolute_path)),
        );
        if seen.len() > 1 {
            return true;
        }
    }
    false
}

fn duplicate_physical_bytes(members: &[DuplicateCandidateRow]) -> Option<u64> {
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    let mut any = false;
    for member in members {
        let key = member
            .identity_key
            .clone()
            .unwrap_or_else(|| format!("path:{}", member.absolute_path));
        if !seen.insert(key) {
            continue;
        }
        if let Some(bytes) = member.physical_bytes {
            total = total.saturating_add(bytes);
            any = true;
        }
    }
    any.then_some(total)
}

fn delete_descendant_document_rows(conn: &Connection, nav_id: i64) -> DbResult<()> {
    conn.execute(
        "WITH RECURSIVE descendants(id, node_id) AS (
           SELECT id, node_id FROM nav_item WHERE parent_nav_id = ?1
           UNION ALL
           SELECT ni.id, ni.node_id FROM nav_item ni
           JOIN descendants d ON ni.parent_nav_id = d.id
         )
         DELETE FROM document_fts
         WHERE node_id IN (SELECT node_id FROM descendants WHERE node_id IS NOT NULL)",
        params![nav_id],
    )?;
    conn.execute(
        "WITH RECURSIVE descendants(id, node_id) AS (
           SELECT id, node_id FROM nav_item WHERE parent_nav_id = ?1
           UNION ALL
           SELECT ni.id, ni.node_id FROM nav_item ni
           JOIN descendants d ON ni.parent_nav_id = d.id
         )
         DELETE FROM document_index
         WHERE node_id IN (SELECT node_id FROM descendants WHERE node_id IS NOT NULL)",
        params![nav_id],
    )?;
    Ok(())
}

fn mark_existing_descendant_nodes_absent(conn: &Connection, nav_id: i64) -> DbResult<()> {
    conn.execute(
        "WITH RECURSIVE descendants(id, node_id) AS (
           SELECT id, node_id FROM nav_item WHERE parent_nav_id = ?1
           UNION ALL
           SELECT ni.id, ni.node_id FROM nav_item ni
           JOIN descendants d ON ni.parent_nav_id = d.id
         )
         UPDATE node
         SET present = 0, last_seen_at = ?2
         WHERE id IN (SELECT node_id FROM descendants WHERE node_id IS NOT NULL)",
        params![nav_id, now()],
    )?;
    Ok(())
}

fn delete_descendant_nav_rows(conn: &Connection, nav_id: i64) -> DbResult<()> {
    conn.execute(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM nav_item WHERE parent_nav_id = ?1
           UNION ALL
           SELECT ni.id FROM nav_item ni
           JOIN descendants d ON ni.parent_nav_id = d.id
         )
         DELETE FROM nav_item WHERE id IN (SELECT id FROM descendants)",
        params![nav_id],
    )?;
    Ok(())
}

fn insert_files_for_project(
    conn: &Connection,
    project_id: i64,
    files: &[ScannedFile],
) -> DbResult<()> {
    let mut dirs: HashMap<String, i64> = HashMap::new();

    for file in files {
        if file.item_kind == "directory" {
            ensure_dir_nav(conn, project_id, file, &mut dirs)?;
            continue;
        }

        let parent = parent_path(&file.relative_path);
        let parent_nav_id = ensure_dir_nav_path(conn, project_id, parent.as_deref(), &mut dirs)?;
        let node_id = upsert_item_node(conn, file)?;
        let priority = context_priority(&file.relative_path);
        upsert_nav_item(conn, project_id, node_id, parent_nav_id, file, priority)?;

        if let Some(body) = &file.body {
            index_document(
                conn,
                project_id,
                node_id,
                &file.relative_path,
                &file.display_name,
                body,
            )?;
        }
    }

    Ok(())
}

fn upsert_nav_item(
    conn: &Connection,
    project_id: i64,
    node_id: i64,
    parent_nav_id: Option<i64>,
    file: &ScannedFile,
    priority: i64,
) -> DbResult<i64> {
    let existing_id = conn
        .query_row(
            "SELECT id FROM nav_item WHERE project_id = ?1 AND path = ?2",
            params![project_id, file.relative_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(id) = existing_id {
        conn.execute(
            "UPDATE nav_item
             SET node_id = ?2, parent_nav_id = ?3, display_path = ?4, display_name = ?5,
                 item_kind = ?6, priority = ?7, sort_key = ?8, is_context = ?9,
                 is_markdown = ?10, is_sensitive = ?11, protected_level = ?12,
                 child_count = ?13, fully_scanned = ?14, collapse_default = ?15,
                 scan_error = ?16
             WHERE id = ?1",
            params![
                id,
                node_id,
                parent_nav_id,
                file.display_path,
                file.display_name,
                file.item_kind,
                priority,
                file.display_name.to_ascii_lowercase(),
                bool_to_i64(file.is_context),
                bool_to_i64(file.is_markdown),
                bool_to_i64(file.is_sensitive),
                file.protected_level.as_deref(),
                file.child_count,
                bool_to_i64(file.fully_scanned),
                bool_to_i64(file.collapse_default),
                file.scan_error.as_deref()
            ],
        )?;
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO nav_item(project_id, node_id, parent_nav_id, path, display_path, display_name, item_kind, priority, sort_key, is_context, is_markdown, is_sensitive, protected_level, child_count, fully_scanned, collapse_default, scan_error)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            project_id,
            node_id,
            parent_nav_id,
            file.relative_path,
            file.display_path,
            file.display_name,
            file.item_kind,
            priority,
            file.display_name.to_ascii_lowercase(),
            bool_to_i64(file.is_context),
            bool_to_i64(file.is_markdown),
            bool_to_i64(file.is_sensitive),
            file.protected_level.as_deref(),
            file.child_count,
            bool_to_i64(file.fully_scanned),
            bool_to_i64(file.collapse_default),
            file.scan_error.as_deref()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_git_metadata(
    conn: &Connection,
    project_id: i64,
    git: Option<&GitRepoSummary>,
) -> DbResult<()> {
    let Some(git) = git.filter(|summary| summary.has_git) else {
        return Ok(());
    };

    conn.execute(
        "INSERT OR REPLACE INTO git_repo(project_id, current_branch, head_ref, origin_url, metadata_error, indexed_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project_id,
            git.current_branch.as_deref(),
            git.head_ref.as_deref(),
            git.origin_url.as_deref(),
            git.metadata_error.as_deref(),
            now()
        ],
    )?;
    Ok(())
}

fn ensure_dir_nav(
    conn: &Connection,
    project_id: i64,
    file: &ScannedFile,
    dirs: &mut HashMap<String, i64>,
) -> DbResult<Option<i64>> {
    if let Some(id) = dirs.get(&file.relative_path) {
        return Ok(Some(*id));
    }
    let node_id = upsert_item_node(conn, file)?;
    let parent = parent_path(&file.relative_path);
    let parent_nav_id = ensure_dir_nav_path(conn, project_id, parent.as_deref(), dirs)?;
    let priority = 10;
    let nav_id = upsert_nav_item(conn, project_id, node_id, parent_nav_id, file, priority)?;
    dirs.insert(file.relative_path.clone(), nav_id);
    Ok(Some(nav_id))
}

fn ensure_dir_nav_path(
    conn: &Connection,
    project_id: i64,
    dir_path: Option<&str>,
    dirs: &mut HashMap<String, i64>,
) -> DbResult<Option<i64>> {
    let Some(dir_path) = dir_path.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Some(id) = dirs.get(dir_path) {
        return Ok(Some(*id));
    }

    let parent = parent_path(dir_path);
    let parent_nav_id = ensure_dir_nav_path(conn, project_id, parent.as_deref(), dirs)?;
    let display_name = display_name_for_path(dir_path);
    let node_id = upsert_item_node(
        conn,
        &ScannedFile {
            absolute_path: dir_path.to_string(),
            relative_path: dir_path.to_string(),
            display_path: display_path_for_path(dir_path),
            display_name: display_name.clone(),
            item_kind: "directory".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: is_sensitive_path(dir_path),
            protected_level: protected_level_for_path(dir_path),
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        },
    )?;
    let sort_key = display_name.to_ascii_lowercase();
    conn.execute(
        "INSERT OR IGNORE INTO nav_item(project_id, node_id, parent_nav_id, path, display_path, display_name, item_kind, priority, sort_key)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'directory', 10, ?7)",
        params![
            project_id,
            node_id,
            parent_nav_id,
            dir_path,
            display_path_for_path(dir_path),
            display_name,
            sort_key
        ],
    )?;
    let id = conn.query_row(
        "SELECT id FROM nav_item WHERE project_id = ?1 AND path = ?2",
        params![project_id, dir_path],
        |row| row.get(0),
    )?;
    dirs.insert(dir_path.to_string(), id);
    Ok(Some(id))
}

fn upsert_item_node(conn: &Connection, file: &ScannedFile) -> DbResult<i64> {
    let identity_json = file
        .identity
        .as_ref()
        .and_then(|identity| serde_json::to_value(identity).ok());
    let attributes = match (&file.body, file.absolute_path.starts_with("fixture://")) {
        (Some(body), true) => {
            json!({"body": body, "source": "fixture", "identity": identity_json}).to_string()
        }
        _ => json!({"source": "scan_metadata", "identity": identity_json}).to_string(),
    };
    let identity = file.identity.as_ref();
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM node WHERE path = ?1 AND kind = ?2",
            params![file.absolute_path, file.item_kind],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        conn.execute(
            "UPDATE node
             SET name = ?2, protected_level = ?3, volume_id = ?4, inode_key = ?5, link_count = ?6,
                 is_reparse = ?7, reparse_kind = ?8, size_apparent = ?9, size_allocated = ?10,
                 mtime = ?11, attributes = ?12, last_seen_at = ?13, present = 1
             WHERE id = ?1",
            params![
                id,
                file.display_name,
                file.protected_level.as_deref(),
                identity.and_then(|value| value.volume_id.as_deref()),
                identity.and_then(|value| value.inode_key.as_deref()),
                identity.and_then(|value| value.link_count.map(|count| count as i64)),
                identity
                    .map(|value| bool_to_i64(value.is_reparse || value.is_symlink))
                    .unwrap_or_default(),
                identity.and_then(|value| value.reparse_kind.as_deref()),
                identity.and_then(|value| value.size_apparent.map(|size| size as i64)),
                identity.and_then(|value| value.size_allocated.map(|size| size as i64)),
                identity.and_then(|value| value.modified_at.as_deref()),
                attributes,
                now()
            ],
        )?;
        upsert_scan_cache(conn, file, id)?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO node(kind, path, name, protected_level, volume_id, inode_key, link_count, is_reparse, reparse_kind, size_apparent, size_allocated, mtime, attributes, first_seen_at, last_seen_at, present)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, 1)",
        params![
            file.item_kind,
            file.absolute_path,
            file.display_name,
            file.protected_level.as_deref(),
            identity.and_then(|value| value.volume_id.as_deref()),
            identity.and_then(|value| value.inode_key.as_deref()),
            identity.and_then(|value| value.link_count.map(|count| count as i64)),
            identity
                .map(|value| bool_to_i64(value.is_reparse || value.is_symlink))
                .unwrap_or_default(),
            identity.and_then(|value| value.reparse_kind.as_deref()),
            identity.and_then(|value| value.size_apparent.map(|size| size as i64)),
            identity.and_then(|value| value.size_allocated.map(|size| size as i64)),
            identity.and_then(|value| value.modified_at.as_deref()),
            attributes,
            now()
        ],
    )?;
    let id = conn.last_insert_rowid();
    upsert_scan_cache(conn, file, id)?;
    Ok(id)
}

fn upsert_scan_cache(conn: &Connection, file: &ScannedFile, node_id: i64) -> DbResult<()> {
    let Some(identity) = file.identity.as_ref() else {
        return Ok(());
    };
    let (Some(mtime), Some(size)) = (
        identity.modified_at.as_deref(),
        identity.size_apparent.map(|size| size as i64),
    ) else {
        return Ok(());
    };
    conn.execute(
        "INSERT OR REPLACE INTO scan_cache(path, mtime, size, dir_signature, node_id)
         VALUES(?1, ?2, ?3, NULL, ?4)",
        params![file.absolute_path, mtime, size, node_id],
    )?;
    Ok(())
}

fn index_document(
    conn: &Connection,
    project_id: i64,
    node_id: i64,
    path: &str,
    title: &str,
    body: &str,
) -> DbResult<()> {
    let rendered = render_markdown_safe(body);
    let headings_json =
        serde_json::to_string(&rendered.headings).unwrap_or_else(|_| "[]".to_string());
    let links_json = serde_json::to_string(&rendered.links).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT OR REPLACE INTO document_index(node_id, project_id, title, headings_json, links_json, text_size, indexed_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![node_id, project_id, title, headings_json, links_json, body.len() as i64, now()],
    )?;
    conn.execute(
        "INSERT INTO document_fts(node_id, project_id, path, title, headings, body)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            node_id,
            project_id,
            path,
            title,
            rendered.headings.join("\n"),
            body
        ],
    )?;
    Ok(())
}

fn load_nav_items(
    conn: &Connection,
    clause: &str,
    bind: impl rusqlite::Params,
) -> DbResult<Vec<NavItem>> {
    let clause = format!("{clause} ORDER BY parent_nav_id, priority, sort_key, nav_item.id");
    load_nav_items_unordered(conn, &clause, bind)
}

fn load_nav_items_unordered(
    conn: &Connection,
    clause: &str,
    bind: impl rusqlite::Params,
) -> DbResult<Vec<NavItem>> {
    // Columns are fully qualified because we LEFT JOIN `node` (for its mtime) and
    // `id`/`path`/`protected_level` exist on both tables. Callers' clauses also use
    // `nav_item.id` in their ORDER BY for the same reason.
    let sql = format!(
        "SELECT nav_item.id, nav_item.project_id, nav_item.node_id, nav_item.parent_nav_id,
                nav_item.path, COALESCE(nav_item.display_path, nav_item.path),
                nav_item.display_name, nav_item.item_kind, nav_item.priority,
                nav_item.is_context, nav_item.is_markdown, nav_item.is_sensitive,
                nav_item.protected_level, nav_item.child_count, nav_item.fully_scanned,
                nav_item.collapse_default, nav_item.scan_error,
                nav_item.aggregate_apparent_bytes, nav_item.aggregate_allocated_bytes,
                nav_item.aggregate_physical_bytes, nav_item.aggregate_bytes_partial,
                node.mtime
         FROM nav_item LEFT JOIN node ON node.id = nav_item.node_id {clause}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(bind, |row| {
        Ok(NavItem {
            id: row.get(0)?,
            project_id: row.get(1)?,
            node_id: row.get(2)?,
            parent_nav_id: row.get(3)?,
            path: row.get(4)?,
            display_path: row.get(5)?,
            display_name: row.get(6)?,
            item_kind: row.get(7)?,
            priority: row.get(8)?,
            is_context: row.get::<_, i64>(9)? == 1,
            is_markdown: row.get::<_, i64>(10)? == 1,
            is_sensitive: row.get::<_, i64>(11)? == 1,
            protected_level: row.get(12)?,
            child_count: row.get(13)?,
            fully_scanned: row.get::<_, i64>(14)? == 1,
            collapse_default: row.get::<_, i64>(15)? == 1,
            scan_error: row.get(16)?,
            aggregate_apparent_bytes: row
                .get::<_, Option<i64>>(17)?
                .map(|value| value.max(0) as u64),
            aggregate_allocated_bytes: row
                .get::<_, Option<i64>>(18)?
                .map(|value| value.max(0) as u64),
            aggregate_physical_bytes: row
                .get::<_, Option<i64>>(19)?
                .map(|value| value.max(0) as u64),
            aggregate_bytes_partial: row.get::<_, i64>(20)? == 1,
            modified_at: row.get(21)?,
            children: Vec::new(),
        })
    })?;
    collect_rows(rows)
}

fn quick_open_row<'a>(
    query: &'a str,
) -> impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<QuickOpenResult> + 'a {
    move |row| {
        let label: String = row.get(2)?;
        let path: String = row.get(3)?;
        let project_name: String = row.get(5)?;
        let project_path: String = row.get(6)?;
        let score =
            score_quick_open_with_project(&label, &path, &project_name, &project_path, query)
                .unwrap_or(0);
        Ok(QuickOpenResult {
            node_id: row.get::<_, Option<i64>>(0)?.unwrap_or_default(),
            project_id: row.get(1)?,
            label,
            path,
            item_kind: row.get(4)?,
            score,
        })
    }
}

fn insert_recent(
    conn: &Connection,
    node_id: i64,
    project_id: i64,
    item_kind: &str,
) -> DbResult<()> {
    let opened_at = now();
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM recent_item WHERE node_id = ?1 AND item_kind = ?2 ORDER BY opened_at DESC LIMIT 1",
            params![node_id, item_kind],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        conn.execute(
            "UPDATE recent_item SET project_id = ?2, opened_at = ?3 WHERE id = ?1",
            params![id, project_id, opened_at],
        )?;
    } else {
        conn.execute(
            "INSERT INTO recent_item(node_id, project_id, item_kind, opened_at) VALUES(?1, ?2, ?3, ?4)",
            params![node_id, project_id, item_kind, opened_at],
        )?;
    }
    conn.execute(
        "DELETE FROM recent_item WHERE id NOT IN (
             SELECT id FROM recent_item ORDER BY opened_at DESC LIMIT ?1
         )",
        params![RECENT_LIMIT],
    )?;
    conn.execute(
        "UPDATE nav_item SET last_opened_at = ?2 WHERE node_id = ?1",
        params![node_id, opened_at],
    )?;
    Ok(())
}

fn body_from_attributes(attributes: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(attributes?).ok()?;
    value.get("body")?.as_str().map(ToString::to_string)
}

#[derive(Debug)]
struct TextRead {
    source: String,
    truncated: bool,
    system_error_code: Option<i64>,
}

#[derive(Debug)]
struct PreviewReadError {
    message: String,
    system_error_code: Option<i64>,
}

enum TextReadResult {
    Text(TextRead),
    Binary,
    Error(PreviewReadError),
}

fn file_kind_for_record(record: &PreviewRecord) -> FileKind {
    if record.item_kind == "directory" {
        FileKind::Directory
    } else if record.is_reparse
        || matches!(
            record.reparse_kind.as_deref(),
            Some("symlink") | Some("cloud_placeholder")
        )
    {
        FileKind::Symlink
    } else if record.is_markdown || record.is_context {
        FileKind::Markdown
    } else {
        FileKind::Text
    }
}

fn read_disk_text_limited(path: &str) -> TextReadResult {
    if path.starts_with("fixture://") {
        return TextReadResult::Error(PreviewReadError {
            message: "Fixture body missing from database.".to_string(),
            system_error_code: None,
        });
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            return TextReadResult::Error(PreviewReadError {
                message: friendly_io_error("File open failed", &err),
                system_error_code: err.raw_os_error().map(i64::from),
            });
        }
    };
    let mut buffer = vec![0; PREVIEW_LIMIT_BYTES as usize + 1];
    let read = match file.read(&mut buffer) {
        Ok(read) => read,
        Err(err) => {
            return TextReadResult::Error(PreviewReadError {
                message: friendly_io_error("File read failed", &err),
                system_error_code: err.raw_os_error().map(i64::from),
            });
        }
    };
    buffer.truncate(read);
    let truncated = read as u64 > PREVIEW_LIMIT_BYTES;
    if truncated {
        buffer.truncate(PREVIEW_LIMIT_BYTES as usize);
    }
    let sniff_end = buffer.len().min(PREVIEW_SNIFF_BYTES);
    if buffer[..sniff_end].contains(&0) {
        return TextReadResult::Binary;
    }
    let source = match String::from_utf8(buffer) {
        Ok(value) => value,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).to_string(),
    };
    TextReadResult::Text(TextRead {
        source,
        truncated,
        system_error_code: None,
    })
}

fn friendly_io_error(prefix: &str, err: &std::io::Error) -> String {
    match err.raw_os_error() {
        Some(32) => format!("{prefix}: file is in use by another process (Windows error 32)."),
        Some(code) => format!("{prefix}: {err} (system error {code})."),
        None => format!("{prefix}: {err}."),
    }
}

fn fts_query_for(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| {
            token
                .chars()
                .filter(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '-' || *ch == '.')
                .collect::<String>()
        })
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn like_filter(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim().to_ascii_lowercase();
    (!trimmed.is_empty()).then(|| format!("%{trimmed}%"))
}

fn resolve_relative_path(from_path: &str, target: &str) -> Option<String> {
    let target = target.split('#').next().unwrap_or(target).trim();
    if target.is_empty() || is_remote_target(target) {
        return None;
    }
    let decoded = percent_decode(target)?;
    let mut parts = if decoded.starts_with('/') || decoded.starts_with('\\') {
        Vec::new()
    } else {
        parent_path(from_path)
            .map(|parent| parent.split('/').map(ToString::to_string).collect())
            .unwrap_or_default()
    };
    for part in normalize_path(&decoded).split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value.to_string()),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn markdown_link_evidence(link: &MarkdownLink) -> String {
    if link.label.trim().is_empty() {
        link.target.clone()
    } else {
        format!("{} -> {}", link.label, link.target)
    }
}

fn is_same_file_anchor(target: &str) -> bool {
    target.trim().starts_with('#')
}

fn should_record_unresolved_markdown_link(target: &str) -> bool {
    let target = target.trim();
    !target.is_empty() && !is_same_file_anchor(target) && !is_remote_target(target)
}

fn bare_markdown_name(target: &str) -> Option<String> {
    let target = target.split('#').next().unwrap_or(target).trim();
    if target.is_empty()
        || target.starts_with('.')
        || target.starts_with('/')
        || target.starts_with('\\')
        || target.contains('/')
        || target.contains('\\')
        || target.contains(':')
        || is_remote_target(target)
    {
        return None;
    }
    percent_decode(target).filter(|value| !value.is_empty())
}

fn unresolved_markdown_confidence(target: &str) -> &'static str {
    let normalized = normalize_path(target).to_ascii_lowercase();
    if normalized.contains("..") || normalized.starts_with('/') || normalized.starts_with('\\') {
        "Low"
    } else {
        "Medium"
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex_value(high)? * 16 + hex_value(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_remote_target(target: &str) -> bool {
    let target = target.to_ascii_lowercase();
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("//")
        || target.starts_with("data:")
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> DbResult<Vec<T>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

fn normalize_comment_field(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "user".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Human-vs-AI safety boundary for editing/deleting a comment. The local user
/// (actor `"user"`, acting through the app) keeps full control. A non-"user" actor
/// — a connected AI app, identified by its registered name — may only touch a
/// comment whose `source` equals that same name (a comment it wrote itself). It can
/// NEVER edit or delete a human comment (`source = "user"`) or another agent's
/// comment. Enforced here, in the DB layer, so neither a UI bug nor the connected-app
/// server can bypass it: a "dumb" local model can never destroy a human note. The
/// actor value is always assigned by the trusted caller (the Tauri command pins it to
/// "user"; the connected-app server pins it to the authenticated agent), never
/// supplied by the agent, so it cannot be spoofed.
/// Key for the global "AI write mode" toggle (default OFF). When off, connected AI
/// apps may READ comments but may not add/edit/delete any. The local user is never
/// affected. This is the second of the two gates on an AI write (the first being the
/// per-agent `comments_write` scope checked at the connected-app boundary); the human/AI
/// ownership rule in `guard_comment_actor` is independent of and stronger than both.
const COMMENT_WRITE_ENABLED_KEY: &str = "comment_write_enabled";

fn comment_write_enabled(conn: &Connection) -> DbResult<bool> {
    Ok(setting_value(conn, COMMENT_WRITE_ENABLED_KEY)?.as_deref() == Some("1"))
}

// Keys for the configured AI Assist provider (connector edition). Settings only — the API key
// is never stored in the DB; it lives in the OS keychain. Default is mode `off`, so nothing
// leaves the machine until the user explicitly configures a provider.
const AI_PROVIDER_MODE_KEY: &str = "ai_provider_mode";
const AI_PROVIDER_BASE_URL_KEY: &str = "ai_provider_base_url";
const AI_PROVIDER_MODEL_KEY: &str = "ai_provider_model";
const AI_PROVIDER_FORMAT_KEY: &str = "ai_provider_format";
const AI_GLOSSARY_ENABLED_KEY: &str = "ai_glossary_enabled";

/// Read the AI provider config from the settings table. Absent keys default to a safe, inert
/// config: mode `off` (nothing leaves the machine), empty endpoint/model, and the universal
/// `chat_completions` wire format. No provider is ever defaulted to a real endpoint.
fn ai_provider_config(conn: &Connection) -> DbResult<AiProviderConfig> {
    Ok(AiProviderConfig {
        mode: setting_value(conn, AI_PROVIDER_MODE_KEY)?.unwrap_or_else(|| "off".to_string()),
        base_url: setting_value(conn, AI_PROVIDER_BASE_URL_KEY)?.unwrap_or_default(),
        model: setting_value(conn, AI_PROVIDER_MODEL_KEY)?.unwrap_or_default(),
        format: setting_value(conn, AI_PROVIDER_FORMAT_KEY)?
            .unwrap_or_else(|| "chat_completions".to_string()),
    })
}

/// Key for the "AI total control" tier (default OFF, heavily signposted). When on, a
/// trusted connected AI app may FILE a request to edit or delete a comment it could not
/// otherwise touch (e.g. a human's). That is the ONLY thing this tier adds — it never
/// lets the connector read file bodies, run the Gate-3 mutation actions, or request a
/// final-remove (all of which remain out of the connector's reach). The request changes
/// nothing on its own: the user approves it in-app, and only then does Code Hangar
/// perform it AS the user, after an offered prior backup to a safe user-chosen folder.
/// The agent never bypasses `guard_comment_actor` directly.
const MCP_FULL_CONTROL_KEY: &str = "mcp_full_control_enabled";

fn mcp_full_control_enabled(conn: &Connection) -> DbResult<bool> {
    Ok(setting_value(conn, MCP_FULL_CONTROL_KEY)?.as_deref() == Some("1"))
}

const FINAL_REMOVE_ENABLED_KEY: &str = "final_remove_enabled";

fn final_remove_enabled(conn: &Connection) -> DbResult<bool> {
    // Final removal is OFF by default: the encrypted setting must be explicitly written as "1"
    // before the irreversible action is offered in mutation builds.
    // Every per-action safety (verified backup, fresh confirmation token, protected/sensitive
    // refusal, recovery) still runs regardless — this only controls whether the action is offered.
    Ok(setting_value(conn, FINAL_REMOVE_ENABLED_KEY)?.as_deref() == Some("1"))
}

/// Whether the user has confirmed they run AI tools inside WSL. OFF by default so
/// discovery never spawns `wsl.exe` unprompted (which can surface a WSL error at
/// startup on a machine where WSL is present but not fully set up).
const WSL_SCAN_ENABLED_KEY: &str = "wsl_scan_enabled";

fn wsl_scan_enabled(conn: &Connection) -> DbResult<bool> {
    Ok(setting_value(conn, WSL_SCAN_ENABLED_KEY)?.as_deref() == Some("1"))
}

/// Read-only "panic switch" key (default off). When set, the connector may read but
/// never write or mutate, overriding the write/total-control toggles.
const MCP_READ_ONLY_KEY: &str = "mcp_read_only_mode";

fn mcp_read_only_mode(conn: &Connection) -> DbResult<bool> {
    Ok(setting_value(conn, MCP_READ_ONLY_KEY)?.as_deref() == Some("1"))
}

fn guard_comment_actor(
    conn: &Connection,
    comment_id: i64,
    actor: &str,
    action: &str,
) -> DbResult<()> {
    if actor == "user" {
        return Ok(());
    }
    // Global AI write-mode gate: with it off, an AI app cannot write at all.
    if !comment_write_enabled(conn)? {
        return Err(DbError::FileRead(format!(
            "AI write mode is off. Enable it in Settings before an AI app can {action} comments."
        )));
    }
    let source: Option<String> = conn
        .query_row(
            "SELECT source FROM comment WHERE id = ?1 AND deleted_at IS NULL",
            params![comment_id],
            |row| row.get(0),
        )
        .optional()?;
    match source {
        None => Err(DbError::FileRead("Comment not found.".to_string())),
        Some(source) if source == actor => Ok(()),
        Some(_) => Err(DbError::FileRead(format!(
            "An AI app may only {action} the comments it wrote itself; human comments and other agents' comments are protected."
        ))),
    }
}

fn comment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Comment> {
    Ok(Comment {
        id: row.get(0)?,
        node_id: row.get(1)?,
        project_id: row.get(2)?,
        body: row.get(3)?,
        author: row.get(4)?,
        source: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn code_annotation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeAnnotation> {
    Ok(CodeAnnotation {
        id: row.get(0)?,
        node_id: row.get(1)?,
        snippet_hash: row.get(2)?,
        line_start: row.get::<_, i64>(3)?.max(0) as u64,
        line_end: row.get::<_, i64>(4)?.max(0) as u64,
        note: row.get(5)?,
        anchor_state: "unchecked".to_string(),
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn read_comment(conn: &Connection, id: i64) -> DbResult<Comment> {
    conn.query_row(
        "SELECT id, node_id, project_id, body, author, source, created_at, updated_at
         FROM comment WHERE id = ?1",
        params![id],
        comment_from_row,
    )
    .map_err(DbError::from)
}

/// Input to [`Db::agent_request_create`]. Comment kinds set `target_comment_id` +
/// `proposed_body`; other kinds set the generic target fields. Build with `Default`
/// and fill only what the kind needs.
#[derive(Debug, Clone, Default)]
pub struct NewAgentRequest {
    pub agent_id: Option<i64>,
    pub agent_name: String,
    pub kind: String,
    pub target_comment_id: Option<i64>,
    pub proposed_body: Option<String>,
    pub detail: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<i64>,
    pub project_id: Option<i64>,
    pub payload_json: Option<String>,
    pub cross_scope: bool,
}

// Columns are spelled out (not `ar.*`) so the order is stable across the additive
// migration; the comment JOIN enriches comment kinds with their present body/source.
// Order matches agent_request_from_row: 0-9 base, 10-15 generic, 16-17 comment JOIN.
const AGENT_REQUEST_SELECT_PENDING: &str = "SELECT ar.id, ar.agent_id, ar.agent_name, ar.kind, ar.target_comment_id, ar.proposed_body, ar.detail, ar.status, ar.created_at, ar.resolved_at, ar.target_kind, ar.target_id, ar.project_id, ar.payload_json, ar.result_json, ar.cross_scope, c.body, c.source
     FROM agent_request ar
     LEFT JOIN comment c ON c.id = ar.target_comment_id AND c.deleted_at IS NULL
     WHERE ar.status = 'pending'
     ORDER BY ar.created_at DESC, ar.id DESC";

const AGENT_REQUEST_SELECT_ONE: &str = "SELECT ar.id, ar.agent_id, ar.agent_name, ar.kind, ar.target_comment_id, ar.proposed_body, ar.detail, ar.status, ar.created_at, ar.resolved_at, ar.target_kind, ar.target_id, ar.project_id, ar.payload_json, ar.result_json, ar.cross_scope, c.body, c.source
     FROM agent_request ar
     LEFT JOIN comment c ON c.id = ar.target_comment_id AND c.deleted_at IS NULL
     WHERE ar.id = ?1";

// Every request filed by one agent, all statuses, newest first. The `agent_id = ?1`
// predicate is the own-app scope for `list_my_requests`: an agent can never read
// another app's rows. Column order matches agent_request_from_row.
const AGENT_REQUEST_SELECT_FOR_AGENT: &str = "SELECT ar.id, ar.agent_id, ar.agent_name, ar.kind, ar.target_comment_id, ar.proposed_body, ar.detail, ar.status, ar.created_at, ar.resolved_at, ar.target_kind, ar.target_id, ar.project_id, ar.payload_json, ar.result_json, ar.cross_scope, c.body, c.source
     FROM agent_request ar
     LEFT JOIN comment c ON c.id = ar.target_comment_id AND c.deleted_at IS NULL
     WHERE ar.agent_id = ?1
     ORDER BY ar.created_at DESC, ar.id DESC";

fn agent_request_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentActionRequest> {
    Ok(AgentActionRequest {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        agent_name: row.get(2)?,
        kind: row.get(3)?,
        target_comment_id: row.get(4)?,
        proposed_body: row.get(5)?,
        detail: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        resolved_at: row.get(9)?,
        target_kind: row.get(10)?,
        target_id: row.get(11)?,
        project_id: row.get(12)?,
        payload_json: row.get(13)?,
        result_json: row.get(14)?,
        cross_scope: row.get(15)?,
        current_body: row.get(16)?,
        current_source: row.get(17)?,
    })
}

fn read_agent_request(conn: &Connection, id: i64) -> DbResult<AgentActionRequest> {
    conn.query_row(
        AGENT_REQUEST_SELECT_ONE,
        params![id],
        agent_request_from_row,
    )
    .map_err(DbError::from)
}

fn explain_folder_classification(
    display_name: &str,
    display_path: &str,
    item_kind: &str,
    protected_level: Option<&str>,
    is_context: bool,
    is_sensitive: bool,
) -> (String, String, String) {
    if item_kind != "directory" {
        return (
            "file".to_string(),
            "high".to_string(),
            "This is a file entry, not a folder. Folder relationship classification does not apply."
                .to_string(),
        );
    }
    if protected_level.is_some() || is_sensitive {
        return (
            "protected".to_string(),
            "high".to_string(),
            "This folder is governed by local Protected Zone or sensitive-file rules.".to_string(),
        );
    }

    let name = display_name.to_ascii_lowercase();
    let path = display_path.replace('\\', "/").to_ascii_lowercase();
    let path_has = |needle: &str| path.split('/').any(|part| part == needle);

    if name == ".git" || path_has(".git") {
        return (
            "source-control-metadata".to_string(),
            "high".to_string(),
            "This folder is Git metadata for a local working tree.".to_string(),
        );
    }
    if matches!(name.as_str(), "node_modules" | ".pnpm" | "vendor") || path_has("node_modules") {
        return (
            "dependency-cache".to_string(),
            "high".to_string(),
            "This folder is a local dependency tree or package cache.".to_string(),
        );
    }
    if matches!(name.as_str(), ".venv" | "venv" | "env") || path_has(".venv") || path_has("venv") {
        return (
            "virtual-environment".to_string(),
            "high".to_string(),
            "This folder is a local language runtime environment.".to_string(),
        );
    }
    if matches!(
        name.as_str(),
        "target" | "dist" | "build" | ".next" | "out" | "__pycache__" | ".cache"
    ) {
        return (
            "build-or-cache-output".to_string(),
            "high".to_string(),
            "This folder is likely generated build output or cache data.".to_string(),
        );
    }
    // The app's own specialty: an AI coding tool's config/state directory (or anything
    // under it). Recognise the tool dotfolders so the folder explanation can say what
    // steers the agent rather than guessing "source" or "other".
    if [
        ".claude",
        ".cursor",
        ".codex",
        ".gemini",
        ".windsurf",
        ".aider",
        ".hermes",
        ".openclaw",
    ]
    .iter()
    .any(|&dir| name == dir || path_has(dir))
    {
        return (
            "ai-tool-config".to_string(),
            "high".to_string(),
            "This folder holds an AI coding tool's own settings, rules, skills or state \
             (Claude Code, Cursor, Codex, Gemini/Antigravity, and similar) — not your \
             project's source."
                .to_string(),
        );
    }
    if matches!(name.as_str(), "docs" | "doc" | "documentation") || is_context {
        return (
            "documentation-context".to_string(),
            "medium".to_string(),
            "This folder likely holds project documentation or context files.".to_string(),
        );
    }
    if matches!(
        name.as_str(),
        "src" | "source" | "app" | "apps" | "crates" | "packages"
    ) {
        return (
            "project-source".to_string(),
            "medium".to_string(),
            "This folder likely contains source code owned by the project.".to_string(),
        );
    }
    if matches!(
        name.as_str(),
        "models" | "model" | "checkpoints" | "loras" | "lora" | "controlnet"
    ) {
        return (
            "model-assets".to_string(),
            "low".to_string(),
            "This folder may contain reusable model assets. Review before treating it as owned by one project."
                .to_string(),
        );
    }

    (
        "unknown".to_string(),
        "unknown".to_string(),
        "Code Hangar cannot classify this relationship.".to_string(),
    )
}

fn count_i64(conn: &Connection, sql: &str) -> DbResult<i64> {
    conn.query_row(sql, [], |row| row.get(0))
        .map_err(DbError::from)
}

fn count_i64_with_visibility(
    conn: &Connection,
    sql: &str,
    include_fixture_projects: i64,
) -> DbResult<i64> {
    conn.query_row(sql, params![include_fixture_projects], |row| row.get(0))
        .map_err(DbError::from)
}

fn setting_value(conn: &Connection, key: &str) -> DbResult<Option<String>> {
    conn.query_row(
        "SELECT value FROM setting WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(DbError::from)
}

fn set_setting(conn: &Connection, key: &str, value: &str) -> DbResult<()> {
    conn.execute(
        "INSERT INTO setting(key, value)
         VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn retry_busy<T>(mut f: impl FnMut() -> DbResult<T>) -> DbResult<T> {
    let mut delay = Duration::from_millis(25);
    for attempt in 0..5 {
        match f() {
            Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == ErrorCode::DatabaseBusy
                    || error.code == ErrorCode::DatabaseLocked =>
            {
                if attempt == 4 {
                    return Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(error, None)));
                }
                sleep(delay);
                delay = delay.saturating_mul(2);
            }
            result => return result,
        }
    }
    f()
}

fn parent_path(path: &str) -> Option<String> {
    let normalized = normalize_path(path);
    normalized
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn normalize_ledger_change_set(change_set: &SessionChangeSet) -> SessionChangeSet {
    let mut normalized = change_set.clone();
    for file in &mut normalized.files {
        if let Some(reality) = &mut file.reality {
            reality.observed_ms = None;
        }
        for edit in &mut file.edits {
            if let Some(reality) = &mut edit.reality {
                reality.observed_ms = None;
            }
        }
    }
    normalized
}

fn change_ledger_entry_hash(
    project_id: i64,
    source_ref: &str,
    source_modified_ms: i64,
    observed_at: &str,
    content_hash: &str,
    previous_entry_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [
        project_id.to_string(),
        source_ref.to_string(),
        source_modified_ms.to_string(),
        observed_at.to_string(),
        content_hash.to_string(),
        previous_entry_hash.unwrap_or_default().to_string(),
    ] {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

struct FixtureProject {
    id: &'static str,
    name: &'static str,
    root: &'static str,
    files: &'static [FixtureFile],
}

struct FixtureFile {
    path: &'static str,
    body: &'static str,
}

fn fixture_projects() -> Vec<FixtureProject> {
    vec![
        FixtureProject {
            id: "markdown-project",
            name: "Fixture Markdown Project",
            root: "fixture://markdown-project",
            files: &[
                FixtureFile {
                    path: "README.md",
                    body: include_str!("../../../fixtures/markdown-project/README.md"),
                },
                FixtureFile {
                    path: "AGENTS.md",
                    body: include_str!("../../../fixtures/markdown-project/AGENTS.md"),
                },
                FixtureFile {
                    path: "docs/overview.md",
                    body: include_str!("../../../fixtures/markdown-project/docs/overview.md"),
                },
                FixtureFile {
                    path: "docs/diagram.png",
                    body: "",
                },
                FixtureFile {
                    path: "assets/unused.png",
                    body: "",
                },
                FixtureFile {
                    path: "prompts/system.md",
                    body: include_str!("../../../fixtures/markdown-project/prompts/system.md"),
                },
                FixtureFile {
                    path: "src/main.ts",
                    body: include_str!("../../../fixtures/markdown-project/src/main.ts"),
                },
                FixtureFile {
                    path: "package.json",
                    body: include_str!("../../../fixtures/markdown-project/package.json"),
                },
            ],
        },
        FixtureProject {
            id: "sensitive-project",
            name: "Fixture Sensitive Project",
            root: "fixture://sensitive-project",
            files: &[
                FixtureFile {
                    path: "README.md",
                    body: include_str!("../../../fixtures/sensitive-project/README.md"),
                },
                FixtureFile {
                    path: ".env",
                    body: "",
                },
                FixtureFile {
                    path: "credentials.json",
                    body: "",
                },
                FixtureFile {
                    path: "token.json",
                    body: "",
                },
            ],
        },
        FixtureProject {
            id: "git-like-project",
            name: "Fixture Git-like Project",
            root: "fixture://git-like-project",
            files: &[
                FixtureFile {
                    path: "README.md",
                    body: include_str!("../../../fixtures/git-like-project/README.md"),
                },
                FixtureFile {
                    path: "AGENTS.md",
                    body: include_str!("../../../fixtures/git-like-project/AGENTS.md"),
                },
                FixtureFile {
                    path: ".git/config",
                    body: "",
                },
                FixtureFile {
                    path: "src/main.rs",
                    body: include_str!("../../../fixtures/git-like-project/src/main.rs"),
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn loads_fixture_projects() {
        let db = Db::open_memory().unwrap();
        let projects = db.projects_list().unwrap();
        assert!(projects
            .iter()
            .any(|project| project.name == "Fixture Markdown Project"));
    }

    #[test]
    fn review_checkpoint_and_encrypted_ledger_round_trip() {
        let db = Db::open_memory().unwrap();
        let project_id = db.projects_list().unwrap()[0].id;
        let checkpoint = db
            .set_project_review_checkpoint(
                project_id,
                1_234,
                Some("git-fingerprint"),
                Some("0123456789012345678901234567890123456789"),
            )
            .unwrap();
        assert_eq!(checkpoint.session_cutoff_ms, 1_234);
        assert_eq!(
            db.project_review_checkpoint(project_id)
                .unwrap()
                .unwrap()
                .git_fingerprint
                .as_deref(),
            Some("git-fingerprint")
        );
        assert_eq!(
            db.project_review_checkpoint(project_id)
                .unwrap()
                .unwrap()
                .git_head
                .as_deref(),
            Some("0123456789012345678901234567890123456789")
        );
        assert_eq!(
            db.project_review_checkpoints()
                .unwrap()
                .iter()
                .map(|checkpoint| checkpoint.project_id)
                .collect::<Vec<_>>(),
            vec![project_id]
        );

        let change_set = SessionChangeSet {
            path: "fixture-session.jsonl".to_string(),
            source_kind: "Codex".to_string(),
            coverage: hangar_core::SessionChangeCoverage {
                level: "full".to_string(),
                label: "Fixture evidence".to_string(),
                note: "Observed locally".to_string(),
            },
            files: Vec::new(),
            edit_count: 0,
            added_lines: 0,
            removed_lines: 0,
            redacted_count: 0,
            parsed_records: 1,
            omitted_records: 0,
        };
        let first_id = db
            .store_review_evidence(project_id, &change_set.path, Some(9_999), &change_set)
            .unwrap();
        let repeated_id = db
            .store_review_evidence(project_id, &change_set.path, Some(9_999), &change_set)
            .unwrap();
        assert_eq!(first_id, repeated_id, "the same source version is upserted");
        let ledger = db.project_review_ledger(project_id, 10).unwrap();
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].change_set, change_set);
        assert_eq!(ledger[0].entry_hash.len(), 64);
        assert!(ledger[0].previous_entry_hash.is_none());
        assert!(ledger[0].encoded_bytes > 0);

        let mut next_change = change_set.clone();
        next_change.coverage.note = "A second immutable observation".to_string();
        let second_id = db
            .store_change_evidence(
                project_id,
                None,
                "codehangar:fixture:2",
                Some(10_000),
                Some("value"),
                Some("session-2"),
                Some(&"a".repeat(64)),
                Some(&"b".repeat(64)),
                &next_change,
            )
            .unwrap();
        assert_ne!(second_id, first_id);
        let ledger = db.project_review_ledger(project_id, 10).unwrap();
        assert_eq!(ledger.len(), 2);
        let second = ledger.iter().find(|entry| entry.id == second_id).unwrap();
        let first = ledger.iter().find(|entry| entry.id == first_id).unwrap();
        assert_eq!(
            second.previous_entry_hash.as_deref(),
            Some(first.entry_hash.as_str())
        );
        assert_eq!(second.origin.as_deref(), Some("value"));
        assert_eq!(second.session_id.as_deref(), Some("session-2"));
        assert_eq!(second.before_hash.as_deref(), Some("a".repeat(64).as_str()));
        assert_eq!(second.after_hash.as_deref(), Some("b".repeat(64).as_str()));

        db.with_writer(|conn| {
            conn.execute(
                "UPDATE change_ledger SET change_set_json = '{}'
                 WHERE id = ?1",
                [second_id],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db
            .project_review_ledger(project_id, 10)
            .unwrap_err()
            .to_string()
            .contains("integrity check"));
    }

    #[test]
    fn project_check_approval_round_trip_updates_and_revokes() {
        let db = Db::open_memory().unwrap();
        let project_id = db.projects_list().unwrap()[0].id;
        let first_fingerprint = "a".repeat(64);
        let changed_fingerprint = "b".repeat(64);

        assert!(db
            .project_check_approval(project_id, "npm:test")
            .unwrap()
            .is_none());

        let approved_at = db
            .set_project_check_approval(project_id, "npm:test", &first_fingerprint)
            .unwrap();
        assert_eq!(
            db.project_check_approval(project_id, "npm:test").unwrap(),
            Some((first_fingerprint, approved_at))
        );

        db.set_project_check_approval(project_id, "npm:test", &changed_fingerprint)
            .unwrap();
        assert_eq!(
            db.project_check_approval(project_id, "npm:test")
                .unwrap()
                .unwrap()
                .0,
            changed_fingerprint
        );
        assert!(db
            .revoke_project_check_approval(project_id, "npm:test")
            .unwrap());
        assert!(!db
            .revoke_project_check_approval(project_id, "npm:test")
            .unwrap());
        assert!(db
            .project_check_approval(project_id, "npm:test")
            .unwrap()
            .is_none());
    }

    #[test]
    fn compact_leaves_the_database_usable_and_is_repeatable() {
        let db = Db::open_memory().unwrap();
        let before = db.projects_list().unwrap().len();
        // VACUUM rewrites the whole database; running it twice must not error or lose data.
        db.compact().unwrap();
        db.compact().unwrap();
        let after = db.projects_list().unwrap();
        assert_eq!(after.len(), before, "compaction must preserve rows");
        assert!(after
            .iter()
            .any(|project| project.name == "Fixture Markdown Project"));
    }

    #[test]
    #[ignore = "file-backed performance fixture; run with scripts/acceptance-v011.ps1 -Lane RuntimePerf"]
    fn file_backed_compaction_reclaims_space_and_is_repeatable() {
        const ROW_COUNT: usize = 4_000;
        const PAYLOAD_BYTES: usize = 4 * 1024;

        fn footprint(path: &Path) -> u64 {
            ["", "-wal", "-shm"]
                .into_iter()
                .filter_map(|suffix| {
                    let candidate = if suffix.is_empty() {
                        path.to_path_buf()
                    } else {
                        PathBuf::from(format!("{}{}", path.to_string_lossy(), suffix))
                    };
                    fs::metadata(candidate).ok().map(|metadata| metadata.len())
                })
                .sum()
        }

        let dir = tempdir().unwrap();
        let path = dir.path().join("compact.sqlite3");
        let db = Db::open(&path).unwrap();
        let payload = "x".repeat(PAYLOAD_BYTES);

        let populate_started = Instant::now();
        db.with_writer(|conn| {
            let tx = conn.transaction()?;
            {
                let mut insert = tx.prepare("INSERT INTO setting(key, value) VALUES(?1, ?2)")?;
                for index in 0..ROW_COUNT {
                    insert.execute(params![format!("perf.compact.row.{index:05}"), payload])?;
                }
                tx.execute(
                    "INSERT INTO setting(key, value) VALUES('perf.compact.sentinel', 'preserved')",
                    [],
                )?;
            }
            tx.commit()?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
        .unwrap();
        let populate_ms = populate_started.elapsed().as_millis();
        let expanded_bytes = footprint(&path);
        assert!(
            expanded_bytes > 8 * 1024 * 1024,
            "fixture did not create a meaningful database: {expanded_bytes} bytes"
        );

        db.with_writer(|conn| {
            conn.execute(
                "DELETE FROM setting WHERE key LIKE 'perf.compact.row.%'",
                [],
            )?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
        .unwrap();
        let before_bytes = footprint(&path);

        let first_started = Instant::now();
        db.compact().unwrap();
        let first_ms = first_started.elapsed().as_millis();
        let after_bytes = footprint(&path);
        assert!(
            after_bytes.saturating_mul(3) < before_bytes,
            "compaction did not reclaim the deleted payload: {before_bytes} -> {after_bytes}"
        );

        let second_started = Instant::now();
        db.compact().unwrap();
        let second_ms = second_started.elapsed().as_millis();
        let repeated_bytes = footprint(&path);
        assert!(
            repeated_bytes <= after_bytes.saturating_add(128 * 1024),
            "repeat compaction unexpectedly grew the database: {after_bytes} -> {repeated_bytes}"
        );

        db.with_conn(|conn| {
            let sentinel: String = conn.query_row(
                "SELECT value FROM setting WHERE key = 'perf.compact.sentinel'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(sentinel, "preserved");
            let integrity: String =
                conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            assert_eq!(integrity, "ok");
            Ok(())
        })
        .unwrap();

        drop(db);
        let reopened = Db::open(&path).unwrap();
        reopened
            .with_conn(|conn| {
                let sentinel: String = conn.query_row(
                    "SELECT value FROM setting WHERE key = 'perf.compact.sentinel'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(sentinel, "preserved");
                Ok(())
            })
            .unwrap();

        println!(
            "PERF_COMPACT rows={ROW_COUNT} payload_bytes={PAYLOAD_BYTES} populate_ms={populate_ms} before_bytes={before_bytes} after_bytes={after_bytes} first_ms={first_ms} repeated_bytes={repeated_bytes} second_ms={second_ms}"
        );
    }

    #[test]
    fn startup_maintenance_marks_existing_markdown_relationships_complete() {
        let db = Db::open_memory().unwrap();

        db.run_startup_maintenance().unwrap();

        db.with_conn(|conn| {
            assert_eq!(
                setting_value(conn, MARKDOWN_EDGE_BACKFILL_SETTING)?,
                Some("complete".to_string())
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn startup_maintenance_defers_large_markdown_backfill() {
        let db = Db::open_memory().unwrap();

        db.with_writer(|conn| {
            conn.execute(
                "DELETE FROM setting WHERE key = ?1",
                params![MARKDOWN_EDGE_BACKFILL_SETTING],
            )?;
            conn.execute("DELETE FROM edge WHERE kind = 'markdown_links_to'", [])?;
            conn.execute(
                "DELETE FROM relationship_issue
                 WHERE kind IN ('unresolved_markdown_link', 'ambiguous_markdown_link')",
                [],
            )?;
            let project_id: i64 = conn.query_row(
                "SELECT id FROM node WHERE kind = 'project' LIMIT 1",
                [],
                |row| row.get(0),
            )?;
            {
                let tx = conn.transaction()?;
                let mut insert_node = tx.prepare(
                    "INSERT INTO node(kind, path, name, attributes, first_seen_at, last_seen_at, present)
                     VALUES('file', ?1, ?2, '{}', ?3, ?3, 1)",
                )?;
                let mut insert_document = tx.prepare(
                    "INSERT INTO document_index(node_id, project_id, title, headings_json, links_json, indexed_at)
                     VALUES(?1, ?2, ?3, '[]', '[]', ?4)",
                )?;
                for index in 0..=MAX_STARTUP_MARKDOWN_BACKFILL_DOCS {
                    let path = format!("fixture://large/doc-{index}.md");
                    let name = format!("doc-{index}.md");
                    let timestamp = now();
                    insert_node.execute(params![path, name, timestamp])?;
                    let node_id = tx.last_insert_rowid();
                    insert_document.execute(params![node_id, project_id, name, now()])?;
                }
                drop(insert_document);
                drop(insert_node);
                tx.commit()?;
            }

            backfill_missing_markdown_edges(conn)?;

            assert_eq!(
                setting_value(conn, MARKDOWN_EDGE_BACKFILL_SETTING)?,
                Some("deferred-large".to_string())
            );
            assert_eq!(
                count_i64(conn, "SELECT COUNT(*) FROM edge WHERE kind = 'markdown_links_to'")?,
                0
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn sensitive_files_are_blocked_from_preview() {
        let db = Db::open_memory().unwrap();
        let hit = db
            .quick_open(".env", 10)
            .unwrap()
            .into_iter()
            .find(|result| result.project_id != 2)
            .unwrap();
        let preview = db
            .file_preview(hit.node_id, PreviewMode::Source, true)
            .unwrap();
        assert_eq!(preview.state, PreviewState::Blocked);
        assert!(preview.source.is_none());
    }

    #[test]
    fn rendered_preview_keeps_context_json_preformatted() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let package_body = r#"{"name":"app","scripts":{"dev":"vite"}}"#.to_string();
        let package_path = dir.path().join("package.json");
        fs::write(&package_path, &package_body).unwrap();

        let files = [ScannedFile {
            display_name: "package.json".to_string(),
            is_markdown: false,
            is_context: true,
            identity: Some(file_identity(package_body.len() as u64, None, None, None)),
            body: Some(package_body),
            ..scanned_fixture_for_path(&package_path, "package.json")
        }];
        let root_path = dir.path().to_string_lossy().to_string();
        db.load_scanned_root(&root_path, &files, None).unwrap();
        let project_id = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root_path)
            .unwrap()
            .id;

        let hit = db
            .quick_open("package", 20)
            .unwrap()
            .into_iter()
            .find(|result| result.project_id == project_id && result.path == "package.json")
            .unwrap();
        let preview = db
            .file_preview(hit.node_id, PreviewMode::Rendered, true)
            .unwrap();

        let html = preview.rendered_html.unwrap();
        assert!(html.starts_with("<pre><code>"));
        assert!(html.contains("&quot;name&quot;"));
        assert!(preview.headings.is_empty());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn ai_explain_target_requires_a_registered_unprotected_file() {
        let db = Db::open_memory().unwrap();
        let projects = db.projects_list().unwrap();
        let normal_project_id = projects
            .iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .expect("normal fixture project")
            .id;
        let sensitive_project_id = projects
            .iter()
            .find(|project| project.name == "Fixture Sensitive Project")
            .expect("sensitive fixture project")
            .id;
        let normal = db
            .quick_open("README.md", 20)
            .unwrap()
            .into_iter()
            .find(|item| item.project_id == normal_project_id)
            .expect("normal fixture file");
        let target = db
            .ai_explain_target(normal.node_id)
            .unwrap()
            .expect("registered file target");
        assert!(target.path.starts_with("fixture://markdown-project/"));
        assert!(!target.is_sensitive);
        assert!(target.protected_level.is_none());
        assert!(!target.is_reparse);
        assert!(target.reparse_kind.is_none());
        assert_eq!(
            db.ai_explain_project_paths(normal.node_id).unwrap(),
            vec!["fixture://markdown-project".to_string()]
        );

        let sensitive = db
            .quick_open(".env", 20)
            .unwrap()
            .into_iter()
            .find(|item| item.project_id == sensitive_project_id)
            .expect("sensitive fixture file");
        let target = db
            .ai_explain_target(sensitive.node_id)
            .unwrap()
            .expect("registered sensitive target");
        assert!(target.is_sensitive || target.protected_level.is_some());
        assert!(db.ai_explain_target(i64::MAX).unwrap().is_none());
    }

    #[test]
    fn sensitive_fixture_can_be_revealed_transiently() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let env_path = dir.path().join(".env");
        fs::write(&env_path, "SECRET=value").unwrap();
        let files = [ScannedFile {
            absolute_path: env_path.to_string_lossy().to_string(),
            relative_path: ".env".to_string(),
            display_path: ".env".to_string(),
            display_name: ".env".to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: true,
            protected_level: Some("no_preview".to_string()),
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let hit = db.quick_open(".env", 10).unwrap().pop().unwrap();
        let preview = db.file_reveal(hit.node_id, PreviewMode::Source).unwrap();
        assert_eq!(preview.state, PreviewState::Ready);
        assert!(preview.was_revealed);
        assert_eq!(preview.source.as_deref(), Some("SECRET=value"));
        // Revealing a sensitive file must stay transient: it must never leave a
        // durable trail of which secrets were opened in the recent_item table.
        assert!(
            db.recent_items_list(20).unwrap().is_empty(),
            "revealing a sensitive file must not record a recent item"
        );
    }

    // Encryption-at-rest: a crash during the plaintext->encrypted migration must
    // never leave a readable plaintext copy of the DB on disk past the next open.
    // (File-backed open needs Windows DPAPI, so this is Windows-only.)
    #[cfg(windows)]
    #[test]
    fn open_sweeps_a_stale_plaintext_migration_leftover() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codehangar.sqlite3");
        // A real encrypted DB at the canonical path.
        {
            let db = Db::open(&path).unwrap();
            db.set_mcp_read_only_mode(true).unwrap();
        }
        // Simulate the crash window: a completed migration that never cleaned up its
        // plaintext copy. The content is irrelevant — reconcile keys off the canonical
        // DB being a valid encrypted one.
        let leftover = path.with_extension("sqlite3.plaintext-migrating");
        fs::write(
            &leftover,
            b"SQLite format 3\x00 plaintext copy of user data",
        )
        .unwrap();
        assert!(leftover.exists());

        // Re-opening must sweep the readable leftover and keep the real data.
        let db = Db::open(&path).unwrap();
        assert!(
            !leftover.exists(),
            "a readable plaintext copy of the database survived startup"
        );
        assert!(
            db.mcp_read_only_mode_value().unwrap(),
            "reconcile must not have discarded the real encrypted database"
        );
    }

    #[test]
    fn file_backed_reads_reuse_pooled_read_connections() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codehangar.sqlite3");
        let db = Db::open(&path).unwrap();

        db.set_mcp_read_only_mode(true).unwrap();
        // Serial reads each check a connection out and back in, so they reuse the pool rather than
        // opening (and re-deriving the cipher key for) a fresh connection every time.
        for _ in 0..12 {
            assert!(db.mcp_read_only_mode_value().unwrap());
        }
        let pooled = db.read_pool.lock().unwrap().len();
        assert!(
            pooled >= 1,
            "clean serial reads should leave a connection pooled for reuse"
        );
        assert!(
            pooled <= MAX_POOLED_READ_CONNS,
            "the read pool must stay capped at MAX_POOLED_READ_CONNS"
        );

        // A reused (pooled) read connection must still observe a write committed after it was pooled
        // — proving pooling never serves a stale snapshot.
        db.set_mcp_read_only_mode(false).unwrap();
        assert!(
            !db.mcp_read_only_mode_value().unwrap(),
            "a reused read connection must see later committed writes"
        );
    }

    #[test]
    fn configuring_an_additional_writer_does_not_checkpoint_the_wal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("writer-config.sqlite3");
        let key = hex_encode(&generate_key_material().unwrap());
        let primary = Connection::open(&path).unwrap();
        configure_file_connection(&primary, &key).unwrap();
        primary
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE payload(value BLOB);
                 INSERT INTO payload(value) VALUES(zeroblob(1048576));",
            )
            .unwrap();

        let wal_path = PathBuf::from(format!("{}-wal", path.to_string_lossy()));
        let before = fs::metadata(&wal_path).unwrap().len();
        assert!(before > 0, "the test must create a non-empty WAL");

        let additional = Connection::open(&path).unwrap();
        configure_file_connection(&additional, &key).unwrap();
        let after_configure = fs::metadata(&wal_path).unwrap().len();
        assert!(
            after_configure > 0,
            "opening a scan/recovery writer must not truncate the shared WAL"
        );

        checkpoint_stale_wal(&additional);
        let after_startup_checkpoint = fs::metadata(&wal_path).map(|meta| meta.len()).unwrap_or(0);
        assert!(
            after_startup_checkpoint < after_configure,
            "the explicit startup checkpoint should reclaim the WAL"
        );
    }

    // The legacy-blob re-wrap must replace the sole key file atomically (temp +
    // rename), leaving no truncated blob or stray temp, and the result must open via
    // the entropy path.
    #[cfg(windows)]
    #[test]
    fn atomic_rewrap_key_blob_replaces_the_blob_without_leaving_a_temp() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codehangar.sqlite3.key.dpapi");
        let key = generate_key_material().unwrap();
        atomic_rewrap_key_blob(&path, &key).unwrap();
        assert!(path.exists());
        let tmp = {
            let mut p: std::ffi::OsString = path.as_os_str().to_os_string();
            p.push(".rewrap.tmp");
            PathBuf::from(p)
        };
        assert!(!tmp.exists(), "re-wrap left a temp file behind");
        let (recovered, was_legacy) = unprotect_key_material(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(recovered, key);
        assert!(!was_legacy, "a re-wrapped blob must carry entropy");
    }

    #[test]
    fn investigation_report_counts_files_under_an_adhoc_root() {
        // Repro for the "0 files / empty preview" bug seen when investigating a folder. The
        // real scanner canonicalizes the walk root, so on Windows the indexed file nodes carry
        // an extended-length `\\?\C:\…` path that does NOT share scan_root.path's textual
        // prefix. The old path-prefix count silently returned zero (blocking the Gate-3 plan);
        // counting through nav_item.project_id is robust to that. We reproduce the mismatch by
        // indexing files whose absolute paths use the verbatim prefix while the registered root
        // path does not — exactly what `Path::canonicalize` yields on Windows.
        let db = Db::open_memory().unwrap();
        let root_path = r"C:\plain\project";
        let scanned_prefix = r"\\?\C:\plain\project";
        let root = db.roots_add_adhoc(root_path).unwrap();
        let mk = |abs: String, rel: &str, name: &str| ScannedFile {
            absolute_path: abs,
            relative_path: rel.to_string(),
            display_path: rel.to_string(),
            display_name: name.to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        };
        let files = [
            mk(format!(r"{scanned_prefix}\a.txt"), "a.txt", "a.txt"),
            mk(
                format!(r"{scanned_prefix}\models\llm.gguf"),
                "models/llm.gguf",
                "llm.gguf",
            ),
        ];
        db.load_scanned_root(root_path, &files, None).unwrap();

        let report = db.investigation_report(root.id).unwrap();
        assert!(
            report.root_node_id.is_some(),
            "the ad-hoc project node must resolve"
        );
        assert_eq!(
            report.file_count, 2,
            "investigation_report must count the indexed files even when the scanner \
             canonicalized their paths to a different prefix, got {}",
            report.file_count
        );
    }

    #[test]
    fn cloud_placeholder_files_are_not_opened_for_preview() {
        // A dehydrated online-only file (is_reparse=0, reparse_kind='cloud_placeholder')
        // must never be opened for a preview — that would hydrate/download it. The file
        // is a real, readable markdown file, so a Blocked state with no source proves the
        // guard short-circuited before read_disk_text_limited.
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let note_path = dir.path().join("notes.md");
        fs::write(&note_path, "ONLINE ONLY CONTENT").unwrap();
        let mut identity = file_identity(19, None, None, None);
        identity.reparse_kind = Some("cloud_placeholder".to_string()); // is_reparse stays false
        let files = [ScannedFile {
            absolute_path: note_path.to_string_lossy().to_string(),
            relative_path: "notes.md".to_string(),
            display_path: "notes.md".to_string(),
            display_name: "notes.md".to_string(),
            item_kind: "file".to_string(),
            is_markdown: true,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: Some(identity),
            body: None,
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let hit = db.quick_open("notes.md", 10).unwrap().pop().unwrap();
        // Even with reveal=true, the cloud placeholder must not be opened/hydrated.
        let preview = db
            .file_preview(hit.node_id, PreviewMode::Source, true)
            .unwrap();
        assert_eq!(preview.state, PreviewState::Blocked);
        assert!(
            preview.source.is_none(),
            "cloud placeholder content must not be read"
        );
        assert!(
            preview
                .blocked_reason
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains("online-only"),
            "blocked reason should explain it is online-only: {:?}",
            preview.blocked_reason
        );
    }

    #[test]
    fn preview_policy_controls_transient_sensitive_access() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let env_path = dir.path().join(".env");
        fs::write(&env_path, "TOKEN=value").unwrap();
        let files = [ScannedFile {
            absolute_path: env_path.to_string_lossy().to_string(),
            relative_path: ".env".to_string(),
            display_path: ".env".to_string(),
            display_name: ".env".to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: true,
            protected_level: Some("no_preview".to_string()),
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let hit = db.quick_open(".env", 10).unwrap().pop().unwrap();

        // Default policy: explicit reveal is disabled.
        let blocked = db
            .file_reveal_with_policy(hit.node_id, PreviewMode::Source, PreviewPolicy::default())
            .unwrap();
        assert_eq!(blocked.state, PreviewState::Blocked);

        // Relaxing the preview block WITHOUT the reveal consent must not expose
        // sensitive content. Auto-preview only layers on top of an explicit consent.
        let relaxed_without_consent = db
            .file_preview_with_policy(
                hit.node_id,
                PreviewMode::Source,
                false,
                PreviewPolicy {
                    allow_sensitive_reveal: false,
                    relax_non_strong_protected_preview: true,
                },
            )
            .unwrap();
        assert_eq!(relaxed_without_consent.state, PreviewState::Blocked);
        assert!(!relaxed_without_consent.was_revealed);
        assert!(relaxed_without_consent.source.is_none());

        // With the reveal consent granted, auto-preview surfaces the text transiently.
        let relaxed = db
            .file_preview_with_policy(
                hit.node_id,
                PreviewMode::Source,
                false,
                PreviewPolicy {
                    allow_sensitive_reveal: true,
                    relax_non_strong_protected_preview: true,
                },
            )
            .unwrap();
        assert_eq!(relaxed.state, PreviewState::Ready);
        assert!(relaxed.was_revealed);
        assert_eq!(relaxed.source.as_deref(), Some("TOKEN=value"));
    }

    #[test]
    fn strong_protected_zone_stays_blocked_under_reveal_and_relax() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let ssh_dir = dir.path().join(".ssh");
        fs::create_dir_all(&ssh_dir).unwrap();
        let key_path = ssh_dir.join("id_rsa");
        fs::write(&key_path, "PRIVATE-KEY").unwrap();
        let files = [ScannedFile {
            absolute_path: key_path.to_string_lossy().to_string(),
            relative_path: ".ssh/id_rsa".to_string(),
            display_path: ".ssh/id_rsa".to_string(),
            display_name: "id_rsa".to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: true,
            protected_level: Some("no_preview".to_string()),
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let hit = db.quick_open("id_rsa", 10).unwrap().pop().unwrap();
        let full_policy = PreviewPolicy {
            allow_sensitive_reveal: true,
            relax_non_strong_protected_preview: true,
        };

        // Explicit reveal of a strong zone is always blocked, even with full consent.
        let revealed = db
            .file_reveal_with_policy(hit.node_id, PreviewMode::Source, full_policy.clone())
            .unwrap();
        assert_eq!(revealed.state, PreviewState::Blocked);
        assert!(!revealed.was_revealed);
        assert!(revealed.source.is_none());

        // Relaxed auto-preview of a strong zone is likewise always blocked.
        let relaxed = db
            .file_preview_with_policy(hit.node_id, PreviewMode::Source, false, full_policy)
            .unwrap();
        assert_eq!(relaxed.state, PreviewState::Blocked);
        assert!(!relaxed.was_revealed);
        assert!(relaxed.source.is_none());
    }

    #[test]
    fn quick_open_finds_context_files() {
        let db = Db::open_memory().unwrap();
        let results = db.quick_open("agents", 10).unwrap();
        assert!(results.iter().any(|result| result.label == "AGENTS.md"));
    }

    #[test]
    fn quick_open_combines_file_and_project_terms_in_any_order() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "# Qualified project").unwrap();
        let outcome = hangar_fs::scan_markdown_context_root(dir.path()).unwrap();
        let root = dir.path().to_string_lossy();
        db.load_scanned_root(&root, &outcome.files, None).unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root)
            .unwrap();

        for query in [
            format!("README {}", project.name),
            format!("{} README", project.name),
        ] {
            let results = db.quick_open(&query, 10).unwrap();
            assert!(results
                .iter()
                .any(|result| { result.project_id == project.id && result.path == "README.md" }));
        }
        assert!(db
            .quick_open("README definitely-another-project", 10)
            .unwrap()
            .iter()
            .all(|result| result.project_id != project.id));
    }

    #[test]
    fn project_context_files_rank_root_docs_before_nested_readmes() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::create_dir_all(dir.path().join(".claude/commands")).unwrap();
        fs::create_dir_all(dir.path().join("crates/a")).unwrap();
        fs::create_dir_all(dir.path().join("crates/b")).unwrap();
        fs::write(dir.path().join("README.md"), "# Root").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Agents").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("docs/overview.md"), "# Overview").unwrap();
        fs::write(
            dir.path().join(".claude/commands/review.md"),
            "# Review command",
        )
        .unwrap();
        fs::write(dir.path().join("crates/a/README.md"), "# A").unwrap();
        fs::write(dir.path().join("crates/b/README.md"), "# B").unwrap();

        let outcome = hangar_fs::scan_markdown_context_root(dir.path()).unwrap();
        db.load_scanned_root(&dir.path().to_string_lossy(), &outcome.files, None)
            .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();
        let files = db.project_context_files(project.id).unwrap();
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths[..3], ["README.md", "AGENTS.md", "docs/overview.md"]);
        assert!(paths.contains(&".claude/commands/review.md"));
        assert!(paths.contains(&"package.json"));
        let claude = files
            .iter()
            .find(|file| file.path == ".claude/commands/review.md")
            .unwrap();
        assert_eq!(claude.context_group, "Agent instructions");
        assert!(claude.recommended);
        assert!(!paths.contains(&"crates/a/README.md"));
        assert!(!paths.contains(&"crates/b/README.md"));
    }

    #[test]
    fn fixture_git_metadata_is_passive() {
        let db = Db::open_memory().unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.name == "Fixture Git-like Project")
            .unwrap();
        let git = db.project_git_status(project.id).unwrap();
        assert!(git.has_git);
        assert_eq!(git.current_branch.as_deref(), Some("main"));
        assert_eq!(
            git.origin_url.as_deref(),
            Some("https://example.invalid/passive-only.git")
        );
    }

    #[test]
    fn dashboard_summarizes_indexed_fixture_data() {
        let db = Db::open_memory().unwrap();
        let dashboard = db.dashboard_summary().unwrap();

        assert_eq!(dashboard.total_projects, 3);
        assert!(dashboard.context_files >= 3);
        assert!(dashboard.total_items >= dashboard.context_files);
        assert!(dashboard.indexed_documents > 0);
        assert_eq!(dashboard.git_projects, 1);
        assert!(dashboard.sensitive_files >= 3);
        assert_eq!(dashboard.adapters_needing_review, 0);
        assert!(dashboard.stale_or_dirty.contains("No live disk check"));
        assert!(dashboard
            .largest_projects
            .iter()
            .all(|project| project.physical_bytes.is_some()));
    }

    #[test]
    fn dashboard_can_exclude_hidden_fixture_projects() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let readme_path = dir.path().join("README.md");
        fs::write(&readme_path, "# Real project\n\nLocal dashboard context.").unwrap();
        let readme = ScannedFile {
            is_markdown: true,
            is_context: true,
            body: Some("# Real project\n\nLocal dashboard context.".to_string()),
            ..scanned_fixture_for_path(&readme_path, "README.md")
        };
        db.load_scanned_root(&dir.path().to_string_lossy(), &[readme], None)
            .unwrap();

        let with_fixtures = db.dashboard_summary().unwrap();
        let real_only = db.dashboard_summary_filtered(false).unwrap();

        assert_eq!(with_fixtures.total_projects, 4);
        assert_eq!(real_only.total_projects, 1);
        assert_eq!(real_only.context_files, 1);
        assert!(real_only.total_items < with_fixtures.total_items);
        assert_eq!(real_only.git_projects, 0);
        assert_eq!(real_only.largest_projects.len(), 1);
        assert_eq!(
            real_only.largest_projects[0].path,
            dir.path().to_string_lossy()
        );
    }

    #[test]
    fn builtin_adapters_are_seeded() {
        let db = Db::open_memory().unwrap();
        let adapters = db.adapters_list().unwrap();

        assert!(adapters
            .iter()
            .any(|adapter| adapter.name == "generic_markdown_context" && adapter.enabled));
        assert!(adapters
            .iter()
            .any(|adapter| adapter.name == "generic_git_project" && adapter.enabled));
        assert!(adapters.iter().any(|adapter| {
            adapter.name == "generic_model_workflow_assets" && adapter.description.contains("model")
        }));
    }

    #[test]
    fn dashboard_defers_live_disk_stale_check() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("stale.txt");
        fs::write(&file_path, "old").unwrap();
        let files = [ScannedFile {
            identity: Some(FileIdentity {
                size_apparent: Some(3),
                size_allocated: None,
                modified_at: Some("1".to_string()),
                readonly: false,
                is_symlink: false,
                is_reparse: false,
                reparse_kind: None,
                volume_id: None,
                inode_key: None,
                link_count: None,
                inaccessible: false,
                error: None,
            }),
            ..scanned_fixture_for_path(&file_path, "stale.txt")
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        fs::write(&file_path, "newer").unwrap();

        let dashboard = db.dashboard_summary().unwrap();

        assert!(dashboard.stale_or_dirty.contains("fingerprinted files"));
        assert!(dashboard
            .stale_or_dirty
            .contains("deferred to explicit rescan"));
    }

    #[test]
    fn dashboard_deduplicates_physical_bytes_by_file_identity() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let files = [
            ScannedFile {
                absolute_path: dir.path().join("a.bin").to_string_lossy().to_string(),
                relative_path: "a.bin".to_string(),
                display_path: "a.bin".to_string(),
                display_name: "a.bin".to_string(),
                item_kind: "file".to_string(),
                is_markdown: false,
                is_context: false,
                is_sensitive: false,
                protected_level: None,
                child_count: 0,
                fully_scanned: true,
                collapse_default: false,
                scan_error: None,
                identity: Some(file_identity(100, Some(4096), Some("vol"), Some("inode"))),
                body: None,
            },
            ScannedFile {
                absolute_path: dir.path().join("b.bin").to_string_lossy().to_string(),
                relative_path: "b.bin".to_string(),
                display_path: "b.bin".to_string(),
                display_name: "b.bin".to_string(),
                item_kind: "file".to_string(),
                is_markdown: false,
                is_context: false,
                is_sensitive: false,
                protected_level: None,
                child_count: 0,
                fully_scanned: true,
                collapse_default: false,
                scan_error: None,
                identity: Some(file_identity(100, Some(4096), Some("vol"), Some("inode"))),
                body: None,
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let project = db
            .dashboard_summary()
            .unwrap()
            .largest_projects
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();
        assert_eq!(project.apparent_bytes, 200);
        assert_eq!(project.allocated_bytes, Some(8192));
        assert_eq!(project.physical_bytes, Some(4096));
        assert!(!project.footprint_partial);
    }

    #[test]
    fn partial_nav_aggregates_are_marked_lower_bound() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let files = [ScannedFile {
            absolute_path: dir.path().join("heavy").to_string_lossy().to_string(),
            relative_path: "heavy".to_string(),
            display_path: "heavy".to_string(),
            display_name: "heavy".to_string(),
            item_kind: "directory".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: false,
            collapse_default: true,
            scan_error: Some("Directory item limit reached".to_string()),
            identity: Some(file_identity(0, None, None, None)),
            body: None,
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();
        let nav_item = db
            .project_nav_tree(project.id)
            .unwrap()
            .into_iter()
            .find(|item| item.path == "heavy")
            .unwrap();
        assert!(nav_item.aggregate_bytes_partial);
        let dashboard_project = db
            .dashboard_summary()
            .unwrap()
            .largest_projects
            .into_iter()
            .find(|summary| summary.project_id == project.id)
            .unwrap();
        assert!(dashboard_project.footprint_partial);
    }

    #[test]
    fn project_nav_children_pages_root_and_directory_children() {
        let db = Db::open_memory().unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .unwrap();

        let root = db.project_nav_children(project.id, None, 2, 0).unwrap();
        assert_eq!(root.items.len(), 2);
        assert!(root.has_more);

        let docs = db
            .quick_open("overview", 10)
            .unwrap()
            .into_iter()
            .find(|result| result.path == "docs/overview.md")
            .unwrap();
        let docs_parent = db
            .project_nav_tree(project.id)
            .unwrap()
            .into_iter()
            .find(|item| item.path == "docs")
            .unwrap();
        let children = db
            .project_nav_children(project.id, Some(docs_parent.id), 20, 0)
            .unwrap();
        assert!(children
            .items
            .iter()
            .any(|item| item.node_id == Some(docs.node_id)));

        let reveal_path = db.project_nav_path(project.id, docs.node_id).unwrap();
        assert_eq!(
            reveal_path.first().and_then(|item| item.parent_nav_id),
            None
        );
        assert_eq!(
            reveal_path.last().and_then(|item| item.node_id),
            Some(docs.node_id)
        );
        assert!(reveal_path
            .windows(2)
            .all(|pair| pair[1].parent_nav_id == Some(pair[0].id)));
    }

    #[test]
    fn folder_explanation_classifies_documentation_without_overstating() {
        let db = Db::open_memory().unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .unwrap();
        let docs = db
            .project_nav_tree(project.id)
            .unwrap()
            .into_iter()
            .find(|item| item.path == "docs")
            .unwrap();

        let explanation = db.folder_explanation(docs.id).unwrap().unwrap();

        assert_eq!(explanation.classification, "documentation-context");
        assert_eq!(explanation.confidence, "medium");
        assert!(explanation.summary.contains("likely"));
        assert!(explanation
            .signals
            .iter()
            .any(|signal| signal.contains("direct child")));
    }

    #[test]
    fn folder_explanation_recognises_ai_tool_config_dirs() {
        // The tool dotfolder itself, and anything nested under it, is AI-tool config.
        let (kind, conf, _) = explain_folder_classification(
            ".claude",
            "C:/proj/.claude",
            "directory",
            None,
            false,
            false,
        );
        assert_eq!(kind, "ai-tool-config");
        assert_eq!(conf, "high");
        let (kind_sub, _, _) = explain_folder_classification(
            "rules",
            "C:/proj/.cursor/rules",
            "directory",
            None,
            false,
            false,
        );
        assert_eq!(kind_sub, "ai-tool-config");
        // An ordinary source folder must NOT be misclassified as AI-tool config.
        let (kind_src, _, _) =
            explain_folder_classification("src", "C:/proj/src", "directory", None, false, false);
        assert_ne!(kind_src, "ai-tool-config");
    }

    #[test]
    fn markdown_local_links_create_relationship_edges() {
        let db = Db::open_memory().unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .unwrap();
        let readme = db
            .quick_open("README.md", 20)
            .unwrap()
            .into_iter()
            .find(|result| result.project_id == project.id && result.path == "README.md")
            .unwrap();
        let overview = db
            .quick_open("overview", 20)
            .unwrap()
            .into_iter()
            .find(|result| result.project_id == project.id && result.path == "docs/overview.md")
            .unwrap();

        let readme_relationships = db.node_relationships(readme.node_id).unwrap();
        assert!(readme_relationships.outgoing.iter().any(|relationship| {
            relationship.node_id == overview.node_id
                && relationship.kind == "markdown_links_to"
                && relationship.confidence == "High"
        }));

        let overview_relationships = db.node_relationships(overview.node_id).unwrap();
        assert!(overview_relationships
            .incoming
            .iter()
            .any(|relationship| relationship.node_id == readme.node_id));
    }

    #[test]
    fn markdown_relationships_report_missing_and_ambiguous_links() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let readme_body =
            "# Links\n\n[Missing](missing.md)\n\n[Ambiguous](shared.md)\n".to_string();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::create_dir_all(dir.path().join("prompts")).unwrap();
        fs::write(dir.path().join("README.md"), &readme_body).unwrap();
        fs::write(dir.path().join("docs/shared.md"), "# Shared A").unwrap();
        fs::write(dir.path().join("prompts/shared.md"), "# Shared B").unwrap();
        let files = [
            ScannedFile {
                display_name: "README.md".to_string(),
                is_markdown: true,
                is_context: true,
                identity: Some(file_identity(readme_body.len() as u64, None, None, None)),
                body: Some(readme_body),
                ..scanned_fixture_for_path(&dir.path().join("README.md"), "README.md")
            },
            ScannedFile {
                display_name: "shared.md".to_string(),
                is_markdown: true,
                is_context: true,
                identity: Some(file_identity(10, None, None, None)),
                body: Some("# Shared A".to_string()),
                ..scanned_fixture_for_path(&dir.path().join("docs/shared.md"), "docs/shared.md")
            },
            ScannedFile {
                display_name: "shared.md".to_string(),
                is_markdown: true,
                is_context: true,
                identity: Some(file_identity(10, None, None, None)),
                body: Some("# Shared B".to_string()),
                ..scanned_fixture_for_path(
                    &dir.path().join("prompts/shared.md"),
                    "prompts/shared.md",
                )
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();
        let readme = db
            .quick_open("README.md", 20)
            .unwrap()
            .into_iter()
            .find(|result| result.project_id == project.id && result.path == "README.md")
            .unwrap();

        let relationships = db.node_relationships(readme.node_id).unwrap();

        assert!(relationships.issues.iter().any(|issue| {
            issue.kind == "unresolved_markdown_link"
                && issue.target == "missing.md"
                && issue.confidence == "Medium"
        }));
        assert!(relationships.issues.iter().any(|issue| {
            issue.kind == "ambiguous_markdown_link"
                && issue.target == "shared.md"
                && issue.confidence == "Low"
        }));
        let low_edges = relationships
            .outgoing
            .iter()
            .filter(|relationship| {
                relationship.display_name == "shared.md" && relationship.confidence == "Low"
            })
            .count();
        assert_eq!(low_edges, 2);
    }

    #[test]
    fn workflow_model_references_build_graph_edges_and_missing_issues() {
        let root = tempdir().unwrap();
        let model_dir = root.path().join("models").join("checkpoints");
        let workflow_dir = root.path().join("user").join("default").join("workflows");
        fs::create_dir_all(&model_dir).unwrap();
        fs::create_dir_all(&workflow_dir).unwrap();
        let model_path = model_dir.join("base.safetensors");
        let gguf_path = model_dir.join("local.gguf");
        let workflow_path = workflow_dir.join("video.json");
        fs::write(
            &model_path,
            safetensors_fixture_bytes(
                br#"{"weight":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}"#,
            ),
        )
        .unwrap();
        fs::write(&gguf_path, gguf_fixture_bytes(3, 7, 4)).unwrap();
        fs::write(
            &workflow_path,
            br#"{"nodes":{"1":{"inputs":{"ckpt_name":"base.safetensors"}},"2":{"inputs":{"lora_name":"missing.safetensors"}}}}"#,
        )
        .unwrap();

        let db = Db::open_memory().unwrap();
        let files = vec![
            scanned_fixture_for_path(&model_path, "models/checkpoints/base.safetensors"),
            scanned_fixture_for_path(&gguf_path, "models/checkpoints/local.gguf"),
            scanned_fixture_for_path(&workflow_path, "user/default/workflows/video.json"),
        ];
        db.load_scanned_root(root.path().to_str().unwrap(), &files, None)
            .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root.path().to_string_lossy())
            .unwrap();
        let (workflow_node_id, model_node_id) = db
            .with_conn(|conn| {
                let workflow_node_id = conn.query_row(
                    "SELECT node_id FROM nav_item WHERE project_id = ?1 AND path = ?2",
                    params![project.id, "user/default/workflows/video.json"],
                    |row| row.get::<_, i64>(0),
                )?;
                let model_node_id = conn.query_row(
                    "SELECT node_id FROM nav_item WHERE project_id = ?1 AND path = ?2",
                    params![project.id, "models/checkpoints/base.safetensors"],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok((workflow_node_id, model_node_id))
            })
            .unwrap();

        let relationships = db.node_relationships(workflow_node_id).unwrap();
        assert!(
            relationships.outgoing.iter().any(|relationship| {
                relationship.node_id == model_node_id
                    && relationship.kind == "workflow_references_model"
                    && relationship.confidence == "Medium"
            }),
            "relationships: {relationships:?}"
        );
        assert!(relationships.issues.iter().any(|issue| {
            issue.kind == "missing_model_reference" && issue.target == "missing.safetensors"
        }));

        let map = db.project_graph_map(project.id, 100).unwrap();
        assert!(map.nodes.iter().any(|node| node.graph_kind == "workflow"));
        assert!(map
            .nodes
            .iter()
            .any(|node| node.graph_kind == "model:checkpoint"));
        let safetensors_node = map
            .nodes
            .iter()
            .find(|node| node.path == "models/checkpoints/base.safetensors")
            .unwrap();
        assert!(safetensors_node
            .details
            .iter()
            .any(|detail| detail == "1 tensor"));
        assert!(safetensors_node
            .details
            .iter()
            .any(|detail| detail == "Dtypes: F16"));
        let gguf_node = map
            .nodes
            .iter()
            .find(|node| node.path == "models/checkpoints/local.gguf")
            .unwrap();
        assert!(gguf_node.details.contains(&"GGUF v3".to_string()));
        assert!(gguf_node.details.contains(&"7 tensors".to_string()));
        assert!(map.edges.iter().any(|edge| {
            edge.source_node_id == workflow_node_id
                && edge.target_node_id == model_node_id
                && edge.kind == "workflow_references_model"
        }));
        assert_eq!(map.total_issues, 1);
        assert_eq!(
            map.issues[0].source_path.as_deref(),
            Some("user/default/workflows/video.json")
        );
    }

    #[test]
    fn ci_and_vendored_workflow_files_do_not_pollute_the_model_graph() {
        let root = tempdir().unwrap();
        let workflow_dir = root.path().join(".github").join("workflows");
        fs::create_dir_all(&workflow_dir).unwrap();
        let ci_path = workflow_dir.join("ci.yml");
        let ci_json_path = workflow_dir.join("release.json");
        let cargo_workflow = root
            .path()
            .join(".local/cargo/registry/src/pkg/workflows/test.json");
        let node_modules_workflow = root.path().join("node_modules/pkg/workflow/test.json");
        fs::create_dir_all(cargo_workflow.parent().unwrap()).unwrap();
        fs::create_dir_all(node_modules_workflow.parent().unwrap()).unwrap();
        fs::write(&ci_path, "name: CI\non:\n  push:\n").unwrap();
        fs::write(&ci_json_path, "not model workflow json").unwrap();
        fs::write(&cargo_workflow, "not model workflow json").unwrap();
        fs::write(&node_modules_workflow, "not model workflow json").unwrap();

        let db = Db::open_memory().unwrap();
        db.load_scanned_root(
            root.path().to_str().unwrap(),
            &[
                scanned_fixture_for_path(&ci_path, ".github/workflows/ci.yml"),
                scanned_fixture_for_path(&ci_json_path, ".github/workflows/release.json"),
                scanned_fixture_for_path(
                    &cargo_workflow,
                    ".local/cargo/registry/src/pkg/workflows/test.json",
                ),
                scanned_fixture_for_path(
                    &node_modules_workflow,
                    "node_modules/pkg/workflow/test.json",
                ),
            ],
            None,
        )
        .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root.path().to_string_lossy())
            .unwrap();

        let map = db.project_graph_map(project.id, 100).unwrap();
        assert!(map
            .nodes
            .iter()
            .all(|node| !node.path.contains("workflows/") && !node.path.contains("workflow/")));
        assert_eq!(map.total_issues, 0);
        assert!(map
            .issues
            .iter()
            .all(|issue| issue.kind != "workflow_parse_error"));
    }

    #[test]
    fn graph_map_limit_allows_controlled_expansion_past_the_old_ceiling() {
        assert_eq!(project_graph_map_limit(300), 300);
        assert_eq!(project_graph_map_limit(1_500), 1_500);
        assert_eq!(
            project_graph_map_limit(MAX_PROJECT_GRAPH_MAP_NODES + 1),
            MAX_PROJECT_GRAPH_MAP_NODES
        );
    }

    #[test]
    fn workflow_graph_marks_ambiguous_models_and_skips_sensitive_workflows() {
        let root = tempdir().unwrap();
        let model_a = root
            .path()
            .join("models")
            .join("a")
            .join("shared.safetensors");
        let model_b = root
            .path()
            .join("models")
            .join("b")
            .join("shared.safetensors");
        let workflow = root.path().join("workflows").join("ambiguous.json");
        let sensitive_workflow = root.path().join("workflows").join("credentials.json");
        fs::create_dir_all(model_a.parent().unwrap()).unwrap();
        fs::create_dir_all(model_b.parent().unwrap()).unwrap();
        fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        fs::write(&model_a, b"first model").unwrap();
        fs::write(&model_b, b"second model").unwrap();
        fs::write(
            &workflow,
            br#"{"inputs":{"ckpt_name":"shared.safetensors"}}"#,
        )
        .unwrap();
        fs::write(
            &sensitive_workflow,
            br#"{"inputs":{"ckpt_name":"shared.safetensors"}}"#,
        )
        .unwrap();

        let mut sensitive =
            scanned_fixture_for_path(&sensitive_workflow, "workflows/credentials.json");
        sensitive.is_sensitive = true;
        let db = Db::open_memory().unwrap();
        db.load_scanned_root(
            root.path().to_str().unwrap(),
            &[
                scanned_fixture_for_path(&model_a, "models/a/shared.safetensors"),
                scanned_fixture_for_path(&model_b, "models/b/shared.safetensors"),
                scanned_fixture_for_path(&workflow, "workflows/ambiguous.json"),
                sensitive,
            ],
            None,
        )
        .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root.path().to_string_lossy())
            .unwrap();
        let (workflow_node_id, sensitive_node_id) = db
            .with_conn(|conn| {
                let workflow_node_id = conn.query_row(
                    "SELECT node_id FROM nav_item WHERE project_id = ?1 AND path = 'workflows/ambiguous.json'",
                    params![project.id],
                    |row| row.get::<_, i64>(0),
                )?;
                let sensitive_node_id = conn.query_row(
                    "SELECT node_id FROM nav_item WHERE project_id = ?1 AND path = 'workflows/credentials.json'",
                    params![project.id],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok((workflow_node_id, sensitive_node_id))
            })
            .unwrap();

        let relationships = db.node_relationships(workflow_node_id).unwrap();
        assert_eq!(
            relationships
                .outgoing
                .iter()
                .filter(|relationship| relationship.kind == "workflow_references_model")
                .count(),
            2,
            "relationships: {relationships:?}"
        );
        assert!(relationships
            .outgoing
            .iter()
            .all(|relationship| relationship.confidence == "Low"));
        assert!(relationships.issues.iter().any(|issue| {
            issue.kind == "ambiguous_model_reference" && issue.target == "shared.safetensors"
        }));
        let sensitive_relationships = db.node_relationships(sensitive_node_id).unwrap();
        assert!(sensitive_relationships.outgoing.is_empty());
        assert!(sensitive_relationships.issues.is_empty());
    }

    #[test]
    fn graph_map_marks_duplicate_model_candidates_without_persisting_issue() {
        let root = tempdir().unwrap();
        let model_a = root.path().join("models").join("a").join("dup.safetensors");
        let model_b = root.path().join("models").join("b").join("dup.safetensors");
        fs::create_dir_all(model_a.parent().unwrap()).unwrap();
        fs::create_dir_all(model_b.parent().unwrap()).unwrap();
        let mut bytes = safetensors_fixture_bytes(
            br#"{"weight":{"dtype":"F16","shape":[1024],"data_offsets":[0,2048]}}"#,
        );
        bytes.extend_from_slice(&vec![7_u8; 2048]);
        fs::write(&model_a, &bytes).unwrap();
        fs::write(&model_b, &bytes).unwrap();

        let db = Db::open_memory().unwrap();
        let mut scanned_a = scanned_fixture_for_path(&model_a, "models/a/dup.safetensors");
        scanned_a.identity = Some(file_identity(bytes.len() as u64, None, None, None));
        let mut scanned_b = scanned_fixture_for_path(&model_b, "models/b/dup.safetensors");
        scanned_b.identity = Some(file_identity(bytes.len() as u64, None, None, None));
        db.load_scanned_root(root.path().to_str().unwrap(), &[scanned_a, scanned_b], None)
            .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root.path().to_string_lossy())
            .unwrap();

        let map = db.project_graph_map(project.id, 100).unwrap();
        assert!(map
            .issues
            .iter()
            .any(|issue| issue.kind == "duplicate_model_candidate"
                && issue.confidence == "Medium"
                && issue
                    .evidence
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Full hash confirmation is deferred")));
        assert!(map
            .edges
            .iter()
            .any(|edge| edge.kind == "duplicate_model_candidate" && edge.confidence == "Medium"));
        let persisted_duplicate_issues = db
            .with_conn(|conn| {
                count_i64(
                    conn,
                    "SELECT COUNT(*) FROM relationship_issue WHERE kind = 'duplicate_model_candidate'",
                )
            })
            .unwrap();
        assert_eq!(persisted_duplicate_issues, 0);
    }

    #[test]
    fn graph_map_excludes_cloud_placeholder_models_from_duplicate_hashing() {
        // A dehydrated OneDrive/Dropbox model has is_reparse=0 but
        // reparse_kind='cloud_placeholder'. It must be excluded from duplicate hashing
        // so the graph never opens (hydrates) it — even when a same-size, same-content
        // sibling exists that would otherwise pair into a duplicate group.
        let root = tempdir().unwrap();
        let model_a = root.path().join("models").join("a").join("dup.safetensors");
        let model_b = root.path().join("models").join("b").join("dup.safetensors");
        fs::create_dir_all(model_a.parent().unwrap()).unwrap();
        fs::create_dir_all(model_b.parent().unwrap()).unwrap();
        let mut bytes = safetensors_fixture_bytes(
            br#"{"weight":{"dtype":"F16","shape":[1024],"data_offsets":[0,2048]}}"#,
        );
        bytes.extend_from_slice(&vec![7_u8; 2048]);
        fs::write(&model_a, &bytes).unwrap();
        fs::write(&model_b, &bytes).unwrap();

        let db = Db::open_memory().unwrap();
        let mut scanned_a = scanned_fixture_for_path(&model_a, "models/a/dup.safetensors");
        scanned_a.identity = Some(file_identity(bytes.len() as u64, None, None, None));
        let mut scanned_b = scanned_fixture_for_path(&model_b, "models/b/dup.safetensors");
        let mut identity_b = file_identity(bytes.len() as u64, None, None, None);
        identity_b.reparse_kind = Some("cloud_placeholder".to_string()); // is_reparse stays false
        scanned_b.identity = Some(identity_b);
        db.load_scanned_root(root.path().to_str().unwrap(), &[scanned_a, scanned_b], None)
            .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root.path().to_string_lossy())
            .unwrap();

        let map = db.project_graph_map(project.id, 100).unwrap();
        assert!(
            !map.issues
                .iter()
                .any(|issue| issue.kind == "duplicate_model_candidate"),
            "cloud placeholder must be excluded from duplicate hashing: {:?}",
            map.issues
        );
        assert!(
            !map.edges
                .iter()
                .any(|edge| edge.kind == "duplicate_model_candidate"),
            "no duplicate edge should involve the cloud placeholder"
        );
    }

    #[test]
    fn graph_map_marks_shared_cache_candidates_with_details() {
        let root = tempdir().unwrap();
        let cache_dir = root.path().join(".cache").join("huggingface").join("hub");
        fs::create_dir_all(&cache_dir).unwrap();
        let mut cache = scanned_fixture_for_path(&cache_dir, ".cache/huggingface/hub");
        cache.item_kind = "directory".to_string();
        cache.display_name = "hub".to_string();

        let db = Db::open_memory().unwrap();
        db.load_scanned_root(root.path().to_str().unwrap(), &[cache], None)
            .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == root.path().to_string_lossy())
            .unwrap();

        let map = db.project_graph_map(project.id, 100).unwrap();
        let cache_node = map
            .nodes
            .iter()
            .find(|node| node.path == ".cache/huggingface/hub")
            .unwrap();
        assert_eq!(cache_node.graph_kind, "cache");
        assert!(cache_node
            .details
            .contains(&"Hugging Face cache".to_string()));
        assert!(cache_node
            .details
            .contains(&"Usually shared by multiple projects or tools".to_string()));
        assert!(map.issues.iter().any(|issue| {
            issue.kind == "shared_cache_candidate"
                && issue.node_id == cache_node.node_id
                && issue.confidence == "Medium"
                && issue
                    .evidence
                    .as_deref()
                    .unwrap_or_default()
                    .contains("normally a shared tool/model cache")
        }));
    }

    #[test]
    fn orphan_finder_lists_unreferenced_assets_only() {
        let db = Db::open_memory().unwrap();
        let orphans = db.graph_orphans(20).unwrap();

        assert!(orphans
            .candidates
            .iter()
            .any(|candidate| candidate.path == "assets/unused.png"
                && candidate.confidence == "Medium"));
        assert!(!orphans
            .candidates
            .iter()
            .any(|candidate| candidate.path == "docs/diagram.png"));
        assert!(!orphans
            .candidates
            .iter()
            .any(|candidate| candidate.path == ".git/config"));
    }

    #[test]
    fn orphan_finder_excludes_installed_python_dependencies() {
        assert!(classify_orphan_candidate(
            "tools/tts-xb-venv/Lib/site-packages/onnx/backend/test/data/model.onnx"
        )
        .is_none());
        assert!(classify_orphan_candidate(
            "wsl/usr/lib/python3/dist-packages/torch/test/checkpoint.pt"
        )
        .is_none());
        assert!(classify_orphan_candidate("models/checkpoint.pt").is_some());
    }

    #[test]
    fn node_orphan_status_evaluates_single_file() {
        let db = Db::open_memory().unwrap();
        let orphan = db
            .graph_orphans(20)
            .unwrap()
            .candidates
            .into_iter()
            .find(|candidate| candidate.path == "assets/unused.png")
            .unwrap();
        let status = db.node_orphan_status(orphan.node_id).unwrap();

        assert!(status.evaluated);
        assert!(status.is_candidate);
        assert_eq!(status.incoming_references, 0);

        let readme = db
            .quick_open("README.md", 20)
            .unwrap()
            .into_iter()
            .find(|result| result.path == "README.md")
            .unwrap();
        let readme_status = db.node_orphan_status(readme.node_id).unwrap();
        assert!(readme_status.evaluated);
        assert!(!readme_status.is_candidate);
    }

    #[test]
    fn lost_project_name_marker_matches_whole_tokens_only() {
        // Whole-token markers are flagged, including across separators and digits.
        assert!(has_lost_project_name_marker(
            "old-experiment",
            "projects/old-experiment"
        ));
        assert!(has_lost_project_name_marker("draft2", "x/draft2"));
        assert!(has_lost_project_name_marker("Archive", "y/Archive"));
        assert!(has_lost_project_name_marker("api", "tmp/unused/api"));
        // Names that merely contain a marker as a substring must not trigger.
        assert!(!has_lost_project_name_marker(
            "latest-pipeline",
            "build/latest-pipeline"
        ));
        assert!(!has_lost_project_name_marker(
            "gold-tracker",
            "apps/gold-tracker"
        ));
        assert!(!has_lost_project_name_marker("contest", "web/contest"));
    }

    #[test]
    fn orphan_and_lost_project_searches_are_explicitly_filterable() {
        let db = Db::open_memory().unwrap();
        let hidden_assets = db
            .orphan_asset_candidates(OrphanAssetSearchOptions {
                min_size_bytes: Some(0),
                project_id: None,
                asset_kind: None,
                min_confidence: None,
                include_partial: false,
                include_fixture_projects: false,
                limit: 20,
            })
            .unwrap();
        assert!(hidden_assets.candidates.is_empty());
        let assets = db
            .orphan_asset_candidates(OrphanAssetSearchOptions {
                min_size_bytes: Some(0),
                project_id: None,
                asset_kind: Some("image"),
                min_confidence: Some("Medium"),
                include_partial: false,
                include_fixture_projects: true,
                limit: 20,
            })
            .unwrap();
        assert!(assets
            .candidates
            .iter()
            .any(|candidate| candidate.path == "assets/unused.png"));
        let lost = db
            .lost_project_candidates(LostProjectSearchOptions {
                min_size_bytes: Some(0),
                project_id: None,
                stale_preset: Some("quiet"),
                signals: &["no_recent_opens".to_string()],
                keyword: None,
                include_partial: true,
                include_fixture_projects: true,
                limit: 20,
            })
            .unwrap();
        assert!(!lost.candidates.is_empty());
        assert!(lost.candidates.iter().all(|candidate| candidate
            .signals
            .iter()
            .any(|signal| signal == "no_recent_opens")));
        // Every lost candidate exposes a node_id usable as an Operation Plan target;
        // for a project candidate that node is the project itself.
        assert!(lost
            .candidates
            .iter()
            .all(|candidate| candidate.node_id.is_some()));
        assert!(lost
            .candidates
            .iter()
            .all(|candidate| candidate.candidate_kind != "project"
                || candidate.node_id == Some(candidate.project_id)));
        let hidden_lost = db
            .lost_project_candidates(LostProjectSearchOptions {
                min_size_bytes: Some(0),
                project_id: None,
                stale_preset: Some("any"),
                signals: &[],
                keyword: None,
                include_partial: true,
                include_fixture_projects: false,
                limit: 20,
            })
            .unwrap();
        assert!(hidden_lost.candidates.is_empty());
        let keyword_lost = db
            .lost_project_candidates(LostProjectSearchOptions {
                min_size_bytes: Some(0),
                project_id: None,
                stale_preset: Some("custom"),
                signals: &[],
                keyword: Some("fixture"),
                include_partial: true,
                include_fixture_projects: true,
                limit: 20,
            })
            .unwrap();
        assert!(keyword_lost.candidates.iter().any(|candidate| candidate
            .signals
            .iter()
            .any(|signal| signal == "keyword_match")));
    }

    #[test]
    fn duplicate_candidates_can_exclude_fixture_projects() {
        let db = Db::open_memory().unwrap();
        let root = "fixture://hidden-duplicate-project";
        let body = "fixture-duplicate-payload\n".repeat(96);
        let size = body.len() as u64;
        let fixture_files = ["fixture-a.dat", "fixture-b.dat"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| ScannedFile {
                absolute_path: format!("{root}/{name}"),
                relative_path: name.to_string(),
                display_path: name.to_string(),
                display_name: name.to_string(),
                item_kind: "file".to_string(),
                is_markdown: false,
                is_context: false,
                is_sensitive: false,
                protected_level: None,
                child_count: 0,
                fully_scanned: true,
                collapse_default: false,
                scan_error: None,
                identity: Some(file_identity(
                    size,
                    Some(size),
                    Some("fixture-volume"),
                    Some(if index == 0 { "fixture-a" } else { "fixture-b" }),
                )),
                body: Some(body.clone()),
            })
            .collect::<Vec<_>>();
        db.load_scanned_root(root, &fixture_files, None).unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE node SET attributes = '{\"source\":\"fixture\"}' WHERE kind = 'project' AND path = ?1",
                params![root],
            )?;
            Ok(())
        })
        .unwrap();

        let visible_fixtures = db
            .duplicate_candidates_filtered(Some(0), None, None, None, true, 50)
            .unwrap();
        assert!(visible_fixtures.groups.iter().any(|group| group
            .members
            .iter()
            .any(|member| member.path == "fixture-a.dat")));
        let hidden_fixtures = db
            .duplicate_candidates_filtered(Some(0), None, None, None, false, 50)
            .unwrap();
        assert!(hidden_fixtures.groups.is_empty());
    }

    #[test]
    fn duplicate_candidates_use_size_and_partial_hash_without_sensitive_files() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let duplicate_body = "duplicate-payload\n".repeat(96);
        let unique_body = format!("{}unique-tail", "duplicate-payload\n".repeat(95));
        fs::write(dir.path().join("copy-a.dat"), &duplicate_body).unwrap();
        fs::write(dir.path().join("copy-b.dat"), &duplicate_body).unwrap();
        fs::write(dir.path().join("unique.dat"), &unique_body).unwrap();
        fs::write(dir.path().join(".env"), &duplicate_body).unwrap();
        let duplicate_size = duplicate_body.len() as u64;
        let files = [
            ScannedFile {
                identity: Some(file_identity(
                    duplicate_size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-a"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("copy-a.dat"), "copy-a.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    duplicate_size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-b"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("copy-b.dat"), "copy-b.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    unique_body.len() as u64,
                    Some(4096),
                    Some("vol"),
                    Some("inode-c"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("unique.dat"), "unique.dat")
            },
            ScannedFile {
                relative_path: ".env".to_string(),
                display_path: ".env".to_string(),
                display_name: ".env".to_string(),
                is_sensitive: true,
                protected_level: Some("no_preview".to_string()),
                identity: Some(file_identity(
                    duplicate_size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-secret"),
                )),
                ..scanned_fixture_for_path(&dir.path().join(".env"), ".env")
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let duplicates = db.duplicate_candidates(10).unwrap();
        let group = duplicates
            .groups
            .iter()
            .find(|group| {
                let paths = group
                    .members
                    .iter()
                    .map(|member| member.path.as_str())
                    .collect::<HashSet<_>>();
                paths.contains("copy-a.dat") && paths.contains("copy-b.dat")
            })
            .unwrap();

        assert_eq!(group.confidence, "Medium");
        assert_eq!(group.member_count, 2);
        assert_eq!(group.physical_bytes, Some(8192));
        assert!(group.reason.contains("Full hash confirmation"));
        assert!(!group
            .members
            .iter()
            .any(|member| member.path == ".env" || member.path == "unique.dat"));
    }

    #[test]
    fn duplicate_candidates_do_not_treat_hardlinks_as_recoverable_duplicates() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let body = "same-hardlink-payload\n".repeat(96);
        fs::write(dir.path().join("hard-a.dat"), &body).unwrap();
        fs::write(dir.path().join("hard-b.dat"), &body).unwrap();
        let size = body.len() as u64;
        let files = [
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("same-inode"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("hard-a.dat"), "hard-a.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("same-inode"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("hard-b.dat"), "hard-b.dat")
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let duplicates = db.duplicate_candidates(10).unwrap();

        assert!(duplicates.groups.is_empty());
    }

    #[test]
    fn confirm_duplicate_group_confirms_byte_identical_only() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        // A and B are byte-identical; C shares the bounded 64 KiB partial-hash
        // prefix and the exact size, but differs AFTER the window, so it collides on
        // the partial hash only and must be excluded by full hashing.
        let prefix = "duplicate-payload\n".repeat(8192); // > 64 KiB shared prefix
        let duplicate_body = format!("{prefix}{}", "tail-aaaaaa\n".repeat(64));
        let collision_body = format!("{prefix}{}", "tail-bbbbbb\n".repeat(64));
        assert_eq!(duplicate_body.len(), collision_body.len());
        assert!(duplicate_body.len() > 64 * 1024);
        fs::write(dir.path().join("copy-a.dat"), &duplicate_body).unwrap();
        fs::write(dir.path().join("copy-b.dat"), &duplicate_body).unwrap();
        fs::write(dir.path().join("collide-c.dat"), &collision_body).unwrap();
        let size = duplicate_body.len() as u64;
        let files = [
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-a"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("copy-a.dat"), "copy-a.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-b"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("copy-b.dat"), "copy-b.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-c"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("collide-c.dat"), "collide-c.dat")
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        // All three share the size + partial hash, so they form one candidate group.
        let candidates = db.duplicate_candidates(10).unwrap();
        let group = candidates
            .groups
            .iter()
            .find(|group| {
                group
                    .members
                    .iter()
                    .any(|member| member.path == "copy-a.dat")
            })
            .unwrap();
        assert_eq!(group.member_count, 3);
        let node_a = group
            .members
            .iter()
            .find(|member| member.path == "copy-a.dat")
            .unwrap()
            .node_id;

        let confirmation = db.confirm_duplicate_group(node_a).unwrap();
        assert_eq!(confirmation.target_node_id, node_a);
        assert!(!confirmation.partial);
        assert_eq!(confirmation.confirmed_groups.len(), 1);
        let confirmed = &confirmation.confirmed_groups[0];
        assert_eq!(confirmed.confidence, "High");
        assert_eq!(confirmed.member_count, 2);
        // Reclaimable = one kept copy's physical footprint freed (two identical files).
        assert_eq!(confirmed.reclaimable_bytes, 4096);
        assert_eq!(confirmation.reclaimable_bytes, 4096);
        let paths: HashSet<&str> = confirmed
            .members
            .iter()
            .map(|member| member.path.as_str())
            .collect();
        assert!(paths.contains("copy-a.dat"));
        assert!(paths.contains("copy-b.dat"));
        assert!(!paths.contains("collide-c.dat"));
        // All three were full-hashed (C just landed in its own unique-hash bucket).
        assert_eq!(confirmation.checked_files, 3);
    }

    #[test]
    fn confirm_interruptible_reports_progress_and_honors_cancel() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let body = "interruptible-dup-payload\n".repeat(4096); // > 64 KiB → forms a candidate group
        fs::write(dir.path().join("dup-a.dat"), &body).unwrap();
        fs::write(dir.path().join("dup-b.dat"), &body).unwrap();
        let size = body.len() as u64;
        let files = [
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-a"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("dup-a.dat"), "dup-a.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-b"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("dup-b.dat"), "dup-b.dat")
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let group = db
            .duplicate_candidates(10)
            .unwrap()
            .groups
            .into_iter()
            .find(|g| g.members.iter().any(|m| m.path == "dup-a.dat"))
            .unwrap();
        let node_a = group
            .members
            .iter()
            .find(|m| m.path == "dup-a.dat")
            .unwrap()
            .node_id;

        // Not cancelled: reports progress and matches the synchronous confirm.
        let cancel = AtomicBool::new(false);
        let mut updates: Vec<hangar_core::DuplicateConfirmProgress> = Vec::new();
        let confirmation = db
            .confirm_duplicate_group_interruptible(node_a, &cancel, &mut |p| updates.push(p))
            .unwrap()
            .expect("an un-cancelled confirm returns Some");
        assert_eq!(confirmation.checked_files, 2);
        assert_eq!(confirmation.reclaimable_bytes, 4096);
        assert!(!updates.is_empty(), "progress must be reported");
        let last = updates.last().unwrap();
        assert_eq!(last.checked_files, 2);
        assert_eq!(last.total_files, 2);
        assert_eq!(last.bytes_hashed, last.total_bytes);

        // Pre-cancelled: returns None (nothing is confirmed; the work is read-only regardless).
        let cancelled = AtomicBool::new(true);
        let mut noop = |_| {};
        let result = db
            .confirm_duplicate_group_interruptible(node_a, &cancelled, &mut noop)
            .unwrap();
        assert!(result.is_none(), "a pre-cancelled confirm returns None");
    }

    #[test]
    fn confirm_duplicate_group_hardlinks_not_double_counted() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        // hard-a and hard-b share an inode (a hardlink to one physical file); copy-c
        // is a distinct physical file with identical content. Reclaimable must count
        // the hardlink's footprint once, not twice.
        let body = "hardlink-confirm-payload\n".repeat(96);
        fs::write(dir.path().join("hard-a.dat"), &body).unwrap();
        fs::write(dir.path().join("hard-b.dat"), &body).unwrap();
        fs::write(dir.path().join("copy-c.dat"), &body).unwrap();
        let size = body.len() as u64;
        let files = [
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("same-inode"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("hard-a.dat"), "hard-a.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("same-inode"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("hard-b.dat"), "hard-b.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("inode-c"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("copy-c.dat"), "copy-c.dat")
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        // Resolve a node id for the hardlinked file via the per-node candidate lookup,
        // which surfaces the full candidate set (hardlinks included).
        let candidates = db
            .duplicate_candidates_filtered(Some(0), None, None, None, true, 50)
            .unwrap();
        let node_a = candidates
            .groups
            .iter()
            .flat_map(|group| group.members.iter())
            .find(|member| member.path == "hard-a.dat")
            .or_else(|| {
                candidates
                    .groups
                    .iter()
                    .flat_map(|group| group.members.iter())
                    .find(|member| member.path == "copy-c.dat")
            })
            .map(|member| member.node_id);
        // If a candidate group surfaced, confirm from one of its members; otherwise
        // fall back to confirming from copy-c which always has distinct identity.
        let target = node_a.unwrap();

        let confirmation = db.confirm_duplicate_group(target).unwrap();
        assert!(!confirmation.partial);
        assert_eq!(confirmation.confirmed_groups.len(), 1);
        let confirmed = &confirmation.confirmed_groups[0];
        assert_eq!(confirmed.confidence, "High");
        // Two distinct physical files (the hardlink pair collapses to one) → keep one,
        // reclaim the other → exactly one 4096-byte footprint.
        assert_eq!(confirmed.reclaimable_bytes, 4096);
        assert_eq!(confirmation.reclaimable_bytes, 4096);
    }

    #[test]
    fn duplicate_candidates_dedupe_overlapping_roots_by_physical_path() {
        let body = "same-physical-file\n".repeat(96);
        let attributes = Some(serde_json::json!({ "body": body }).to_string());
        let rows = vec![
            DuplicateCandidateRow {
                node_id: 1,
                project_id: 10,
                project_name: "Outer".to_string(),
                path: "AI/antigravity/copy-a.dat".to_string(),
                display_name: "copy-a.dat".to_string(),
                absolute_path: "fixture://C:/AI/antigravity/copy-a.dat".to_string(),
                size_bytes: body.len() as u64,
                physical_bytes: Some(4096),
                footprint_partial: false,
                identity_key: Some("vol:copy-a".to_string()),
                attributes: attributes.clone(),
            },
            DuplicateCandidateRow {
                node_id: 2,
                project_id: 11,
                project_name: "Nested".to_string(),
                path: "copy-a.dat".to_string(),
                display_name: "copy-a.dat".to_string(),
                absolute_path: "fixture://C:/AI/antigravity/copy-a.dat".to_string(),
                size_bytes: body.len() as u64,
                physical_bytes: Some(4096),
                footprint_partial: false,
                identity_key: Some("vol:copy-a".to_string()),
                attributes: attributes.clone(),
            },
            DuplicateCandidateRow {
                node_id: 3,
                project_id: 10,
                project_name: "Outer".to_string(),
                path: "AI/other/copy-b.dat".to_string(),
                display_name: "copy-b.dat".to_string(),
                absolute_path: "fixture://C:/AI/other/copy-b.dat".to_string(),
                size_bytes: body.len() as u64,
                physical_bytes: Some(4096),
                footprint_partial: false,
                identity_key: Some("vol:copy-b".to_string()),
                attributes,
            },
        ];

        let duplicates = duplicate_groups_from_rows(rows, 10, "all", None).unwrap();

        assert_eq!(duplicates.groups.len(), 1);
        assert_eq!(duplicates.groups[0].member_count, 2);
        let physical_paths = duplicates.groups[0]
            .members
            .iter()
            .map(|member| member.path.as_str())
            .collect::<HashSet<_>>();
        assert!(physical_paths.contains("AI/antigravity/copy-a.dat"));
        assert!(physical_paths.contains("AI/other/copy-b.dat"));
    }

    #[test]
    fn duplicate_candidates_honor_explicit_filters() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let body = "filtered-duplicate\n".repeat(96);
        fs::write(dir.path().join("filtered-a.dat"), &body).unwrap();
        fs::write(dir.path().join("filtered-b.dat"), &body).unwrap();
        let size = body.len() as u64;
        let files = [
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("filter-a"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("filtered-a.dat"), "filtered-a.dat")
            },
            ScannedFile {
                identity: Some(file_identity(
                    size,
                    Some(4096),
                    Some("vol"),
                    Some("filter-b"),
                )),
                ..scanned_fixture_for_path(&dir.path().join("filtered-b.dat"), "filtered-b.dat")
            },
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let filtered = db
            .duplicate_candidates_filtered(Some(10 * 1024 * 1024), None, None, None, true, 10)
            .unwrap();
        assert!(filtered.groups.is_empty());
        let data = db
            .duplicate_candidates_filtered(Some(1), None, Some("data"), None, true, 10)
            .unwrap();
        assert!(data.groups.iter().any(|group| group
            .members
            .iter()
            .any(|member| member.path == "filtered-a.dat")));
        let unlimited = db
            .duplicate_candidates_filtered(Some(1), None, Some("data"), None, true, 0)
            .unwrap();
        assert!(unlimited.total >= unlimited.groups.len() as i64);
        let current_file = data
            .groups
            .iter()
            .flat_map(|group| group.members.iter())
            .find(|member| member.path == "filtered-a.dat")
            .unwrap();
        let current_file_matches = db
            .duplicate_candidates_filtered(
                Some(1),
                None,
                Some("data"),
                Some(current_file.node_id),
                true,
                10,
            )
            .unwrap();
        assert_eq!(current_file_matches.total, 1);
        assert!(current_file_matches.groups[0]
            .members
            .iter()
            .any(|member| member.node_id == current_file.node_id));
    }

    #[test]
    fn quick_open_finds_items_beyond_first_five_hundred() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let files = (0..620)
            .map(|index| ScannedFile {
                absolute_path: dir
                    .path()
                    .join(format!("file-{index:03}-needle.txt"))
                    .to_string_lossy()
                    .to_string(),
                relative_path: format!("file-{index:03}-needle.txt"),
                display_path: format!("file-{index:03}-needle.txt"),
                display_name: format!("file-{index:03}-needle.txt"),
                item_kind: "file".to_string(),
                is_markdown: false,
                is_context: false,
                is_sensitive: false,
                protected_level: None,
                child_count: 0,
                fully_scanned: true,
                collapse_default: false,
                scan_error: None,
                identity: None,
                body: None,
            })
            .collect::<Vec<_>>();
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let results = db.quick_open("file-619-needle", 5).unwrap();
        assert!(results
            .iter()
            .any(|result| result.path == "file-619-needle.txt"));
    }

    #[test]
    fn fts_uses_indexed_body_without_internal_json_false_positives() {
        let db = Db::open_memory().unwrap();

        assert!(db.search_documents("scan_metadata", 10).unwrap().is_empty());
        assert!(db
            .search_documents("overview", 10)
            .unwrap()
            .iter()
            .any(|hit| hit.path == "docs/overview.md"));
        let filtered = db
            .search_documents_filtered(DocumentSearchOptions {
                query: "overview",
                project_id: None,
                indexed_kind: Some("context"),
                path_filter: None,
                name_filter: None,
                include_fixture_projects: true,
                limit: 1,
            })
            .unwrap();
        assert_eq!(filtered.hits.len(), 1);
        assert!(filtered.duration_ms < 60_000);

        let path_filtered = db
            .search_documents_filtered(DocumentSearchOptions {
                query: "overview",
                project_id: None,
                indexed_kind: Some("context"),
                path_filter: Some("docs"),
                name_filter: None,
                include_fixture_projects: true,
                limit: 0,
            })
            .unwrap();
        assert!(path_filtered
            .hits
            .iter()
            .any(|hit| hit.path == "docs/overview.md"));
    }

    #[test]
    fn document_search_excludes_fixture_projects_before_applying_the_limit() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let readme_path = dir.path().join("README.md");
        fs::write(&readme_path, "overview from a real local project").unwrap();
        let real_file = ScannedFile {
            is_markdown: true,
            is_context: true,
            body: Some("overview from a real local project".to_string()),
            ..scanned_fixture_for_path(&readme_path, "README.md")
        };
        db.load_scanned_root(&dir.path().to_string_lossy(), &[real_file], None)
            .unwrap();
        let real_project_id = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .expect("real scanned project")
            .id;

        let with_fixtures = db
            .search_documents_filtered(DocumentSearchOptions {
                query: "overview",
                project_id: None,
                indexed_kind: None,
                path_filter: None,
                name_filter: None,
                include_fixture_projects: true,
                limit: 50,
            })
            .unwrap();
        assert!(with_fixtures
            .hits
            .iter()
            .any(|hit| hit.project_id != real_project_id));

        let real_only = db
            .search_documents_filtered(DocumentSearchOptions {
                query: "overview",
                project_id: None,
                indexed_kind: None,
                path_filter: None,
                name_filter: None,
                include_fixture_projects: false,
                limit: 1,
            })
            .unwrap();
        assert_eq!(real_only.hits.len(), 1);
        assert_eq!(real_only.hits[0].project_id, real_project_id);
        assert!(!real_only.truncated);
    }

    #[test]
    fn fts_does_not_index_heavy_or_sensitive_bodies() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        fs::create_dir_all(dir.path().join(".pytest_cache")).unwrap();
        fs::write(dir.path().join("README.md"), "uniquecontextterm").unwrap();
        fs::write(
            dir.path().join("node_modules/pkg/readme.md"),
            "uniqueheavyterm",
        )
        .unwrap();
        fs::write(
            dir.path().join(".pytest_cache/README.md"),
            "uniquegeneratedcacheterm",
        )
        .unwrap();
        fs::write(dir.path().join(".env"), "uniquesensitive").unwrap();
        let outcome = hangar_fs::scan_markdown_context_root(dir.path()).unwrap();
        db.load_scanned_root(&dir.path().to_string_lossy(), &outcome.files, None)
            .unwrap();

        assert!(!db
            .search_documents("uniquecontextterm", 10)
            .unwrap()
            .is_empty());
        assert!(db
            .search_documents("uniqueheavyterm", 10)
            .unwrap()
            .is_empty());
        assert!(db
            .search_documents("uniquegeneratedcacheterm", 10)
            .unwrap()
            .is_empty());
        assert!(db
            .search_documents("uniquesensitive", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn context_reconciliation_removes_legacy_cache_documents() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join(".pytest_cache");
        fs::create_dir_all(&cache_dir).unwrap();
        let cache_readme = cache_dir.join("README.md");
        fs::write(&cache_readme, "uniquelegacycacheterm").unwrap();
        let legacy_file = ScannedFile {
            display_name: "README.md".to_string(),
            is_markdown: true,
            is_context: true,
            body: Some("uniquelegacycacheterm".to_string()),
            ..scanned_fixture_for_path(&cache_readme, ".pytest_cache/README.md")
        };
        db.load_scanned_root(&dir.path().to_string_lossy(), &[legacy_file], None)
            .unwrap();

        assert!(!db
            .search_documents("uniquelegacycacheterm", 10)
            .unwrap()
            .is_empty());
        db.with_writer(|conn| reconcile_stale_context_classification(conn))
            .unwrap();
        assert!(db
            .search_documents("uniquelegacycacheterm", 10)
            .unwrap()
            .is_empty());
        db.with_conn(|conn| {
            let is_context: i64 = conn.query_row(
                "SELECT is_context FROM nav_item WHERE path = '.pytest_cache/README.md'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(is_context, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn preview_truncates_large_text_and_reports_binary_metadata() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let big_path = dir.path().join("big.log");
        fs::write(&big_path, "a".repeat((PREVIEW_LIMIT_BYTES as usize) + 16)).unwrap();
        let binary_path = dir.path().join("image.bin");
        fs::write(&binary_path, [0, 159, 146, 150]).unwrap();
        let files = [
            scanned_fixture_for_path(&big_path, "big.log"),
            scanned_fixture_for_path(&binary_path, "image.bin"),
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let big = db
            .quick_open("big.log", 10)
            .unwrap()
            .into_iter()
            .find(|result| result.path == "big.log")
            .unwrap();
        let preview = db
            .file_preview(big.node_id, PreviewMode::Source, true)
            .unwrap();
        assert_eq!(preview.state, PreviewState::Ready);
        assert!(preview.truncated);
        assert_eq!(preview.source.unwrap().len(), PREVIEW_LIMIT_BYTES as usize);

        let binary = db
            .quick_open("image.bin", 10)
            .unwrap()
            .into_iter()
            .find(|result| result.path == "image.bin")
            .unwrap();
        let preview = db
            .file_preview(binary.node_id, PreviewMode::Source, true)
            .unwrap();
        assert_eq!(preview.state, PreviewState::Unsupported);
        assert_eq!(preview.file_kind, FileKind::Binary);
    }

    #[test]
    fn resolves_local_markdown_links_without_escaping_root() {
        let db = Db::open_memory().unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .unwrap();
        let readme = db
            .quick_open("README.md", 10)
            .unwrap()
            .into_iter()
            .find(|result| result.project_id == project.id && result.path == "README.md")
            .unwrap();

        let overview = db
            .resolve_local_link(project.id, readme.node_id, "docs/overview.md")
            .unwrap();
        assert!(overview.is_some());
        let escaped = db
            .resolve_local_link(project.id, readme.node_id, "../../../secret.md")
            .unwrap();
        assert!(escaped.is_none());
    }

    #[test]
    fn unregister_root_removes_local_inventory_only() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("README.md");
        fs::write(&file_path, "# Temporary").unwrap();
        let root = db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&file_path, "README.md")];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        assert!(db.quick_open("Temporary", 10).unwrap().is_empty());
        assert!(db.quick_open("README.md", 50).unwrap().len() > 1);

        db.roots_unregister(root.id).unwrap();

        assert!(file_path.exists());
        assert!(db
            .projects_list()
            .unwrap()
            .iter()
            .all(|project| project.path != dir.path().to_string_lossy()));
    }

    #[test]
    fn unregister_with_scan_cache_rows_does_not_hit_fk_violation() {
        // Real scans create scan_cache rows that REFERENCE node(id); the identity:None
        // fixtures do not, which is why the FK-ordering bug slipped past the test above. Here
        // we insert a referencing scan_cache row and confirm unregister/discard still succeeds
        // (before the fix, deleting the node rows first aborted with a FK constraint failure).
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("README.md");
        fs::write(&file_path, "# Temporary").unwrap();
        let root = db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&file_path, "README.md")];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        db.with_conn(|conn| {
            let node_id: i64 = conn.query_row(
                "SELECT node_id FROM nav_item WHERE node_id IS NOT NULL LIMIT 1",
                [],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO scan_cache(path, mtime, size, node_id) VALUES (?1, '2026-01-01', 11, ?2)",
                params![file_path.to_string_lossy(), node_id],
            )?;
            Ok(())
        })
        .unwrap();

        // Must not abort with a FOREIGN KEY constraint failure.
        db.roots_unregister(root.id).unwrap();

        let remaining: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM scan_cache", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(remaining, 0, "scan_cache rows must be cleaned up");
        assert!(file_path.exists());
        assert!(db
            .projects_list()
            .unwrap()
            .iter()
            .all(|project| project.path != dir.path().to_string_lossy()));
    }

    #[test]
    fn adhoc_investigation_root_is_hidden_from_listings_and_discardable() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        // Investigating a folder indexes it as a project internally but flags it ad-hoc.
        let root = db.roots_add_adhoc(&path).unwrap();
        assert!(db.root_is_adhoc(root.id).unwrap());

        // It is hidden from the user's scan-roots list...
        assert!(db
            .roots_list()
            .unwrap()
            .iter()
            .all(|entry| entry.id != root.id));
        // ...and from the projects list (so it is never "registered").
        let listed = db.projects_list().unwrap();
        assert!(listed
            .iter()
            .all(|project| !project.path.to_lowercase().contains(&path.to_lowercase())));

        // Discarding the investigation removes the root entirely.
        db.roots_unregister(root.id).unwrap();
        assert!(!db.root_is_adhoc(root.id).unwrap());
        assert!(db
            .roots_list()
            .unwrap()
            .iter()
            .all(|entry| entry.id != root.id));
    }

    #[test]
    fn reset_local_inventory_clears_real_projects_keeps_demos() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("README.md");
        fs::write(&file_path, "# Real").unwrap();
        db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&file_path, "README.md")];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        // A demo/fixture project (source = "fixture") must survive the reset.
        let now = now();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO node(id, kind, path, name, attributes, first_seen_at, last_seen_at, present)
                 VALUES(950001, 'project', 'C:\\demo', 'demo', '{\"source\":\"fixture\"}', ?1, ?1, 1)",
                params![now],
            )?;
            Ok(())
        })
        .unwrap();

        let removed = db.reset_local_inventory().unwrap();
        assert_eq!(removed, 1);

        // The real file on disk is never touched.
        assert!(file_path.exists());
        // Every scan root is gone.
        assert!(db.roots_list().unwrap().is_empty());
        // The scanned project is gone; the demo project remains.
        let projects = db.projects_list().unwrap();
        assert!(projects
            .iter()
            .all(|project| project.path != dir.path().to_string_lossy()));
        assert!(projects.iter().any(|project| project.path == "C:\\demo"));
    }

    #[test]
    fn reset_local_inventory_schedules_wipe_on_file_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("inventory.sqlite3");
        let db = Db::open(&db_path).unwrap();

        // Add a real project on top of the built-in demos that Db::open seeds,
        // plus several megabytes of full-text body so there is genuine disk to
        // reclaim — the shape a broad scan produces.
        let project_dir = dir.path().join("realproj");
        fs::create_dir_all(&project_dir).unwrap();
        let file_path = project_dir.join("README.md");
        fs::write(&file_path, "# Real").unwrap();
        db.roots_add(&project_dir.to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&file_path, "README.md")];
        db.load_scanned_root(&project_dir.to_string_lossy(), &files, None)
            .unwrap();
        let big_body = "lorem ipsum dolor sit amet consectetur ".repeat(160_000);
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO document_fts(node_id, project_id, path, title, headings, body)
                 VALUES(0, 0, 'bulk', 'bulk', '', ?1)",
                params![big_body],
            )?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
        .unwrap();
        let size_before = fs::metadata(&db_path).unwrap().len();
        assert!(size_before > 3_000_000, "expected an inflated database");
        assert!(db
            .projects_list()
            .unwrap()
            .iter()
            .any(|project| project.path == project_dir.to_string_lossy()));

        // A file-backed reset schedules the wipe (a sentinel) rather than deleting
        // the large encrypted index in place — clearing that index would crawl,
        // whereas deleting the file at startup is O(1).
        let removed = db.reset_local_inventory().unwrap();
        assert_eq!(removed, 1);
        assert!(reset_sentinel_path(&db_path).exists());

        // Simulate the next startup: close the connection, apply the pending wipe,
        // then reopen — the file is deleted and recreated fresh, reclaiming the
        // disk, and the demos come back while the real project is gone.
        drop(db);
        assert!(wipe_pending_reset(&db_path));
        assert!(!reset_sentinel_path(&db_path).exists());

        let db2 = Db::open(&db_path).unwrap();
        let size_after = fs::metadata(&db_path).unwrap().len();
        assert!(
            size_after.saturating_mul(3) < size_before,
            "the wipe must reclaim the disk: {size_before} -> {size_after}"
        );
        let projects = db2.projects_list().unwrap();
        assert!(projects
            .iter()
            .all(|project| project.path != project_dir.to_string_lossy()));
        assert!(!projects.is_empty(), "demos should be restored after wipe");
        assert!(db2.roots_list().unwrap().is_empty());
        // The real file on disk is never touched.
        assert!(file_path.exists());
    }

    #[test]
    fn project_unregister_removes_orphan_without_root() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("README.md");
        fs::write(&file_path, "# Orphan").unwrap();
        let root = db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&file_path, "README.md")];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        // Simulate the orphan state: the scan_root is gone but the project remains.
        db.with_writer(|conn| {
            conn.execute("DELETE FROM scan_root WHERE id = ?1", params![root.id])?;
            Ok(())
        })
        .unwrap();

        let project_id = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .map(|project| project.id)
            .expect("orphan project should still be listed");

        db.project_unregister(project_id).unwrap();

        assert!(file_path.exists());
        assert!(db
            .projects_list()
            .unwrap()
            .iter()
            .all(|project| project.path != dir.path().to_string_lossy()));
    }

    #[test]
    fn project_unregister_preserves_nodes_used_by_other_projects() {
        let db = Db::open_memory().unwrap();
        let now = now();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO node(id, kind, path, name, attributes, first_seen_at, last_seen_at, present)
                 VALUES(900010, 'project', 'C:\\alpha', 'alpha', '{}', ?1, ?1, 1)",
                params![now],
            )?;
            conn.execute(
                "INSERT INTO node(id, kind, path, name, attributes, first_seen_at, last_seen_at, present)
                 VALUES(900020, 'project', 'C:\\beta', 'beta', '{}', ?1, ?1, 1)",
                params![now],
            )?;
            conn.execute(
                "INSERT INTO node(id, kind, path, name, attributes, first_seen_at, last_seen_at, present)
                 VALUES(900030, 'file', 'C:\\shared\\README.md', 'README.md', '{}', ?1, ?1, 1)",
                params![now],
            )?;
            conn.execute(
                "INSERT INTO nav_item(project_id, node_id, path, display_path, display_name, item_kind, priority, sort_key)
                 VALUES(900010, 900030, 'shared/README.md', 'shared/README.md', 'README.md', 'file', 0, 'README.md')",
                [],
            )?;
            conn.execute(
                "INSERT INTO nav_item(project_id, node_id, path, display_path, display_name, item_kind, priority, sort_key)
                 VALUES(900020, 900030, 'shared/README.md', 'shared/README.md', 'README.md', 'file', 0, 'README.md')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        db.project_unregister(900010).unwrap();

        db.with_read_conn(|conn| {
            let shared_node_exists: i64 =
                conn.query_row("SELECT COUNT(*) FROM node WHERE id = 900030", [], |row| {
                    row.get(0)
                })?;
            let beta_nav_exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nav_item WHERE project_id = 900020 AND node_id = 900030",
                [],
                |row| row.get(0),
            )?;
            let alpha_nav_exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nav_item WHERE project_id = 900010",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(shared_node_exists, 1);
            assert_eq!(beta_nav_exists, 1);
            assert_eq!(alpha_nav_exists, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn nav_item_node_index_exists_for_shared_node_cleanup() {
        let db = Db::open_memory().unwrap();
        db.with_read_conn(|conn| {
            let mut stmt = conn.prepare("PRAGMA index_list(nav_item)")?;
            let indexes = collect_rows(stmt.query_map([], |row| row.get::<_, String>(1))?)?;
            assert!(indexes.iter().any(|index| index == "idx_nav_node"));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn unregister_root_handles_nested_nav_items() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let docs_dir = dir.path().join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        let file_path = docs_dir.join("README.md");
        fs::write(&file_path, "# Nested").unwrap();
        let root = db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [
            ScannedFile {
                absolute_path: docs_dir.to_string_lossy().to_string(),
                relative_path: "docs".to_string(),
                display_path: "docs".to_string(),
                display_name: "docs".to_string(),
                item_kind: "directory".to_string(),
                is_markdown: false,
                is_context: false,
                is_sensitive: false,
                protected_level: None,
                child_count: 1,
                fully_scanned: true,
                collapse_default: false,
                scan_error: None,
                identity: None,
                body: None,
            },
            scanned_fixture_for_path(&file_path, "docs/README.md"),
        ];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        db.roots_unregister(root.id).unwrap();

        assert!(file_path.exists());
        db.with_read_conn(|conn| {
            let node_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM node WHERE path = ?1 OR path = ?2",
                params![
                    docs_dir.to_string_lossy().to_string(),
                    file_path.to_string_lossy().to_string()
                ],
                |row| row.get(0),
            )?;
            assert_eq!(node_count, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn root_finalize_honors_cancel_before_heavy_steps() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("hangar.sqlite3");
        let db = Db::open(&db_path).unwrap();
        let root_dir = dir.path().join("project");
        fs::create_dir_all(&root_dir).unwrap();
        let file_path = root_dir.join("README.md");
        fs::write(&file_path, "# Temporary").unwrap();
        db.roots_add(&root_dir.to_string_lossy()).unwrap();
        let mut writer = db.open_write_session().unwrap();
        let project_id = writer.begin_root_scan(&root_dir.to_string_lossy()).unwrap();
        writer
            .persist_batch(
                project_id,
                &[scanned_fixture_for_path(&file_path, "README.md")],
            )
            .unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let err = writer
            .finish_root_scan_interruptible_with_progress(
                project_id,
                &root_dir.to_string_lossy(),
                None,
                true,
                Arc::clone(&cancel),
                |message| {
                    if message.contains("updating folder child counts") {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
            )
            .unwrap_err();

        assert!(err.to_string().contains("Cancelled"));
        let root = db
            .roots_list()
            .unwrap()
            .into_iter()
            .find(|root| root.path == root_dir.to_string_lossy())
            .unwrap();
        assert!(root.last_scanned_at.is_none());
    }

    #[test]
    fn roots_add_creates_outdated_project_placeholder() {
        let db = Db::open_memory().unwrap();
        let root = db.roots_add("fixture://new-local-root").unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == "fixture://new-local-root")
            .unwrap();

        assert_eq!(project.name, "new-local-root");
        assert_eq!(project.context_count, 0);
        assert_eq!(project.scan_state, "outdated");
        assert_eq!(project.scan_root_id, Some(root.id));
    }

    #[test]
    fn projects_list_lite_preserves_outdated_scan_state() {
        let db = Db::open_memory().unwrap();
        let root = db.roots_add("fixture://new-lite-root").unwrap();

        let project = db
            .projects_list_lite()
            .unwrap()
            .into_iter()
            .find(|project| project.path == "fixture://new-lite-root")
            .unwrap();

        assert_eq!(project.context_count, 0);
        assert_eq!(project.scan_state, "outdated");
        assert_eq!(project.scan_root_id, Some(root.id));

        let dir = tempdir().unwrap();
        let readme_path = dir.path().join("README.md");
        fs::write(&readme_path, "# Context").unwrap();
        let files = [ScannedFile {
            is_markdown: true,
            is_context: true,
            ..scanned_fixture_for_path(&readme_path, "README.md")
        }];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let scanned_root_path = dir.path().to_string_lossy().to_string();
        let scanned_project = db
            .projects_list_lite()
            .unwrap()
            .into_iter()
            .find(|project| project.path == scanned_root_path)
            .unwrap();

        assert_eq!(scanned_project.context_count, 1);
        assert_eq!(scanned_project.scan_state, "scanned");
    }

    #[test]
    fn scanned_root_without_nav_items_still_needs_scan() {
        let db = Db::open_memory().unwrap();
        let root = db.roots_add("fixture://empty-inventory-root").unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE scan_root SET last_scanned_at = ?2 WHERE id = ?1",
                params![root.id, now()],
            )?;
            Ok(())
        })
        .unwrap();

        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == "fixture://empty-inventory-root")
            .unwrap();

        assert_eq!(project.scan_state, "outdated");
        assert_eq!(project.scan_root_id, Some(root.id));
    }

    #[test]
    fn incomplete_scanned_root_with_inventory_still_needs_scan() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let readme_path = dir.path().join("README.md");
        fs::write(&readme_path, "# Partial").unwrap();
        let root = db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&readme_path, "README.md")];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let scanned_project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();
        assert_eq!(scanned_project.scan_state, "scanned");

        db.with_writer(|conn| {
            conn.execute(
                "UPDATE scan_root SET last_scanned_at = NULL WHERE id = ?1",
                params![root.id],
            )?;
            Ok(())
        })
        .unwrap();

        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();

        assert_eq!(project.scan_state, "outdated");
        assert_eq!(project.scan_root_id, Some(root.id));
    }

    #[test]
    fn scan_estimates_use_previous_inventory_without_rescanning_disk() {
        let db = Db::open_memory().unwrap();
        let dir = tempdir().unwrap();
        let docs_dir = dir.path().join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        let file_path = docs_dir.join("README.md");
        fs::write(&file_path, "# Estimate").unwrap();

        let root = db.roots_add(&dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&file_path, "docs/README.md")];
        db.load_scanned_root(&dir.path().to_string_lossy(), &files, None)
            .unwrap();

        let root_estimate = db.scan_estimate_for_roots(&[root.id]).unwrap().unwrap();
        assert!(root_estimate >= 2);

        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == dir.path().to_string_lossy())
            .unwrap();
        let docs_nav = db
            .project_nav_tree(project.id)
            .unwrap()
            .into_iter()
            .find(|item| item.path == "docs")
            .unwrap();
        let subtree_estimate = db.scan_estimate_for_subtree(docs_nav.id).unwrap().unwrap();
        assert!(subtree_estimate >= 2);
    }

    #[test]
    fn complete_scan_estimate_requires_every_root_to_have_inventory() {
        let db = Db::open_memory().unwrap();
        let scanned_dir = tempdir().unwrap();
        let readme_path = scanned_dir.path().join("README.md");
        fs::write(&readme_path, "# Estimate").unwrap();
        let scanned_root = db.roots_add(&scanned_dir.path().to_string_lossy()).unwrap();
        let files = [scanned_fixture_for_path(&readme_path, "README.md")];
        db.load_scanned_root(&scanned_dir.path().to_string_lossy(), &files, None)
            .unwrap();
        let new_root = db.roots_add("fixture://not-yet-scanned").unwrap();

        assert!(db
            .complete_scan_estimate_for_roots(&[scanned_root.id])
            .unwrap()
            .is_some());
        assert!(db
            .complete_scan_estimate_for_roots(&[scanned_root.id, new_root.id])
            .unwrap()
            .is_none());
    }

    #[test]
    fn subtree_rescan_preserves_pins_and_recents_by_upserting_nodes() {
        let db_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let db = Db::open(db_dir.path().join("hangar.sqlite")).unwrap();
        let docs_dir = project_dir.path().join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        let old_file = docs_dir.join("old.md");
        fs::write(&old_file, "# Old").unwrap();
        db.roots_add(&project_dir.path().to_string_lossy()).unwrap();

        let initial = hangar_fs::scan_markdown_context_root(project_dir.path()).unwrap();
        db.load_scanned_root(&project_dir.path().to_string_lossy(), &initial.files, None)
            .unwrap();
        let old_hit = db
            .quick_open("old.md", 10)
            .unwrap()
            .into_iter()
            .find(|result| result.path == "docs/old.md")
            .unwrap();
        db.pin_item(old_hit.node_id, "file").unwrap();
        db.file_preview(old_hit.node_id, PreviewMode::Source, true)
            .unwrap();
        // For a file-backed DB, file_preview records the "recent" on a fire-and-forget background
        // thread (queue_recent_for_preview), so the write is not guaranteed to have landed when this
        // function returns. Wait for it before the rescan; otherwise the recents assertion below
        // races that async write and fails ~1/3 of runs. This tests rescan PRESERVATION, not the
        // async record itself.
        for _ in 0..200 {
            if db
                .recent_items_list(20)
                .unwrap()
                .iter()
                .any(|item| item.node_id == old_hit.node_id)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            db.recent_items_list(20)
                .unwrap()
                .iter()
                .any(|item| item.node_id == old_hit.node_id),
            "the async recent write never landed before the rescan"
        );
        fs::write(docs_dir.join("new.md"), "# New").unwrap();

        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == project_dir.path().to_string_lossy())
            .unwrap();
        let docs_nav = db
            .project_nav_tree(project.id)
            .unwrap()
            .into_iter()
            .find(|item| item.path == "docs")
            .unwrap();
        let target = db.subtree_scan_target(docs_nav.id).unwrap();
        let mut writer = db.open_write_session().unwrap();
        writer
            .begin_subtree_scan(target.project_id, target.nav_id)
            .unwrap();
        hangar_fs::scan_inventory_stream(
            Path::new(&target.root_path),
            Some(&target.relative_path),
            hangar_fs::ScanLimits::resume_subtree(),
            || false,
            |_, _, _| {},
            |batch| {
                writer
                    .persist_batch(target.project_id, &batch)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            },
        )
        .unwrap();
        writer
            .finish_subtree_scan(target.project_id, target.nav_id, None)
            .unwrap();

        assert!(db
            .pinned_items_list()
            .unwrap()
            .iter()
            .any(|item| item.node_id == old_hit.node_id));
        assert!(db
            .recent_items_list(20)
            .unwrap()
            .iter()
            .any(|item| item.node_id == old_hit.node_id));
        assert!(db
            .quick_open("new.md", 10)
            .unwrap()
            .iter()
            .any(|result| result.path == "docs/new.md"));
    }

    #[test]
    fn comment_add_list_edit_delete_with_author() {
        let db = Db::open_memory().unwrap();
        let root_path = r"C:\tmp\proj";
        let files = [ScannedFile {
            absolute_path: format!(r"{root_path}\a.txt"),
            relative_path: "a.txt".to_string(),
            display_path: "a.txt".to_string(),
            display_name: "a.txt".to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }];
        db.load_scanned_root(root_path, &files, None).unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|p| p.path == root_path)
            .unwrap();
        let node_id = project.id;

        // Human comment on the project node.
        let c = db
            .comment_add(node_id, "  first note  ", "user", "user")
            .unwrap();
        assert_eq!(c.body, "first note"); // trimmed
        assert_eq!(c.author, "user");
        assert_eq!(c.source, "user");
        assert_eq!(c.project_id, Some(node_id)); // a project node owns itself
        assert_eq!(db.comments_count_for_node(node_id).unwrap(), 1);

        // Agent-authored write friendliness (a later phase needs this). An AI write
        // requires the global write-mode toggle; the local user is never subject to it.
        assert!(db
            .comment_add(node_id, "blocked", "claude-code", "agent")
            .is_err());
        db.set_comment_write_enabled(true).unwrap();
        let a = db
            .comment_add(node_id, "agent note", "claude-code", "agent")
            .unwrap();
        assert_eq!(a.author, "claude-code");
        assert_eq!(a.source, "agent");
        assert_eq!(db.comments_for_node(node_id).unwrap().len(), 2);

        // Edit bumps updated_at and changes the body.
        let edited = db.comment_edit(c.id, "edited note", "user").unwrap();
        assert_eq!(edited.body, "edited note");
        assert_ne!(edited.updated_at, edited.created_at);

        // Soft delete excludes the row from list + count.
        db.comment_delete(c.id, "user").unwrap();
        assert_eq!(db.comments_count_for_node(node_id).unwrap(), 1);
        assert!(db
            .comments_for_node(node_id)
            .unwrap()
            .iter()
            .all(|x| x.id != c.id));

        // Empty / whitespace-only bodies are rejected.
        assert!(db.comment_add(node_id, "   ", "user", "user").is_err());
        assert!(db.comment_edit(a.id, "", "user").is_err());
    }

    #[test]
    fn ai_provider_config_defaults_off_and_round_trips() {
        let db = Db::open_memory().unwrap();
        // Fresh DB defaults to mode "off" — nothing leaves the machine until configured.
        let initial = db.ai_provider_config().unwrap();
        assert_eq!(initial.mode, "off");
        assert_eq!(initial.base_url, "");
        assert_eq!(initial.model, "");
        assert_eq!(initial.format, "chat_completions");

        // Persisted config round-trips (the API key is never stored here — only in the keychain).
        let config = AiProviderConfig {
            mode: "local".to_string(),
            base_url: "http://localhost:11434/v1".to_string(),
            model: "qwen2.5-coder".to_string(),
            format: "chat_completions".to_string(),
        };
        db.set_ai_provider_config(&config).unwrap();
        assert_eq!(db.ai_provider_config().unwrap(), config);
    }

    #[test]
    fn personal_glossary_is_opt_in_and_counts_canonical_terms() {
        let db = Db::open_memory().unwrap();
        assert!(!db.ai_glossary_enabled_value().unwrap());
        assert!(db
            .ai_glossary_record("function", "A named block of behaviour.")
            .is_err());
        db.set_ai_glossary_enabled(true).unwrap();
        let first = db
            .ai_glossary_record("function", "A named block of behaviour.")
            .unwrap();
        assert_eq!(first.count, 1);
        let second = db
            .ai_glossary_record("function", "A named block of behaviour.")
            .unwrap();
        assert_eq!(second.count, 2);
        assert_eq!(db.ai_glossary_entries().unwrap(), vec![second]);
    }

    #[test]
    fn code_annotations_round_trip_with_private_anchor_text() {
        let db = Db::open_memory().unwrap();
        let project_id = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .unwrap()
            .id;
        let node_id = db
            .quick_open("README.md", 20)
            .unwrap()
            .into_iter()
            .find(|item| item.project_id == project_id)
            .unwrap()
            .node_id;
        let created = db
            .code_annotation_add(node_id, "b885c8b", 2, 2, "abc", "Remember this")
            .unwrap();
        assert_eq!(created.anchor_state, "unchecked");
        let stored = db.code_annotations_for_node(node_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].snippet, "abc");
        assert_eq!(stored[0].annotation.note, "Remember this");
        assert!(db.code_annotation_delete(created.id, node_id).unwrap());
        assert!(db.code_annotations_for_node(node_id).unwrap().is_empty());
    }

    #[test]
    fn final_remove_enabled_defaults_off_and_round_trips() {
        let db = Db::open_memory().unwrap();
        assert!(
            !db.final_remove_enabled_value().unwrap(),
            "final removal is OFF by default; users must opt in before irreversible removal is offered"
        );
        db.set_final_remove_enabled(true).unwrap();
        assert!(db.final_remove_enabled_value().unwrap());
        db.set_final_remove_enabled(false).unwrap();
        assert!(!db.final_remove_enabled_value().unwrap());
    }

    #[test]
    fn comments_protect_human_records_from_agents() {
        let db = Db::open_memory().unwrap();
        let root_path = r"C:\tmp\guard";
        let files = [ScannedFile {
            absolute_path: format!(r"{root_path}\a.txt"),
            relative_path: "a.txt".to_string(),
            display_path: "a.txt".to_string(),
            display_name: "a.txt".to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }];
        db.load_scanned_root(root_path, &files, None).unwrap();
        let node_id = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|p| p.path == root_path)
            .unwrap()
            .id;

        let human = db
            .comment_add(node_id, "human note", "user", "user")
            .unwrap();
        // With AI write mode OFF (default), an agent cannot even add a comment, but the
        // human can. Then enable it so the ownership rules below can be exercised.
        assert!(db
            .comment_add(node_id, "blocked", "hermes", "hermes")
            .is_err());
        db.set_comment_write_enabled(true).unwrap();
        // Two distinct agents (e.g. a less-smart local model and another).
        let from_hermes = db
            .comment_add(node_id, "from hermes", "hermes", "hermes")
            .unwrap();
        let from_claude = db
            .comment_add(node_id, "from claude", "claude-code", "claude-code")
            .unwrap();

        // An agent may NEVER edit or delete a HUMAN comment.
        assert!(db.comment_delete(human.id, "hermes").is_err());
        assert!(db.comment_edit(human.id, "hijacked", "hermes").is_err());
        // ...nor ANOTHER agent's comment.
        assert!(db.comment_delete(from_claude.id, "hermes").is_err());
        assert!(db
            .comment_edit(from_claude.id, "tampered", "hermes")
            .is_err());
        // It MAY edit/delete only its OWN comment.
        assert!(db
            .comment_edit(from_hermes.id, "from hermes v2", "hermes")
            .is_ok());
        assert!(db.comment_delete(from_hermes.id, "hermes").is_ok());

        // The human keeps full control: can delete an agent's comment to clean up.
        assert!(db.comment_delete(from_claude.id, "user").is_ok());

        // After every agent attempt, the human note is still intact and unchanged.
        let remaining = db.comments_for_node(node_id).unwrap();
        let human_now = remaining.iter().find(|c| c.id == human.id).unwrap();
        assert_eq!(human_now.body, "human note");
        assert_eq!(human_now.source, "user");
    }

    #[test]
    fn mcp_full_control_toggle_defaults_off_and_round_trips() {
        let db = Db::open_memory().unwrap();
        // The "AI total control" tier is OFF until the user deliberately flips it
        // behind the heavily-signposted, accountability warning.
        assert!(!db.mcp_full_control_enabled_value().unwrap());
        db.set_mcp_full_control_enabled(true).unwrap();
        assert!(db.mcp_full_control_enabled_value().unwrap());
        db.set_mcp_full_control_enabled(false).unwrap();
        assert!(!db.mcp_full_control_enabled_value().unwrap());
    }

    #[test]
    fn agent_request_create_list_and_resolve() {
        let db = Db::open_memory().unwrap();
        let root_path = r"C:\tmp\req";
        let files = [ScannedFile {
            absolute_path: format!(r"{root_path}\a.txt"),
            relative_path: "a.txt".to_string(),
            display_path: "a.txt".to_string(),
            display_name: "a.txt".to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }];
        db.load_scanned_root(root_path, &files, None).unwrap();
        let node_id = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|p| p.path == root_path)
            .unwrap()
            .id;
        let human = db
            .comment_add(node_id, "human note", "user", "user")
            .unwrap();

        // An agent files a pending request to change a HUMAN comment — it does not
        // and cannot execute it; a human must resolve it.
        let created = db
            .agent_request_create(&NewAgentRequest {
                agent_id: Some(7),
                agent_name: "hermes".to_string(),
                kind: "comment_edit".to_string(),
                target_comment_id: Some(human.id),
                proposed_body: Some("proposed text".to_string()),
                target_kind: Some("comment".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(created.status, "pending");

        let pending = db.agent_requests_pending().unwrap();
        assert_eq!(pending.len(), 1);
        let request = &pending[0];
        assert_eq!(request.agent_name, "hermes");
        assert_eq!(request.kind, "comment_edit");
        assert_eq!(request.proposed_body.as_deref(), Some("proposed text"));
        assert_eq!(request.target_kind.as_deref(), Some("comment"));
        assert!(!request.cross_scope);
        // Enriched with the target comment's present state for the reviewer.
        assert_eq!(request.current_body.as_deref(), Some("human note"));
        assert_eq!(request.current_source.as_deref(), Some("user"));

        // Resolving transitions once; a second resolve is a no-op.
        assert!(db.agent_request_set_status(request.id, "approved").unwrap());
        assert!(!db.agent_request_set_status(request.id, "rejected").unwrap());
        assert!(db.agent_requests_pending().unwrap().is_empty());
        assert_eq!(
            db.agent_request_get(request.id).unwrap().unwrap().status,
            "approved"
        );
    }

    #[test]
    fn agent_requests_for_agent_is_scoped_to_that_agent_and_spans_statuses() {
        let db = Db::open_memory().unwrap();
        // Two agents each file a request; a third agent has none. list_my_requests must
        // return ONLY the calling agent's rows, across every status.
        let mine_pending = db
            .agent_request_create(&NewAgentRequest {
                agent_id: Some(1),
                agent_name: "mine".to_string(),
                kind: "read_body".to_string(),
                target_kind: Some("node".to_string()),
                target_id: Some(10),
                ..Default::default()
            })
            .unwrap();
        let mine_resolved = db
            .agent_request_create(&NewAgentRequest {
                agent_id: Some(1),
                agent_name: "mine".to_string(),
                kind: "backup_protected".to_string(),
                target_kind: Some("node".to_string()),
                target_id: Some(11),
                ..Default::default()
            })
            .unwrap();
        db.agent_request_set_status(mine_resolved.id, "rejected")
            .unwrap();
        let _theirs = db
            .agent_request_create(&NewAgentRequest {
                agent_id: Some(2),
                agent_name: "theirs".to_string(),
                kind: "read_body".to_string(),
                target_kind: Some("node".to_string()),
                target_id: Some(20),
                ..Default::default()
            })
            .unwrap();

        let mine = db.agent_requests_for_agent(1).unwrap();
        assert_eq!(mine.len(), 2, "only agent 1's two requests");
        assert!(mine.iter().all(|r| r.agent_name == "mine"));
        // Both statuses are present (pending AND the resolved one) — the loop is fully
        // observable, not just pending rows.
        let ids: Vec<i64> = mine.iter().map(|r| r.id).collect();
        assert!(ids.contains(&mine_pending.id) && ids.contains(&mine_resolved.id));
        assert!(mine.iter().any(|r| r.status == "pending"));
        assert!(mine.iter().any(|r| r.status == "rejected"));
        // Another app's request is never visible here; an app with none sees an empty list.
        assert_eq!(db.agent_requests_for_agent(2).unwrap().len(), 1);
        assert!(db.agent_requests_for_agent(999).unwrap().is_empty());
    }

    #[test]
    fn file_backed_database_is_encrypted_and_reopens() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hangar.sqlite3");

        {
            let db = Db::open(&path).unwrap();
            db.roots_add("fixture://encrypted-root").unwrap();
        }

        assert!(!is_plaintext_sqlite_database(&path).unwrap());
        let reopened = Db::open(&path).unwrap();
        assert!(reopened
            .roots_list()
            .unwrap()
            .iter()
            .any(|root| root.path == "fixture://encrypted-root"));
    }

    #[test]
    fn file_backed_reads_use_independent_connection_during_writer_transaction() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hangar.sqlite3");
        let db = Db::open(&path).unwrap();
        db.roots_add("fixture://reader-root").unwrap();
        let reader = db.clone();

        db.with_writer(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO setting(key, value) VALUES('test.writer.held', '1')",
                [],
            )?;
            let projects = reader.projects_list()?;
            assert!(projects
                .iter()
                .any(|project| project.path == "fixture://reader-root"));
            tx.rollback()?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn file_backed_preview_without_recent_reads_while_writer_transaction_is_open() {
        let db_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let db_path = db_dir.path().join("hangar.sqlite3");
        let readme_path = project_dir.path().join("README.md");
        fs::write(&readme_path, "# Preview\n\nFast source preview.").unwrap();
        let db = Db::open(&db_path).unwrap();
        db.load_scanned_root(
            project_dir.path().to_string_lossy().as_ref(),
            &[scanned_fixture_for_path(&readme_path, "README.md")],
            None,
        )
        .unwrap();
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.path == project_dir.path().to_string_lossy())
            .unwrap();
        let hit = db
            .project_nav_children(project.id, None, 20, 0)
            .unwrap()
            .items
            .into_iter()
            .find(|item| item.path == "README.md")
            .unwrap();
        let node_id = hit.node_id.unwrap();
        let reader = db.clone();

        db.with_writer(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO setting(key, value) VALUES('test.preview.writer.held', '1')",
                [],
            )?;
            let preview = reader.file_preview(node_id, PreviewMode::Source, false)?;
            assert_eq!(preview.state, PreviewState::Ready);
            assert!(preview
                .source
                .as_deref()
                .is_some_and(|source| source.contains("Fast source preview")));
            tx.rollback()?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn encrypted_database_rejects_wrong_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hangar.sqlite3");
        Db::open(&path).unwrap();

        let conn = Connection::open(&path).unwrap();
        let err = configure_file_connection(&conn, "wrong-local-test-key").unwrap_err();
        assert!(err.to_string().contains("sqlite error"));
    }

    #[test]
    fn plaintext_database_migration_preserves_pins_and_recents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hangar.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            configure_memory_connection(&conn).unwrap();
            conn.execute_batch(MIGRATION_001).unwrap();
            ensure_phase1b_columns(&conn).unwrap();
            insert_default_zones(&conn).unwrap();
            load_fixtures_if_empty(&conn).unwrap();
            let node_id: i64 = conn
                .query_row(
                    "SELECT node_id FROM nav_item WHERE path = 'README.md' AND node_id IS NOT NULL LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO pinned_item(node_id, project_id, item_kind, pinned_at)
                 SELECT ?1, project_id, 'file', ?2 FROM nav_item WHERE node_id = ?1 LIMIT 1",
                params![node_id, now()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO recent_item(node_id, project_id, item_kind, opened_at)
                 SELECT ?1, project_id, 'file', ?2 FROM nav_item WHERE node_id = ?1 LIMIT 1",
                params![node_id, now()],
            )
            .unwrap();
        }

        assert!(is_plaintext_sqlite_database(&path).unwrap());
        let db = Db::open(&path).unwrap();

        assert!(!is_plaintext_sqlite_database(&path).unwrap());
        assert!(!path.with_extension("sqlite3.plaintext-migrating").exists());
        assert!(!db.pinned_items_list().unwrap().is_empty());
        assert!(!db.recent_items_list(10).unwrap().is_empty());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_registration_grants_and_revoke_are_enforced() {
        let db = Db::open_memory().unwrap();
        let projects = db.projects_list().unwrap();
        let project_id = projects[0].id;
        let node_id = db.project_context_files(project_id).unwrap()[0].node_id;
        let scopes = vec!["read_structure".to_string(), "read_body".to_string()];
        let agent = db
            .automation_register("Local test agent", "token-hash", &scopes, &[project_id])
            .unwrap();

        let authenticated = db
            .automation_authenticate("token-hash")
            .unwrap()
            .expect("registered token should authenticate");
        assert_eq!(authenticated.project_ids, vec![project_id]);
        assert_eq!(authenticated.scopes, scopes);

        let grant = db.automation_grant_read(agent.id, node_id, 10_000).unwrap();
        assert!(db
            .automation_has_read_grant(agent.id, node_id, 9_999)
            .unwrap());
        assert!(!db
            .automation_has_read_grant(agent.id, node_id, 10_000)
            .unwrap());

        db.automation_log(Some(agent.id), "status", "allowed", "local test")
            .unwrap();
        assert_eq!(db.automation_activity(10).unwrap().len(), 1);
        assert!(db.automation_revoke(agent.id).unwrap());
        assert!(db.automation_authenticate("token-hash").unwrap().is_none());
        assert!(!db
            .automation_has_read_grant(agent.id, grant.node_id, 9_999)
            .unwrap());
        assert!(db.automation_forget_revoked(agent.id).unwrap());
        assert!(db.automation_agents().unwrap().is_empty());
    }

    fn scanned_fixture_for_path(path: &std::path::Path, relative: &str) -> ScannedFile {
        ScannedFile {
            absolute_path: path.to_string_lossy().to_string(),
            relative_path: relative.to_string(),
            display_path: relative.to_string(),
            display_name: relative.to_string(),
            item_kind: "file".to_string(),
            is_markdown: false,
            is_context: false,
            is_sensitive: false,
            protected_level: None,
            child_count: 0,
            fully_scanned: true,
            collapse_default: false,
            scan_error: None,
            identity: None,
            body: None,
        }
    }

    fn safetensors_fixture_bytes(header: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + header.len());
        bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
        bytes.extend_from_slice(header);
        bytes
    }

    fn gguf_fixture_bytes(version: u32, tensors: u64, metadata: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GGUF");
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&tensors.to_le_bytes());
        bytes.extend_from_slice(&metadata.to_le_bytes());
        bytes
    }

    fn file_identity(
        apparent: u64,
        allocated: Option<u64>,
        volume_id: Option<&str>,
        inode_key: Option<&str>,
    ) -> FileIdentity {
        FileIdentity {
            size_apparent: Some(apparent),
            size_allocated: allocated,
            modified_at: None,
            readonly: false,
            is_symlink: false,
            is_reparse: false,
            reparse_kind: None,
            volume_id: volume_id.map(ToString::to_string),
            inode_key: inode_key.map(ToString::to_string),
            link_count: None,
            inaccessible: false,
            error: None,
        }
    }
}
