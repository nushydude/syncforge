use std::path::Path;

use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::duplicates::{self, DuplicateScanJob};
use crate::models::{
    ConflictPolicy, FileEntry, FolderPair, RunItem, RunReport, RunStatus, Snapshot, SyncMode,
};

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("pair not found: {0}")]
    PairNotFound(String),
}

pub type Result<T> = std::result::Result<T, PersistenceError>;

/// Maximum snapshot rows retained per folder pair (newest by `captured_at`).
const SNAPSHOT_RETAIN_COUNT: usize = 3;

/// Default cap for history list queries (newest runs first).
const HISTORY_RUNS_LIMIT: i64 = 100;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let initialized: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'pairs'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !initialized {
            self.conn.execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS pairs (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    left_path TEXT NOT NULL,
                    right_path TEXT NOT NULL,
                    mode TEXT NOT NULL,
                    filters_json TEXT NOT NULL,
                    conflict_policy TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    watch_enabled INTEGER NOT NULL DEFAULT 0,
                    schedule_enabled INTEGER NOT NULL DEFAULT 0,
                    schedule_cron TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS snapshots (
                    id TEXT PRIMARY KEY NOT NULL,
                    pair_id TEXT NOT NULL,
                    captured_at INTEGER NOT NULL,
                    entries_json TEXT NOT NULL,
                    FOREIGN KEY (pair_id) REFERENCES pairs(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS runs (
                    id TEXT PRIMARY KEY NOT NULL,
                    pair_id TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER,
                    status TEXT NOT NULL,
                    summary_json TEXT NOT NULL,
                    FOREIGN KEY (pair_id) REFERENCES pairs(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS run_items (
                    id TEXT PRIMARY KEY NOT NULL,
                    run_id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    action TEXT NOT NULL,
                    status TEXT NOT NULL,
                    message TEXT,
                    bytes INTEGER,
                    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_snapshots_pair_id ON snapshots(pair_id);
                CREATE INDEX IF NOT EXISTS idx_runs_pair_id ON runs(pair_id);
                CREATE INDEX IF NOT EXISTS idx_run_items_run_id ON run_items(run_id);
                ",
            )?;
        }

        self.migrate_watch_enabled()?;
        self.migrate_schedule_fields()?;
        self.migrate_duplicate_scans()?;

        Ok(())
    }

    fn migrate_watch_enabled(&self) -> Result<()> {
        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('pairs') WHERE name = 'watch_enabled'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_column {
            self.conn.execute(
                "ALTER TABLE pairs ADD COLUMN watch_enabled INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        Ok(())
    }

    fn migrate_schedule_fields(&self) -> Result<()> {
        let has_enabled: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('pairs') WHERE name = 'schedule_enabled'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_enabled {
            self.conn.execute(
                "ALTER TABLE pairs ADD COLUMN schedule_enabled INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        let has_cron: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('pairs') WHERE name = 'schedule_cron'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_cron {
            self.conn.execute("ALTER TABLE pairs ADD COLUMN schedule_cron TEXT", [])?;
        }

        Ok(())
    }

    fn migrate_duplicate_scans(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS duplicate_scans (
                id TEXT PRIMARY KEY NOT NULL,
                root TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                phase TEXT,
                files_found INTEGER NOT NULL DEFAULT 0,
                total_files INTEGER,
                hashed_files INTEGER NOT NULL DEFAULT 0,
                hash_total INTEGER,
                bytes_processed INTEGER NOT NULL DEFAULT 0,
                bytes_total INTEGER,
                current_path TEXT,
                cancel_requested INTEGER NOT NULL DEFAULT 0,
                result_json TEXT,
                error TEXT,
                started_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_duplicate_scans_updated_at
                ON duplicate_scans(updated_at DESC);",
        )?;
        Ok(())
    }

    pub fn list_pairs(&self) -> Result<Vec<FolderPair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, left_path, right_path, mode, filters_json, conflict_policy,
                    enabled, watch_enabled, schedule_enabled, schedule_cron, created_at, updated_at
             FROM pairs
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
            ))
        })?;

        rows.map(|row| {
            let (
                id,
                name,
                left_path,
                right_path,
                mode,
                filters_json,
                conflict_policy,
                enabled,
                watch_enabled,
                schedule_enabled,
                schedule_cron,
                created_at,
                updated_at,
            ) = row?;
            Ok(row_to_pair(
                id,
                name,
                left_path,
                right_path,
                mode,
                filters_json,
                conflict_policy,
                enabled,
                watch_enabled,
                schedule_enabled,
                schedule_cron,
                created_at,
                updated_at,
            )?)
        })
        .collect()
    }

    pub fn save_duplicate_scan(&self, job: &DuplicateScanJob) -> Result<()> {
        let result_json = job.result.as_ref().map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "INSERT INTO duplicate_scans (
                id, root, mode, status, phase, files_found, total_files,
                hashed_files, hash_total, bytes_processed, bytes_total,
                current_path, cancel_requested, result_json, error, started_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                root = excluded.root,
                mode = excluded.mode,
                status = excluded.status,
                phase = excluded.phase,
                files_found = excluded.files_found,
                total_files = excluded.total_files,
                hashed_files = excluded.hashed_files,
                hash_total = excluded.hash_total,
                bytes_processed = excluded.bytes_processed,
                bytes_total = excluded.bytes_total,
                current_path = excluded.current_path,
                cancel_requested = excluded.cancel_requested,
                result_json = excluded.result_json,
                error = excluded.error,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at",
            rusqlite::params![
                job.id,
                job.root,
                duplicates::mode_to_str(job.mode),
                duplicates::status_to_str(job.status),
                duplicates::phase_to_str(job.phase),
                job.files_found as i64,
                job.total_files.map(|value| value as i64),
                job.hashed_files as i64,
                job.hash_total.map(|value| value as i64),
                job.bytes_processed as i64,
                job.bytes_total.map(|value| value as i64),
                job.current_path,
                job.cancel_requested as i64,
                result_json,
                job.error,
                job.started_at,
                job.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_duplicate_scan(&self, id: &str) -> Result<Option<DuplicateScanJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root, mode, status, phase, files_found, total_files,
                    hashed_files, hash_total, bytes_processed, bytes_total,
                    current_path, cancel_requested, result_json, error, started_at, updated_at
             FROM duplicate_scans WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        rows.next()?.map(row_to_duplicate_scan).transpose()
    }

    pub fn latest_duplicate_scan(&self) -> Result<Option<DuplicateScanJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root, mode, status, phase, files_found, total_files,
                    hashed_files, hash_total, bytes_processed, bytes_total,
                    current_path, cancel_requested, result_json, error, started_at, updated_at
             FROM duplicate_scans ORDER BY updated_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        rows.next()?.map(row_to_duplicate_scan).transpose()
    }

    pub fn mark_duplicate_scans_interrupted(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE duplicate_scans
             SET status = 'interrupted',
                 cancel_requested = 0,
                 error = 'The previous scan was interrupted. Resume it to continue.',
                 updated_at = updated_at
             WHERE status = 'running'",
            [],
        )?;
        Ok(())
    }

    pub fn save_pair(&self, pair: &FolderPair) -> Result<FolderPair> {
        let filters_json = serde_json::to_string(&pair.filters)?;
        let mode = sync_mode_to_str(pair.mode);
        let conflict_policy = conflict_policy_to_str(pair.conflict_policy);

        self.conn.execute(
            "INSERT INTO pairs (
                id, name, left_path, right_path, mode, filters_json, conflict_policy,
                enabled, watch_enabled, schedule_enabled, schedule_cron, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                left_path = excluded.left_path,
                right_path = excluded.right_path,
                mode = excluded.mode,
                filters_json = excluded.filters_json,
                conflict_policy = excluded.conflict_policy,
                enabled = excluded.enabled,
                watch_enabled = excluded.watch_enabled,
                schedule_enabled = excluded.schedule_enabled,
                schedule_cron = excluded.schedule_cron,
                updated_at = excluded.updated_at",
            params![
                pair.id,
                pair.name,
                pair.left_path,
                pair.right_path,
                mode,
                filters_json,
                conflict_policy,
                pair.enabled as i64,
                pair.watch_enabled as i64,
                pair.schedule_enabled as i64,
                pair.schedule_cron,
                pair.created_at,
                pair.updated_at,
            ],
        )?;

        self.get_pair(&pair.id)?.ok_or_else(|| PersistenceError::PairNotFound(pair.id.clone()))
    }

    pub fn delete_pair(&self, id: &str) -> Result<()> {
        let changed = self.conn.execute("DELETE FROM pairs WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(PersistenceError::PairNotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn get_pair(&self, id: &str) -> Result<Option<FolderPair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, left_path, right_path, mode, filters_json, conflict_policy,
                    enabled, watch_enabled, schedule_enabled, schedule_cron, created_at, updated_at
             FROM pairs WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let pair = row_to_pair(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
            )?;
            return Ok(Some(pair));
        }
        Ok(None)
    }

    pub fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let entries_json = serde_json::to_string(&snapshot.entries)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO snapshots (id, pair_id, captured_at, entries_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                pair_id = excluded.pair_id,
                captured_at = excluded.captured_at,
                entries_json = excluded.entries_json",
            params![snapshot.id, snapshot.pair_id, snapshot.captured_at, entries_json],
        )?;
        Self::prune_snapshots_for_pair(&tx, &snapshot.pair_id)?;
        tx.commit()?;
        Ok(())
    }

    fn prune_snapshots_for_pair(conn: &Connection, pair_id: &str) -> Result<()> {
        let retain = SNAPSHOT_RETAIN_COUNT as i64;
        conn.execute(
            "DELETE FROM snapshots
             WHERE pair_id = ?1
               AND id NOT IN (
                 SELECT id FROM snapshots
                 WHERE pair_id = ?1
                 ORDER BY captured_at DESC
                 LIMIT ?2
               )",
            params![pair_id, retain],
        )?;
        Ok(())
    }

    pub fn latest_snapshot(&self, pair_id: &str) -> Result<Option<Snapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pair_id, captured_at, entries_json
             FROM snapshots
             WHERE pair_id = ?1
             ORDER BY captured_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![pair_id])?;
        if let Some(row) = rows.next()? {
            let entries: Vec<FileEntry> = serde_json::from_str(&row.get::<_, String>(3)?)?;
            return Ok(Some(Snapshot {
                id: row.get(0)?,
                pair_id: row.get(1)?,
                captured_at: row.get(2)?,
                entries,
            }));
        }
        Ok(None)
    }

    pub fn save_run(&self, report: &RunReport) -> Result<()> {
        let summary_json = serde_json::to_string(report)?;
        self.conn.execute(
            "INSERT INTO runs (id, pair_id, started_at, finished_at, status, summary_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                finished_at = excluded.finished_at,
                status = excluded.status,
                summary_json = excluded.summary_json",
            params![
                report.run_id,
                report.pair_id,
                report.started_at,
                report.finished_at,
                run_status_to_str(report.status),
                summary_json,
            ],
        )?;
        Ok(())
    }

    /// Convenience wrapper around [`Self::insert_run_items`].
    #[allow(dead_code)]
    pub fn insert_run_item(&self, item: &RunItem) -> Result<()> {
        self.insert_run_items(std::slice::from_ref(item))
    }

    pub fn insert_run_items(&self, items: &[RunItem]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for item in items {
            tx.execute(
                "INSERT INTO run_items (id, run_id, path, action, status, message, bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    item.id,
                    item.run_id,
                    item.path,
                    item.action,
                    item.status,
                    item.message,
                    item.bytes.map(|b| b as i64),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Past runs, newest first. When `pair_id` is set, only runs for that pair.
    pub fn list_runs(&self, pair_id: Option<&str>) -> Result<Vec<RunReport>> {
        let mut reports = Vec::new();
        match pair_id {
            Some(id) => {
                let mut stmt = self.conn.prepare(
                    "SELECT summary_json FROM runs
                     WHERE pair_id = ?1
                     ORDER BY started_at DESC
                     LIMIT ?2",
                )?;
                let rows =
                    stmt.query_map(params![id, HISTORY_RUNS_LIMIT], |row| row.get::<_, String>(0))?;
                for json in rows {
                    reports.push(serde_json::from_str(&json?)?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT summary_json FROM runs ORDER BY started_at DESC LIMIT ?1")?;
                let rows =
                    stmt.query_map(params![HISTORY_RUNS_LIMIT], |row| row.get::<_, String>(0))?;
                for json in rows {
                    reports.push(serde_json::from_str(&json?)?);
                }
            }
        }
        Ok(reports)
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<RunReport>> {
        let mut stmt = self.conn.prepare("SELECT summary_json FROM runs WHERE id = ?1")?;
        let mut rows = stmt.query(params![run_id])?;
        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            return Ok(Some(serde_json::from_str(&json)?));
        }
        Ok(None)
    }

    pub fn list_run_items(&self, run_id: &str) -> Result<Vec<RunItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, path, action, status, message, bytes
             FROM run_items
             WHERE run_id = ?1
             ORDER BY path COLLATE NOCASE",
        )?;
        let mut items = Vec::new();
        let mut rows = stmt.query(params![run_id])?;
        while let Some(row) = rows.next()? {
            let bytes: Option<i64> = row.get(6)?;
            items.push(RunItem {
                id: row.get(0)?,
                run_id: row.get(1)?,
                path: row.get(2)?,
                action: row.get(3)?,
                status: row.get(4)?,
                message: row.get(5)?,
                bytes: bytes.map(|b| b as u64),
            });
        }
        Ok(items)
    }
}

impl std::convert::From<std::io::Error> for PersistenceError {
    fn from(value: std::io::Error) -> Self {
        PersistenceError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(value)))
    }
}

pub fn new_pair_id() -> String {
    Uuid::new_v4().to_string()
}

fn row_to_pair(
    id: String,
    name: String,
    left_path: String,
    right_path: String,
    mode: String,
    filters_json: String,
    conflict_policy: String,
    enabled: i64,
    watch_enabled: i64,
    schedule_enabled: i64,
    schedule_cron: Option<String>,
    created_at: i64,
    updated_at: i64,
) -> Result<FolderPair> {
    Ok(FolderPair {
        id,
        name,
        left_path,
        right_path,
        mode: str_to_sync_mode(&mode)?,
        filters: serde_json::from_str(&filters_json)?,
        conflict_policy: str_to_conflict_policy(&conflict_policy)?,
        enabled: enabled != 0,
        watch_enabled: watch_enabled != 0,
        schedule_enabled: schedule_enabled != 0,
        schedule_cron,
        created_at,
        updated_at,
    })
}

fn row_to_duplicate_scan(row: &rusqlite::Row<'_>) -> Result<DuplicateScanJob> {
    let mode: String = row.get(2)?;
    let status: String = row.get(3)?;
    let phase: Option<String> = row.get(4)?;
    let result_json: Option<String> = row.get(13)?;
    let mode = duplicates::mode_from_str(&mode).map_err(rusqlite::Error::InvalidParameterName)?;
    let status =
        duplicates::status_from_str(&status).map_err(rusqlite::Error::InvalidParameterName)?;
    let phase = duplicates::phase_from_str(phase).map_err(rusqlite::Error::InvalidParameterName)?;
    let result = result_json.map(|json| serde_json::from_str(&json)).transpose()?;

    Ok(DuplicateScanJob {
        id: row.get(0)?,
        root: row.get(1)?,
        mode,
        status,
        phase,
        files_found: row.get::<_, i64>(5)? as u64,
        total_files: row.get::<_, Option<i64>>(6)?.map(|value| value as u64),
        hashed_files: row.get::<_, i64>(7)? as u64,
        hash_total: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
        bytes_processed: row.get::<_, i64>(9)? as u64,
        bytes_total: row.get::<_, Option<i64>>(10)?.map(|value| value as u64),
        current_path: row.get(11)?,
        cancel_requested: row.get::<_, i64>(12)? != 0,
        result,
        error: row.get(14)?,
        started_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

fn sync_mode_to_str(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Synchronize => "synchronize",
        SyncMode::Echo => "echo",
        SyncMode::Contribute => "contribute",
    }
}

fn str_to_sync_mode(value: &str) -> Result<SyncMode> {
    match value {
        "synchronize" => Ok(SyncMode::Synchronize),
        "echo" => Ok(SyncMode::Echo),
        "contribute" => Ok(SyncMode::Contribute),
        other => {
            Err(PersistenceError::Database(rusqlite::Error::InvalidParameterName(other.into())))
        }
    }
}

fn conflict_policy_to_str(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::NewerWins => "newerWins",
        ConflictPolicy::Left => "left",
        ConflictPolicy::Right => "right",
        ConflictPolicy::KeepBoth => "keepBoth",
        ConflictPolicy::Ask => "ask",
    }
}

fn str_to_conflict_policy(value: &str) -> Result<ConflictPolicy> {
    match value {
        "newerWins" => Ok(ConflictPolicy::NewerWins),
        "left" => Ok(ConflictPolicy::Left),
        "right" => Ok(ConflictPolicy::Right),
        "keepBoth" => Ok(ConflictPolicy::KeepBoth),
        "ask" => Ok(ConflictPolicy::Ask),
        other => {
            Err(PersistenceError::Database(rusqlite::Error::InvalidParameterName(other.into())))
        }
    }
}

fn run_status_to_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duplicates::{
        DuplicateMatchMode, DuplicateScanJob, DuplicateScanPhase, DuplicateScanStatus,
    };
    use crate::models::Filters;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::open(&path).expect("open db");
        (dir, db)
    }

    fn snapshot_count_for_pair(db: &Database, pair_id: &str) -> i64 {
        db.conn
            .query_row(
                "SELECT COUNT(*) FROM snapshots WHERE pair_id = ?1",
                params![pair_id],
                |row| row.get(0),
            )
            .expect("count snapshots")
    }

    #[test]
    fn open_sets_wal_and_busy_timeout() {
        let (_dir, db) = temp_db();
        let journal_mode: String =
            db.conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).expect("journal_mode");
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        let busy_timeout: i64 =
            db.conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0)).expect("busy_timeout");
        assert_eq!(busy_timeout, 5000);
    }

    #[test]
    fn save_snapshot_prunes_older_rows_for_pair() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "Prune".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.save_pair(&pair).expect("save pair");

        for captured_at in 1..=5_i64 {
            db.save_snapshot(&Snapshot {
                id: Uuid::new_v4().to_string(),
                pair_id: pair.id.clone(),
                captured_at,
                entries: vec![],
            })
            .expect("save snapshot");
        }

        assert_eq!(snapshot_count_for_pair(&db, &pair.id), SNAPSHOT_RETAIN_COUNT as i64);
        let latest = db.latest_snapshot(&pair.id).expect("latest").expect("snapshot");
        assert_eq!(latest.captured_at, 5);
    }

    #[test]
    fn two_snapshots_for_same_pair_keeps_both_under_retain_limit() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "Pair".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.save_pair(&pair).expect("save pair");

        db.save_snapshot(&Snapshot {
            id: Uuid::new_v4().to_string(),
            pair_id: pair.id.clone(),
            captured_at: 1,
            entries: vec![],
        })
        .expect("first snapshot");
        db.save_snapshot(&Snapshot {
            id: Uuid::new_v4().to_string(),
            pair_id: pair.id.clone(),
            captured_at: 2,
            entries: vec![],
        })
        .expect("second snapshot");

        assert_eq!(snapshot_count_for_pair(&db, &pair.id), 2);
        let latest = db.latest_snapshot(&pair.id).expect("latest").expect("snapshot");
        assert_eq!(latest.captured_at, 2);
    }

    #[test]
    fn insert_run_items_batches_in_one_transaction() {
        let (_dir, db) = temp_db();
        let pair_id = new_pair_id();
        db.save_pair(&FolderPair {
            id: pair_id.clone(),
            name: "Batch".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        })
        .expect("save pair");
        let run_id = Uuid::new_v4().to_string();
        db.save_run(&RunReport {
            run_id: run_id.clone(),
            pair_id,
            started_at: 1,
            finished_at: None,
            status: RunStatus::Running,
            files_copied: 0,
            files_deleted: 0,
            bytes_transferred: 0,
            errors: vec![],
        })
        .expect("save run");
        let items: Vec<RunItem> = (0..3)
            .map(|i| RunItem {
                id: Uuid::new_v4().to_string(),
                run_id: run_id.clone(),
                path: format!("file{i}.txt"),
                action: "copyLeftToRight".into(),
                status: "completed".into(),
                message: None,
                bytes: Some(i as u64),
            })
            .collect();
        db.insert_run_items(&items).expect("insert batch");
        let loaded = db.list_run_items(&run_id).expect("list");
        assert_eq!(loaded.len(), 3);
    }

    #[test]
    fn watch_enabled_persists() {
        let (_dir, db) = temp_db();
        let mut pair = FolderPair {
            id: new_pair_id(),
            name: "Watch".into(),
            left_path: r"C:\left".into(),
            right_path: r"D:\right".into(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: true,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 100,
            updated_at: 200,
        };
        db.save_pair(&pair).expect("save");
        let loaded = db.get_pair(&pair.id).expect("get").expect("pair");
        assert!(loaded.watch_enabled);

        pair.watch_enabled = false;
        db.save_pair(&pair).expect("update");
        let loaded = db.get_pair(&pair.id).expect("get").expect("pair");
        assert!(!loaded.watch_enabled);
    }

    #[test]
    fn schedule_fields_persist() {
        let (_dir, db) = temp_db();
        let mut pair = FolderPair {
            id: new_pair_id(),
            name: "Scheduled".into(),
            left_path: r"C:\left".into(),
            right_path: r"D:\right".into(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: true,
            schedule_cron: Some("0 9 * * *".into()),
            created_at: 100,
            updated_at: 200,
        };
        db.save_pair(&pair).expect("save");
        let loaded = db.get_pair(&pair.id).expect("get").expect("pair");
        assert!(loaded.schedule_enabled);
        assert_eq!(loaded.schedule_cron.as_deref(), Some("0 9 * * *"));

        pair.schedule_enabled = false;
        pair.schedule_cron = None;
        db.save_pair(&pair).expect("update");
        let loaded = db.get_pair(&pair.id).expect("get").expect("pair");
        assert!(!loaded.schedule_enabled);
        assert!(loaded.schedule_cron.is_none());
    }

    #[test]
    fn pair_crud_round_trip() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "Test".into(),
            left_path: r"C:\left".into(),
            right_path: r"D:\right".into(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::Ask,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 100,
            updated_at: 200,
        };

        db.save_pair(&pair).expect("save");
        let listed = db.list_pairs().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], pair);

        db.delete_pair(&pair.id).expect("delete");
        assert!(db.list_pairs().expect("list").is_empty());
    }

    #[test]
    fn snapshot_and_run_tables_accept_rows() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "History".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.save_pair(&pair).expect("save pair");

        let snapshot = Snapshot {
            id: Uuid::new_v4().to_string(),
            pair_id: pair.id.clone(),
            captured_at: 10,
            entries: vec![FileEntry {
                relative_path: "file.txt".into(),
                size: 5,
                modified_secs: 9,
                modified_nanos: 123,
                is_dir: false,
                hash: None,
            }],
        };
        db.save_snapshot(&snapshot).expect("save snapshot");
        assert!(db.latest_snapshot(&pair.id).expect("latest").is_some());

        let report = RunReport {
            run_id: Uuid::new_v4().to_string(),
            pair_id: pair.id.clone(),
            started_at: 20,
            finished_at: Some(30),
            status: RunStatus::Completed,
            files_copied: 1,
            files_deleted: 0,
            bytes_transferred: 5,
            errors: vec![],
        };
        db.save_run(&report).expect("save run");

        let item = RunItem {
            id: Uuid::new_v4().to_string(),
            run_id: report.run_id.clone(),
            path: "file.txt".into(),
            action: "copyLeftToRight".into(),
            status: "completed".into(),
            message: None,
            bytes: Some(5),
        };
        db.insert_run_item(&item).expect("insert item");
    }

    #[test]
    fn snapshot_loads_entries_without_modified_nanos() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "Legacy".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.save_pair(&pair).expect("save pair");

        let legacy_json =
            r#"[{"relativePath":"old.txt","size":3,"modifiedSecs":42,"isDir":false}]"#;
        db.conn
            .execute(
                "INSERT INTO snapshots (id, pair_id, captured_at, entries_json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    Uuid::new_v4().to_string(),
                    pair.id,
                    10_i64,
                    legacy_json,
                ],
            )
            .expect("insert legacy snapshot");

        let loaded = db.latest_snapshot(&pair.id).expect("load").expect("snapshot");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].modified_nanos, 0);
        assert_eq!(loaded.entries[0].modified_secs, 42);
    }

    #[test]
    fn list_runs_and_detail_round_trip() {
        let (_dir, db) = temp_db();
        let pair_a = new_pair_id();
        let pair_b = new_pair_id();
        for id in [&pair_a, &pair_b] {
            db.save_pair(&FolderPair {
                id: id.clone(),
                name: format!("Pair {id}"),
                left_path: "/a".into(),
                right_path: "/b".into(),
                mode: SyncMode::Echo,
                filters: Filters::default(),
                conflict_policy: ConflictPolicy::NewerWins,
                enabled: true,
                watch_enabled: false,
                schedule_enabled: false,
                schedule_cron: None,
                created_at: 1,
                updated_at: 2,
            })
            .expect("save pair");
        }

        let run_a_old = RunReport {
            run_id: Uuid::new_v4().to_string(),
            pair_id: pair_a.clone(),
            started_at: 10,
            finished_at: Some(20),
            status: RunStatus::Completed,
            files_copied: 1,
            files_deleted: 0,
            bytes_transferred: 5,
            errors: vec![],
        };
        let run_a_new = RunReport {
            run_id: Uuid::new_v4().to_string(),
            pair_id: pair_a.clone(),
            started_at: 30,
            finished_at: Some(40),
            status: RunStatus::Failed,
            files_copied: 0,
            files_deleted: 0,
            bytes_transferred: 0,
            errors: vec!["disk full".into()],
        };
        let run_b = RunReport {
            run_id: Uuid::new_v4().to_string(),
            pair_id: pair_b.clone(),
            started_at: 50,
            finished_at: Some(60),
            status: RunStatus::Completed,
            files_copied: 2,
            files_deleted: 1,
            bytes_transferred: 99,
            errors: vec![],
        };

        for report in [&run_a_old, &run_a_new, &run_b] {
            db.save_run(report).expect("save run");
        }

        let item = RunItem {
            id: Uuid::new_v4().to_string(),
            run_id: run_a_new.run_id.clone(),
            path: "notes.txt".into(),
            action: "copyLeftToRight".into(),
            status: "failed".into(),
            message: Some("disk full".into()),
            bytes: None,
        };
        db.insert_run_item(&item).expect("insert item");

        let all = db.list_runs(None).expect("list all");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].run_id, run_b.run_id);
        assert_eq!(all[1].run_id, run_a_new.run_id);
        assert_eq!(all[2].run_id, run_a_old.run_id);

        let pair_a_runs = db.list_runs(Some(&pair_a)).expect("list pair a");
        assert_eq!(pair_a_runs.len(), 2);
        assert!(pair_a_runs.iter().all(|r| r.pair_id == pair_a));

        let detail = db.get_run(&run_a_new.run_id).expect("get run").expect("run");
        assert_eq!(detail.status, RunStatus::Failed);

        let items = db.list_run_items(&run_a_new.run_id).expect("list items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "notes.txt");
    }

    #[test]
    fn delete_pair_cascades_snapshots_and_runs() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "Cascade".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::Ask,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.save_pair(&pair).expect("save pair");

        let snapshot = Snapshot {
            id: Uuid::new_v4().to_string(),
            pair_id: pair.id.clone(),
            captured_at: 10,
            entries: vec![],
        };
        db.save_snapshot(&snapshot).expect("save snapshot");

        let report = RunReport {
            run_id: Uuid::new_v4().to_string(),
            pair_id: pair.id.clone(),
            started_at: 20,
            finished_at: None,
            status: RunStatus::Running,
            files_copied: 0,
            files_deleted: 0,
            bytes_transferred: 0,
            errors: vec![],
        };
        db.save_run(&report).expect("save run");

        let item = RunItem {
            id: Uuid::new_v4().to_string(),
            run_id: report.run_id.clone(),
            path: "x.txt".into(),
            action: "copyLeftToRight".into(),
            status: "pending".into(),
            message: None,
            bytes: None,
        };
        db.insert_run_item(&item).expect("insert item");

        db.delete_pair(&pair.id).expect("delete pair");

        let snapshot_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("count snapshots");
        let run_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
            .expect("count runs");
        let run_item_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM run_items", [], |row| row.get(0))
            .expect("count run items");

        assert_eq!(snapshot_count, 0);
        assert_eq!(run_count, 0);
        assert_eq!(run_item_count, 0);
    }

    #[test]
    fn duplicate_scan_job_round_trip_and_interruption_recovery() {
        let (_dir, db) = temp_db();
        let job = DuplicateScanJob {
            id: "scan-1".into(),
            root: "/data".into(),
            mode: DuplicateMatchMode::Hash,
            status: DuplicateScanStatus::Running,
            phase: Some(DuplicateScanPhase::Hashing),
            files_found: 12,
            total_files: Some(24),
            hashed_files: 8,
            hash_total: Some(1024),
            bytes_processed: 512,
            bytes_total: Some(2048),
            current_path: Some("nested/file.bin".into()),
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 2,
        };
        db.save_duplicate_scan(&job).expect("save scan");

        let loaded = db.get_duplicate_scan("scan-1").expect("load scan").expect("scan");
        assert_eq!(loaded, job);

        db.mark_duplicate_scans_interrupted().expect("interrupt scan");
        let interrupted =
            db.get_duplicate_scan("scan-1").expect("load interrupted scan").expect("scan");
        assert_eq!(interrupted.status, DuplicateScanStatus::Interrupted);
        assert!(interrupted.error.unwrap_or_default().contains("interrupted"));
    }
}
