//! Connector-only persistence for Safe Manage advisory receipts.
//!
//! The table intentionally cannot store request/result bodies, credentials,
//! provider configuration, display labels or source paths. It records only
//! bounded fingerprints, counts and typed opaque source references.

use super::{now, Db, DbError, DbResult};
use hangar_core::{
    AiSafeManageAdvisoryReceipt, AiSafeManageAdvisoryReceiptStatus,
    AiSafeManageAdvisorySourceReceipt,
};
use rusqlite::{params, Connection, OptionalExtension};

const MAX_RECEIPT_SOURCES: usize = 6;
const MAX_PROVENANCE_JSON_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct NewConnectorAdvisoryReceipt {
    pub receipt_id: String,
    pub project_id: i64,
    pub analysis_run_id: String,
    pub evidence_revision: String,
    pub request_hash: String,
    pub request_chars: u64,
    pub sources: Vec<AiSafeManageAdvisorySourceReceipt>,
}

/// Minimal inventory locator used only while constructing backend-owned
/// Connector candidates. No path leaves the database layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAdvisoryCoreFile {
    pub node_id: i64,
    pub project_id: i64,
    pub display_name: String,
}

pub(super) fn ensure_connector_advisory_schema(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS connector_safe_manage_advisory_receipt (
           receipt_id TEXT PRIMARY KEY,
           project_id INTEGER NOT NULL,
           analysis_run_id TEXT NOT NULL,
           evidence_revision TEXT NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('prepared', 'completed', 'failed')),
           request_hash TEXT NOT NULL,
           request_chars INTEGER NOT NULL,
           source_provenance_json TEXT NOT NULL,
           result_hash TEXT,
           result_chars INTEGER,
           failure_code TEXT,
           created_at TEXT NOT NULL,
           completed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_connector_advisory_receipt_project
           ON connector_safe_manage_advisory_receipt(project_id, created_at DESC, receipt_id DESC);",
    )?;
    Ok(())
}

impl Db {
    pub fn connector_advisory_core_files(
        &self,
        project_id: i64,
    ) -> DbResult<Vec<ConnectorAdvisoryCoreFile>> {
        if project_id <= 0 {
            return Err(DbError::InvalidInput(
                "Connector advisory project id must be positive.".to_string(),
            ));
        }
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ni.node_id, ni.project_id, ni.display_name
                 FROM nav_item ni
                 JOIN node n ON n.id = ni.node_id
                 WHERE ni.project_id = ?1
                   AND ni.item_kind = 'file'
                   AND ni.is_sensitive = 0
                   AND ni.protected_level IS NULL
                   AND n.present = 1
                   AND n.is_reparse = 0
                   AND lower(ni.display_name) IN (
                     'main.rs', 'lib.rs', 'main.py', 'app.py', 'index.ts', 'index.tsx',
                     'app.ts', 'app.tsx', 'main.ts', 'main.tsx', 'program.cs',
                     'main.go', 'mod.rs'
                   )
                 ORDER BY
                   CASE lower(ni.display_name)
                     WHEN 'main.rs' THEN 0 WHEN 'lib.rs' THEN 1
                     WHEN 'main.py' THEN 2 WHEN 'app.py' THEN 3
                     WHEN 'app.tsx' THEN 4 WHEN 'app.ts' THEN 5
                     WHEN 'index.tsx' THEN 6 WHEN 'index.ts' THEN 7
                     ELSE 20
                   END,
                   length(ni.path), ni.sort_key, ni.id
                 LIMIT 12",
            )?;
            let rows = stmt.query_map(params![project_id], |row| {
                Ok(ConnectorAdvisoryCoreFile {
                    node_id: row.get(0)?,
                    project_id: row.get(1)?,
                    display_name: row.get(2)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
        })
    }

    pub fn connector_advisory_receipt_prepare(
        &self,
        input: &NewConnectorAdvisoryReceipt,
    ) -> DbResult<AiSafeManageAdvisoryReceipt> {
        validate_new_receipt(input)?;
        let provenance = serde_json::to_string(&input.sources).map_err(|error| {
            DbError::InvalidInput(format!(
                "Could not encode Connector advisory provenance: {error}"
            ))
        })?;
        if provenance.len() > MAX_PROVENANCE_JSON_BYTES {
            return Err(DbError::InvalidInput(
                "Connector advisory provenance is too large.".to_string(),
            ));
        }
        let created_at = now();
        self.with_conn(|conn| {
            ensure_connector_advisory_schema(conn)?;
            conn.execute(
                "INSERT INTO connector_safe_manage_advisory_receipt(
                   receipt_id, project_id, analysis_run_id, evidence_revision, status,
                   request_hash, request_chars, source_provenance_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, ?6, ?7, ?8)",
                params![
                    input.receipt_id,
                    input.project_id,
                    input.analysis_run_id,
                    input.evidence_revision,
                    input.request_hash,
                    to_i64(input.request_chars, "request character count")?,
                    provenance,
                    created_at,
                ],
            )?;
            load_receipt(conn, &input.receipt_id)?.ok_or_else(|| {
                DbError::InvalidInput("Connector advisory receipt was not persisted.".to_string())
            })
        })
    }

    pub fn connector_advisory_receipt_complete(
        &self,
        receipt_id: &str,
        result_hash: &str,
        result_chars: u64,
    ) -> DbResult<AiSafeManageAdvisoryReceipt> {
        validate_opaque_id(receipt_id, "receipt id", "sm-advisory-", 96)?;
        validate_hash(result_hash, "result hash")?;
        let completed_at = now();
        self.with_conn(|conn| {
            ensure_connector_advisory_schema(conn)?;
            let changed = conn.execute(
                "UPDATE connector_safe_manage_advisory_receipt
                 SET status = 'completed', result_hash = ?2, result_chars = ?3,
                     failure_code = NULL, completed_at = ?4
                 WHERE receipt_id = ?1 AND status = 'prepared'",
                params![
                    receipt_id,
                    result_hash,
                    to_i64(result_chars, "result character count")?,
                    completed_at,
                ],
            )?;
            if changed != 1 {
                return Err(DbError::InvalidInput(
                    "Connector advisory receipt is missing or no longer prepared.".to_string(),
                ));
            }
            load_receipt(conn, receipt_id)?.ok_or_else(|| {
                DbError::InvalidInput(
                    "Connector advisory completion receipt is missing.".to_string(),
                )
            })
        })
    }

    pub fn connector_advisory_receipt_fail(
        &self,
        receipt_id: &str,
        failure_code: &str,
    ) -> DbResult<AiSafeManageAdvisoryReceipt> {
        validate_opaque_id(receipt_id, "receipt id", "sm-advisory-", 96)?;
        if !matches!(
            failure_code,
            "selection_expired"
                | "evidence_changed"
                | "provider_changed"
                | "payload_blocked"
                | "preview_expired"
                | "provider_failed"
                | "internal_error"
        ) {
            return Err(DbError::InvalidInput(
                "Unknown Connector advisory failure category.".to_string(),
            ));
        }
        let completed_at = now();
        self.with_conn(|conn| {
            ensure_connector_advisory_schema(conn)?;
            let changed = conn.execute(
                "UPDATE connector_safe_manage_advisory_receipt
                 SET status = 'failed', result_hash = NULL, result_chars = NULL,
                     failure_code = ?2, completed_at = ?3
                 WHERE receipt_id = ?1 AND status = 'prepared'",
                params![receipt_id, failure_code, completed_at],
            )?;
            if changed != 1 {
                return Err(DbError::InvalidInput(
                    "Connector advisory receipt is missing or no longer prepared.".to_string(),
                ));
            }
            load_receipt(conn, receipt_id)?.ok_or_else(|| {
                DbError::InvalidInput("Connector advisory failure receipt is missing.".to_string())
            })
        })
    }

    pub fn connector_advisory_receipts(
        &self,
        project_id: i64,
        limit: usize,
    ) -> DbResult<Vec<AiSafeManageAdvisoryReceipt>> {
        if project_id <= 0 {
            return Err(DbError::InvalidInput(
                "Connector advisory project id must be positive.".to_string(),
            ));
        }
        let limit = limit.clamp(1, 100);
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT receipt_id, project_id, analysis_run_id, evidence_revision, status,
                        request_hash, request_chars, source_provenance_json, result_hash,
                        result_chars, failure_code, created_at, completed_at
                 FROM connector_safe_manage_advisory_receipt
                 WHERE project_id = ?1
                 ORDER BY created_at DESC, receipt_id DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project_id, limit as i64], row_to_receipt)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
        })
    }
}

fn load_receipt(
    conn: &Connection,
    receipt_id: &str,
) -> DbResult<Option<AiSafeManageAdvisoryReceipt>> {
    conn.query_row(
        "SELECT receipt_id, project_id, analysis_run_id, evidence_revision, status,
                request_hash, request_chars, source_provenance_json, result_hash,
                result_chars, failure_code, created_at, completed_at
         FROM connector_safe_manage_advisory_receipt
         WHERE receipt_id = ?1",
        params![receipt_id],
        row_to_receipt,
    )
    .optional()
    .map_err(DbError::from)
}

fn row_to_receipt(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiSafeManageAdvisoryReceipt> {
    let status: String = row.get(4)?;
    let provenance: String = row.get(7)?;
    let sources = serde_json::from_str(&provenance).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let status = match status.as_str() {
        "prepared" => AiSafeManageAdvisoryReceiptStatus::Prepared,
        "completed" => AiSafeManageAdvisoryReceiptStatus::Completed,
        "failed" => AiSafeManageAdvisoryReceiptStatus::Failed,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                "invalid Connector advisory receipt status".into(),
            ))
        }
    };
    Ok(AiSafeManageAdvisoryReceipt {
        receipt_id: row.get(0)?,
        project_id: row.get(1)?,
        analysis_run_id: row.get(2)?,
        evidence_revision: row.get(3)?,
        status,
        request_hash: row.get(5)?,
        request_chars: from_i64(row.get(6)?, "request_chars")?,
        sources,
        result_hash: row.get(8)?,
        result_chars: row
            .get::<_, Option<i64>>(9)?
            .map(|value| from_i64(value, "result_chars"))
            .transpose()?,
        failure_code: row.get(10)?,
        created_at: row.get(11)?,
        completed_at: row.get(12)?,
    })
}

fn validate_new_receipt(input: &NewConnectorAdvisoryReceipt) -> DbResult<()> {
    validate_opaque_id(&input.receipt_id, "receipt id", "sm-advisory-", 96)?;
    if input.project_id <= 0 {
        return Err(DbError::InvalidInput(
            "Connector advisory project id must be positive.".to_string(),
        ));
    }
    validate_opaque_id(&input.analysis_run_id, "analysis run id", "", 128)?;
    validate_hash(&input.evidence_revision, "evidence revision")?;
    validate_hash(&input.request_hash, "request hash")?;
    to_i64(input.request_chars, "request character count")?;
    if input.sources.is_empty() || input.sources.len() > MAX_RECEIPT_SOURCES {
        return Err(DbError::InvalidInput(format!(
            "Connector advisory provenance must contain 1 to {MAX_RECEIPT_SOURCES} sources."
        )));
    }
    for source in &input.sources {
        validate_opaque_id(&source.selection_id, "selection id", "sm-context-", 96)?;
        validate_hash(&source.content_hash, "source content hash")?;
        to_i64(source.excerpt_chars, "source excerpt character count")?;
        to_i64(source.redaction_count, "source redaction count")?;
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> DbResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DbError::InvalidInput(format!(
            "Connector advisory {label} must be a 64-character hexadecimal fingerprint."
        )));
    }
    Ok(())
}

fn validate_opaque_id(
    value: &str,
    label: &str,
    required_prefix: &str,
    max_len: usize,
) -> DbResult<()> {
    if value.is_empty()
        || value.len() > max_len
        || !value.starts_with(required_prefix)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(DbError::InvalidInput(format!(
            "Connector advisory {label} is invalid."
        )));
    }
    Ok(())
}

fn to_i64(value: u64, label: &str) -> DbResult<i64> {
    i64::try_from(value)
        .map_err(|_| DbError::InvalidInput(format!("Connector advisory {label} is too large.")))
}

fn from_i64(value: i64, label: &str) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            format!("negative Connector advisory {label}").into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_core::{AiSafeManageAdvisoryReceiptStatus, AiSafeManageContextKind};

    fn prepared_input() -> NewConnectorAdvisoryReceipt {
        NewConnectorAdvisoryReceipt {
            receipt_id: "sm-advisory-test-receipt".to_string(),
            project_id: 7,
            analysis_run_id: "analysis-test-run".to_string(),
            evidence_revision: "a".repeat(64),
            request_hash: "b".repeat(64),
            request_chars: 234,
            sources: vec![AiSafeManageAdvisorySourceReceipt {
                selection_id: "sm-context-test-source".to_string(),
                kind: AiSafeManageContextKind::Readme,
                content_hash: "c".repeat(64),
                excerpt_chars: 123,
                redaction_count: 2,
            }],
        }
    }

    #[test]
    fn receipt_lifecycle_persists_only_fingerprints_counts_and_typed_sources() {
        let db = Db::open_memory().expect("memory database");
        let prepared = db
            .connector_advisory_receipt_prepare(&prepared_input())
            .expect("prepared receipt");
        assert_eq!(prepared.status, AiSafeManageAdvisoryReceiptStatus::Prepared);

        let completed = db
            .connector_advisory_receipt_complete(&prepared.receipt_id, &"d".repeat(64), 345)
            .expect("completed receipt");
        assert_eq!(
            completed.status,
            AiSafeManageAdvisoryReceiptStatus::Completed
        );
        assert_eq!(completed.result_chars, Some(345));

        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM pragma_table_info('connector_safe_manage_advisory_receipt')",
            )?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for forbidden in [
                "body",
                "payload",
                "content",
                "response",
                "credential",
                "secret",
                "key",
                "path",
                "url",
                "endpoint",
                "model",
                "label",
            ] {
                assert!(
                    names.iter().all(|name| !name.contains(forbidden)),
                    "forbidden receipt column {forbidden}: {names:?}"
                );
            }
            Ok(())
        })
        .expect("schema inspection");
    }

    #[test]
    fn receipt_rejects_path_shaped_or_untyped_provenance_ids() {
        let db = Db::open_memory().expect("memory database");
        let mut input = prepared_input();
        input.sources[0].selection_id = r"C:\Users\user\secret.txt".to_string();
        assert!(db.connector_advisory_receipt_prepare(&input).is_err());
    }
}
