//! Persistence for v2rayR.
//!
//! Everything the app needs to survive a restart lives here: the node list, the
//! subscriptions and their per-subscription state, custom routing rules, the
//! traffic samples behind the chart, and a small key/value table for "last used
//! node", "last takeover mode" and similar session memory.
//!
//! Design notes
//! ------------
//! * One writer. The desktop app serialises mutations through `v2rayr-app`, so a
//!   single connection behind a `Mutex` is both sufficient and simpler than a
//!   pool.
//! * Nodes are stored as a JSON payload (`domain::Profile`) plus the columns we
//!   need to filter on. Adding a protocol must not require a migration.
//! * `fingerprint` is unique: importing the same node twice updates the row
//!   instead of duplicating it, and the id stays stable across subscription
//!   refreshes so "favourite"/"latency" survive.
//! * Migrations are append-only and idempotent; `PRAGMA user_version` records the
//!   schema revision.

mod profiles;
mod rules;
mod secret;
mod state;
mod subscriptions;
mod traffic;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
pub use secret::{Sealer, open_sealer};
use thiserror::Error;
use v2rayr_domain::{DomainError, ErrorCode};

pub use profiles::{LatencySample, ProfileUpsert, StoredProfile};
pub use rules::{RuleAction, RuleKind, RuleRow};
pub use state::SessionMemory;
pub use subscriptions::{StoredSubscription, SubscriptionUpsert};
pub use traffic::TrafficSample;

/// Latest schema revision this build knows how to write.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("payload: {0}")]
    Json(#[from] serde_json::Error),
    #[error("domain: {0}")]
    Domain(#[from] DomainError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("database schema {found} is newer than this build ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("not found: {0}")]
    NotFound(String),
}

impl StoreError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Sqlite(_) | Self::Io(_) => ErrorCode::Storage,
            Self::Json(_) => ErrorCode::Storage,
            Self::Domain(error) => error.code(),
            Self::SchemaTooNew { .. } => ErrorCode::ConfigInvalid,
            Self::NotFound(_) => ErrorCode::NotFound,
        }
    }
}

pub struct Database {
    conn: Connection,
    path: Option<PathBuf>,
}

impl Database {
    /// Open (and create) the database at `path`, applying pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        restrict_permissions(&path)?;
        let mut db = Self {
            conn,
            path: Some(path),
        };
        db.prepare()?;
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn, path: None };
        db.prepare()?;
        db.migrate()?;
        Ok(db)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? as u32)
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    fn prepare(&mut self) -> Result<(), StoreError> {
        self.conn.pragma_update(None, "journal_mode", "WAL")?;
        self.conn.pragma_update(None, "synchronous", "NORMAL")?;
        self.conn.pragma_update(None, "foreign_keys", "ON")?;
        self.conn.pragma_update(None, "busy_timeout", 5_000)?;
        Ok(())
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let found = self.schema_version()?;
        if found > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found == SCHEMA_VERSION {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        if found < 1 {
            tx.execute_batch(MIGRATION_1)?;
        }
        if found < 2 {
            tx.execute_batch(
                "ALTER TABLE rules ADD COLUMN applications TEXT NOT NULL DEFAULT '[]';",
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        tracing::info!(from = found, to = SCHEMA_VERSION, "database migrated");
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    url             TEXT NOT NULL UNIQUE,
    enabled         INTEGER NOT NULL DEFAULT 1,
    auto_update     INTEGER NOT NULL DEFAULT 1,
    interval_hours  INTEGER NOT NULL DEFAULT 12,
    last_updated_at TEXT,
    next_update_at  TEXT,
    last_error      TEXT,
    quota           TEXT,
    format          TEXT,
    created_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
    id               TEXT PRIMARY KEY,
    sub_id           TEXT REFERENCES subscriptions(id) ON DELETE CASCADE,
    remark           TEXT NOT NULL,
    address          TEXT NOT NULL,
    port             INTEGER NOT NULL,
    protocol         TEXT NOT NULL,
    transport        TEXT NOT NULL,
    payload          TEXT NOT NULL,
    fingerprint      TEXT NOT NULL,
    favorite         INTEGER NOT NULL DEFAULT 0,
    tags             TEXT NOT NULL DEFAULT '[]',
    latency_ms       INTEGER,
    latency_method   TEXT,
    latency_tested_at TEXT,
    last_used_at     TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS profiles_fingerprint ON profiles(fingerprint);
CREATE INDEX IF NOT EXISTS profiles_subscription ON profiles(sub_id);
CREATE INDEX IF NOT EXISTS profiles_remark ON profiles(remark);

CREATE TABLE IF NOT EXISTS rules (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    value      TEXT NOT NULL,
    action     TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1,
    position   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS traffic_samples (
    ts         INTEGER PRIMARY KEY,
    down_bytes INTEGER NOT NULL,
    up_bytes   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS session_memory (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_rules_migrate_to_any_application() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATION_1).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 1;
            INSERT INTO rules VALUES ('00000000-0000-0000-0000-000000000001',
                'domain', 'example.com', 'direct', 1, 0, '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&path).unwrap();
        let rules = db.list_rules().unwrap();
        assert_eq!(rules[0].value, "example.com");
        assert!(rules[0].applications.is_empty());
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn fresh_database_is_at_the_current_schema() {
        let db = Database::open_in_memory().expect("open");
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
    }

    #[test]
    fn opening_twice_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("v2rayr.db");
        {
            let db = Database::open(&path).expect("first open");
            assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
        }
        let db = Database::open(&path).expect("second open");
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_is_refused_instead_of_downgraded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("v2rayr.db");
        {
            let db = Database::open(&path).expect("open");
            db.conn
                .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .expect("bump");
        }
        match Database::open(&path) {
            Err(StoreError::SchemaTooNew { found, supported }) => {
                assert_eq!(found, SCHEMA_VERSION + 1);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            Err(other) => panic!("expected SchemaTooNew, got {}", other.code().as_str()),
            Ok(_) => panic!("expected SchemaTooNew, got an opened database"),
        }
    }
}
