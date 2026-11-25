use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static RUN_ITEM_BATCH_INSERTS: Cell<usize> = const { Cell::new(0) };
    static FAIL_NEXT_RUN_ITEM_BATCH: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
pub fn reset_run_item_batch_counter() {
    RUN_ITEM_BATCH_INSERTS.with(|count| count.set(0));
}

#[cfg(test)]
pub fn run_item_batch_insert_count() -> usize {
    RUN_ITEM_BATCH_INSERTS.with(Cell::get)
}

#[cfg(test)]
pub fn fail_next_run_item_batch() {
    FAIL_NEXT_RUN_ITEM_BATCH.with(|fail| fail.set(true));
}

use crate::duplicates::{self, DuplicateScanJob};
use crate::models::{
    ConflictPolicy, FileEntry, FolderPair, RunItem, RunReport, RunStatus, Snapshot, SyncBaseline,
    SyncBaselineView, SyncMode,
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
pub const RUN_ITEMS_PAGE_MAX: usize = 200;

pub struct Database {
    conn: Connection,
}

/// Database access facade used by the application. Writes share one short-lived
/// serialized connection, while reads open independent WAL connections.
pub struct DatabaseManager {
    path: PathBuf,
    writer: Mutex<Database>,
}

/// Small compatibility surface for synchronous engine code and unit tests.
/// Implementations must keep filesystem work and JSON processing outside their
/// own database critical section where possible.
pub trait DatabaseHandle {
    fn load_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
    ) -> Result<SyncBaselineView>;
    #[allow(dead_code)] // Retained for legacy baseline callers and tests.
    fn save_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
    ) -> Result<bool>;
    fn save_sync_baseline_and_run(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
        report: &RunReport,
    ) -> Result<bool>;
    #[allow(dead_code)] // Retained for legacy history consumers.
    fn latest_snapshot(&self, pair_id: &str) -> Result<Option<Snapshot>>;
    fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()>;
    fn save_run(&self, report: &RunReport) -> Result<()>;
    fn insert_run_items(&self, items: &[RunItem]) -> Result<()>;
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Self::open_connection(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn open_connection(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(conn)
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
                    continue_on_error INTEGER NOT NULL DEFAULT 1,
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
                CREATE INDEX IF NOT EXISTS idx_snapshots_pair_captured
                    ON snapshots(pair_id, captured_at DESC);
                CREATE INDEX IF NOT EXISTS idx_runs_pair_id ON runs(pair_id);
                CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);
                CREATE INDEX IF NOT EXISTS idx_runs_pair_started
                    ON runs(pair_id, started_at DESC);
                CREATE INDEX IF NOT EXISTS idx_run_items_run_id ON run_items(run_id);
                CREATE INDEX IF NOT EXISTS idx_run_items_run_path
                    ON run_items(run_id, path COLLATE NOCASE, id);
                ",
            )?;
        }

        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_snapshots_pair_captured
                 ON snapshots(pair_id, captured_at DESC);
             CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);
             CREATE INDEX IF NOT EXISTS idx_runs_pair_started
                 ON runs(pair_id, started_at DESC);
             CREATE INDEX IF NOT EXISTS idx_run_items_run_path
                 ON run_items(run_id, path COLLATE NOCASE, id);
             CREATE INDEX IF NOT EXISTS idx_run_items_run_action_path
                 ON run_items(run_id, action, path COLLATE NOCASE, id);",
        )?;

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_baselines (
            pair_id TEXT PRIMARY KEY NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
            generation TEXT NOT NULL, root_keys_json TEXT, revision TEXT, entries_json TEXT
        );",
        )?;
        self.migrate_watch_enabled()?;
        self.migrate_schedule_fields()?;
        self.migrate_continue_on_error()?;
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

    fn migrate_continue_on_error(&self) -> Result<()> {
        let transaction = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let has_column: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('pairs') WHERE name = 'continue_on_error')",
            [],
            |row| row.get(0),
        )?;
        if !has_column {
            transaction.execute(
                "ALTER TABLE pairs ADD COLUMN continue_on_error INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        transaction.commit()?;
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
                ON duplicate_scans(updated_at DESC);
            CREATE TABLE IF NOT EXISTS duplicate_result_index (
                job_id TEXT NOT NULL, revision TEXT NOT NULL,
                PRIMARY KEY(job_id, revision)
            );
            CREATE TABLE IF NOT EXISTS duplicate_group_index (
                job_id TEXT NOT NULL, revision TEXT NOT NULL, ordinal INTEGER NOT NULL,
                group_id TEXT NOT NULL, key TEXT NOT NULL, file_count INTEGER NOT NULL,
                potential_savings INTEGER NOT NULL, first_size INTEGER NOT NULL,
                PRIMARY KEY(job_id, revision, ordinal),
                UNIQUE(job_id, revision, group_id)
            );
            CREATE TABLE IF NOT EXISTS duplicate_member_index (
                job_id TEXT NOT NULL, revision TEXT NOT NULL, group_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL, file_json TEXT NOT NULL, item_id TEXT,
                PRIMARY KEY(job_id, revision, group_id, ordinal)
            );",
        )?;
        for (column, definition) in [
            ("attempt", "INTEGER NOT NULL DEFAULT 1"),
            ("sequence", "INTEGER NOT NULL DEFAULT 0"),
            ("summary_json", "TEXT"),
        ] {
            let exists: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('duplicate_scans') WHERE name=?1)",
                [column],
                |r| r.get(0),
            )?;
            if !exists {
                self.conn.execute(
                    &format!("ALTER TABLE duplicate_scans ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        let has_item_id: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('duplicate_member_index') WHERE name='item_id')", [], |r| r.get(0))?;
        if !has_item_id {
            self.conn.execute("ALTER TABLE duplicate_member_index ADD COLUMN item_id TEXT", [])?;
        }
        self.conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS duplicate_member_id ON duplicate_member_index(job_id,revision,item_id) WHERE item_id IS NOT NULL", [])?;
        self.conn.execute("CREATE INDEX IF NOT EXISTS idx_duplicate_scans_sequence ON duplicate_scans(sequence DESC)", [])?;
        self.conn.execute_batch("CREATE TABLE IF NOT EXISTS duplicate_sequence (id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL); INSERT OR IGNORE INTO duplicate_sequence(id,value) VALUES(1,0);")?;
        Ok(())
    }

    pub fn list_pairs(&self) -> Result<Vec<FolderPair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, left_path, right_path, mode, filters_json, conflict_policy,
                    enabled, watch_enabled, schedule_enabled, schedule_cron, created_at, updated_at,
                    continue_on_error
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
                row.get::<_, i64>(13)?,
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
                continue_on_error,
            ) = row?;
            row_to_pair(
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
                continue_on_error,
            )
        })
        .collect()
    }

    pub fn last_synced_at_by_pair(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT pair_id, MAX(finished_at)
             FROM runs
             WHERE status = 'completed' AND finished_at IS NOT NULL
             GROUP BY pair_id",
        )?;
        let rows =
            stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        rows.collect::<std::result::Result<HashMap<_, _>, rusqlite::Error>>().map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn save_duplicate_scan(&self, job: &DuplicateScanJob) -> Result<()> {
        let result_json = job.result.as_ref().map(serde_json::to_string).transpose()?;
        self.save_duplicate_scan_json(job, result_json.as_deref()).map(|_| ())
    }

    fn save_duplicate_scan_json(
        &self,
        job: &DuplicateScanJob,
        result_json: Option<&str>,
    ) -> Result<u64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE duplicate_sequence SET value=value+1 WHERE id=1", [])?;
        let sequence: i64 =
            tx.query_row("SELECT value FROM duplicate_sequence WHERE id=1", [], |row| row.get(0))?;
        let changed = tx.execute(
            "INSERT INTO duplicate_scans (
                id, root, mode, status, phase, files_found, total_files,
                hashed_files, hash_total, bytes_processed, bytes_total,
                current_path, cancel_requested, result_json, error, started_at, updated_at, attempt, sequence
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
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
                cancel_requested = CASE WHEN excluded.attempt = duplicate_scans.attempt AND excluded.status = 'running' THEN MAX(duplicate_scans.cancel_requested,excluded.cancel_requested) ELSE excluded.cancel_requested END,
                attempt = excluded.attempt,
                sequence = excluded.sequence,
                summary_json = CASE WHEN excluded.status = 'running' AND excluded.attempt != duplicate_scans.attempt THEN NULL ELSE duplicate_scans.summary_json END,
                result_json = excluded.result_json,
                error = excluded.error,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at
             WHERE (duplicate_scans.status = 'running' AND excluded.attempt = duplicate_scans.attempt)
                OR (duplicate_scans.status != 'running' AND excluded.status = 'running' AND excluded.attempt = duplicate_scans.attempt + 1)",
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
                job.attempt as i64,
                sequence,
            ],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        if let Some(result) = &job.result {
            Self::write_duplicate_index(&tx, &job.id, result)?;
        }
        tx.commit()?;
        Ok(sequence as u64)
    }

    /// Build an additive index exactly once, including for historical JSON rows.
    /// Original JSON is retained for the reviewed cleanup contract and recovery.
    fn index_duplicate_result(
        &self,
        job_id: &str,
        result: &duplicates::DuplicateScanResult,
    ) -> Result<String> {
        let tx = self.conn.unchecked_transaction()?;
        let revision = Self::write_duplicate_index(&tx, job_id, result)?;
        tx.commit()?;
        Ok(revision)
    }

    fn write_duplicate_index(
        tx: &rusqlite::Transaction<'_>,
        job_id: &str,
        result: &duplicates::DuplicateScanResult,
    ) -> Result<String> {
        let revision = result.revision.clone().unwrap_or_else(|| format!("legacy:{job_id}"));
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM duplicate_result_index WHERE job_id=?1 AND revision=?2)",
            params![job_id, revision],
            |r| r.get(0),
        )?;
        if !exists {
            for (ordinal, group) in result.groups.iter().enumerate() {
                let id = group.id.clone().unwrap_or_else(|| format!("legacy-group:{ordinal}"));
                tx.execute("INSERT INTO duplicate_group_index(job_id,revision,ordinal,group_id,key,file_count,potential_savings,first_size) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![job_id,revision,ordinal as i64,id,group.key,group.files.len() as i64,group.potential_savings_bytes as i64,group.files.first().map_or(0, |f| f.size) as i64])?;
                for (member_ordinal, file) in group.files.iter().enumerate() {
                    tx.execute("INSERT INTO duplicate_member_index(job_id,revision,group_id,ordinal,file_json,item_id) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![job_id,revision,id,member_ordinal as i64,serde_json::to_string(file)?,file.id])?;
                }
            }
            tx.execute(
                "INSERT INTO duplicate_result_index(job_id,revision) VALUES(?1,?2)",
                params![job_id, revision],
            )?;
        }
        let summary = duplicates::DuplicateResultSummary::from_result(job_id, result);
        tx.execute(
            "UPDATE duplicate_scans SET summary_json=?1 WHERE id=?2",
            params![serde_json::to_string(&summary)?, job_id],
        )?;
        Ok(revision)
    }

    fn ensure_duplicate_index(&self, job_id: &str) -> Result<Option<String>> {
        let summary: Option<Option<String>> = self
            .conn
            .query_row("SELECT summary_json FROM duplicate_scans WHERE id=?1", [job_id], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(Some(summary)) = summary {
            let parsed: duplicates::DuplicateResultSummary = serde_json::from_str(&summary)?;
            return Ok(Some(parsed.revision));
        }
        let job = match self.get_duplicate_scan(job_id)? {
            Some(job) => job,
            None => return Ok(None),
        };
        let result = match job.result {
            Some(result) => result,
            None => return Ok(None),
        };
        self.index_duplicate_result(job_id, &result).map(Some)
    }

    fn validate_duplicate_revision(&self, job_id: &str, revision: &str) -> Result<()> {
        let row: Option<(String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT status,summary_json FROM duplicate_scans WHERE id=?1",
                [job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (status, summary) = row.ok_or(rusqlite::Error::InvalidQuery)?;
        let summary: duplicates::DuplicateResultSummary =
            serde_json::from_str(&summary.ok_or(rusqlite::Error::InvalidQuery)?)?;
        if status != "completed" || summary.revision != revision {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        Ok(())
    }

    fn duplicate_groups_page(
        &self,
        job_id: &str,
        revision: &str,
        cursor: usize,
        limit: usize,
    ) -> Result<duplicates::DuplicateGroupsPage> {
        self.validate_duplicate_revision(job_id, revision)?;
        let limit = limit.clamp(1, duplicates::DUPLICATE_PAGE_LIMIT);
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM duplicate_result_index WHERE job_id=?1 AND revision=?2)",
            params![job_id, revision],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        let mut stmt = self.conn.prepare("SELECT group_id,key,file_count,potential_savings,first_size FROM duplicate_group_index WHERE job_id=?1 AND revision=?2 AND ordinal>=?3 ORDER BY ordinal LIMIT ?4")?;
        let rows = stmt
            .query_map(params![job_id, revision, cursor as i64, (limit + 1) as i64], |r| {
                Ok(duplicates::DuplicateGroupSummary {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    file_count: r.get::<_, i64>(2)? as u64,
                    potential_savings_bytes: r.get::<_, i64>(3)? as u64,
                    first_size: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = rows.len() > limit;
        Ok(duplicates::DuplicateGroupsPage {
            revision: revision.into(),
            groups: rows.into_iter().take(limit).collect(),
            next_cursor: more.then_some(cursor + limit),
        })
    }

    fn duplicate_member_by_id(
        &self,
        job_id: &str,
        revision: &str,
        item_id: &str,
    ) -> Result<Option<(String, duplicates::DuplicateFile)>> {
        self.validate_duplicate_revision(job_id, revision)?;
        let row: Option<(String,String)> = self.conn.query_row("SELECT group_id,file_json FROM duplicate_member_index WHERE job_id=?1 AND revision=?2 AND item_id=?3", params![job_id,revision,item_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        row.map(|(id, json)| Ok((id, serde_json::from_str(&json)?))).transpose()
    }

    fn duplicate_members_page(
        &self,
        job_id: &str,
        revision: &str,
        group_id: &str,
        cursor: usize,
        limit: usize,
    ) -> Result<duplicates::DuplicateMembersPage> {
        self.validate_duplicate_revision(job_id, revision)?;
        let limit = limit.clamp(1, duplicates::DUPLICATE_PAGE_LIMIT);
        let exists: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM duplicate_group_index WHERE job_id=?1 AND revision=?2 AND group_id=?3)", params![job_id,revision,group_id], |r| r.get(0))?;
        if !exists {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        let mut stmt = self.conn.prepare("SELECT file_json FROM duplicate_member_index WHERE job_id=?1 AND revision=?2 AND group_id=?3 AND ordinal>=?4 ORDER BY ordinal LIMIT ?5")?;
        let json = stmt
            .query_map(
                params![job_id, revision, group_id, cursor as i64, (limit + 1) as i64],
                |r| r.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = json.len() > limit;
        let files = json
            .into_iter()
            .take(limit)
            .map(|v| serde_json::from_str(&v))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(duplicates::DuplicateMembersPage {
            revision: revision.into(),
            group_id: group_id.into(),
            files,
            next_cursor: more.then_some(cursor + limit),
        })
    }

    fn duplicate_snapshot(&self, id: &str) -> Result<Option<duplicates::DuplicateJobSnapshot>> {
        let mut stmt = self.conn.prepare("SELECT id,root,mode,status,phase,files_found,total_files,hashed_files,hash_total,bytes_processed,bytes_total,current_path,cancel_requested,NULL,error,started_at,updated_at,attempt,sequence,summary_json FROM duplicate_scans WHERE id=?1")?;
        let mut rows = stmt.query([id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let job = row_to_duplicate_scan(row)?;
        let summary: Option<String> = row.get(19)?;
        Ok(Some(duplicates::DuplicateJobSnapshot {
            job,
            summary: summary.map(|s| serde_json::from_str(&s)).transpose()?,
        }))
    }
    fn latest_duplicate_id(&self) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT id FROM duplicate_scans ORDER BY sequence DESC,updated_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn interrupt_abandoned_duplicate(&self, id: &str, attempt: u64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE duplicate_sequence SET value=value+1 WHERE id=1", [])?;
        tx.execute("UPDATE duplicate_scans SET status='interrupted',phase=NULL,cancel_requested=0,error='Scan stopped before its results could be saved. Scan again.',sequence=(SELECT value FROM duplicate_sequence WHERE id=1) WHERE id=?1 AND attempt=?2 AND status='running'", params![id,attempt as i64])?;
        tx.commit()?;
        Ok(())
    }

    fn request_duplicate_cancel(&self, id: &str, attempt: u64) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE duplicate_sequence SET value=value+1 WHERE id=1", [])?;
        let changed = tx.execute("UPDATE duplicate_scans SET cancel_requested=1, sequence=(SELECT value FROM duplicate_sequence WHERE id=1) WHERE id=?1 AND attempt=?2 AND status='running' AND cancel_requested=0", params![id,attempt as i64])?;
        tx.commit()?;
        Ok(changed == 1)
    }

    fn save_snapshot_json(&self, snapshot: &Snapshot, entries_json: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO snapshots (id, pair_id, captured_at, entries_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET pair_id = excluded.pair_id,
                captured_at = excluded.captured_at, entries_json = excluded.entries_json",
            params![snapshot.id, snapshot.pair_id, snapshot.captured_at, entries_json],
        )?;
        Self::prune_snapshots_for_pair(&tx, &snapshot.pair_id)?;
        tx.commit()?;
        Ok(())
    }

    fn save_run_json(&self, report: &RunReport, summary_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, pair_id, started_at, finished_at, status, summary_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET finished_at = excluded.finished_at,
                status = excluded.status, summary_json = excluded.summary_json",
            params![
                report.run_id,
                report.pair_id,
                report.started_at,
                report.finished_at,
                run_status_to_str(report.status),
                summary_json
            ],
        )?;
        Ok(())
    }

    pub fn get_duplicate_scan(&self, id: &str) -> Result<Option<DuplicateScanJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root, mode, status, phase, files_found, total_files,
                    hashed_files, hash_total, bytes_processed, bytes_total,
                    current_path, cancel_requested, result_json, error, started_at, updated_at, attempt, sequence
             FROM duplicate_scans WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        rows.next()?.map(row_to_duplicate_scan).transpose()
    }

    pub fn mark_duplicate_scans_interrupted(&self) -> Result<()> {
        self.conn.execute("UPDATE duplicate_scans SET status='interrupted',phase=NULL,cancel_requested=0,error='The previous scan was interrupted. Resume it to continue.' WHERE status='running'", [])?;
        Ok(())
    }

    pub fn save_pair(&self, pair: &FolderPair) -> Result<FolderPair> {
        let filters_json = serde_json::to_string(&pair.filters)?;
        let mode = sync_mode_to_str(pair.mode);
        let conflict_policy = conflict_policy_to_str(pair.conflict_policy);

        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let previous = self.get_pair(&pair.id)?;
        let scope_changed = previous.as_ref().is_none_or(|old| !same_sync_scope(old, pair));
        tx.execute(
            "INSERT INTO pairs (
                id, name, left_path, right_path, mode, filters_json, conflict_policy,
                enabled, watch_enabled, schedule_enabled, schedule_cron, created_at, updated_at,
                continue_on_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                left_path = excluded.left_path,
                right_path = excluded.right_path,
                mode = excluded.mode,
                filters_json = excluded.filters_json,
                conflict_policy = excluded.conflict_policy,
                enabled = excluded.enabled,
                continue_on_error = excluded.continue_on_error,
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
                pair.continue_on_error as i64,
            ],
        )?;

        if scope_changed {
            tx.execute(
                "INSERT INTO sync_baselines (pair_id, generation) VALUES (?1, ?2)
                ON CONFLICT(pair_id) DO UPDATE SET generation = excluded.generation,
                root_keys_json = NULL, revision = NULL, entries_json = NULL",
                params![pair.id, Uuid::new_v4().to_string()],
            )?;
        }
        tx.commit()?;
        self.get_pair(&pair.id)?.ok_or_else(|| PersistenceError::PairNotFound(pair.id.clone()))
    }

    pub fn load_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
    ) -> Result<SyncBaselineView> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if !self.get_pair(&pair.id)?.as_ref().is_some_and(|saved| same_sync_scope(saved, pair)) {
            tx.commit()?;
            return Ok(SyncBaselineView { generation: None, baseline: None });
        }
        let roots = serde_json::to_string(root_keys)?;
        type BaselineRow = (String, Option<String>, Option<String>, Option<String>);
        let row: Option<BaselineRow> = tx.query_row(
            "SELECT generation, root_keys_json, revision, entries_json FROM sync_baselines WHERE pair_id = ?1",
            params![pair.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
        let (generation, revision, entries) = match row {
            Some((generation, Some(old_roots), revision, entries)) if old_roots == roots => {
                (generation, revision, entries)
            }
            other => {
                let generation = match other {
                    Some((generation, None, _, _)) => generation,
                    _ => Uuid::new_v4().to_string(),
                };
                tx.execute("INSERT INTO sync_baselines (pair_id, generation, root_keys_json) VALUES (?1, ?2, ?3)
                    ON CONFLICT(pair_id) DO UPDATE SET generation=excluded.generation, root_keys_json=excluded.root_keys_json, revision=NULL, entries_json=NULL",
                    params![pair.id, generation, roots])?;
                (generation, None, None)
            }
        };
        tx.commit()?;
        let baseline = match (revision, entries) {
            (Some(revision), Some(entries)) => {
                Some(SyncBaseline { revision, entries: serde_json::from_str(&entries)? })
            }
            _ => None,
        };
        Ok(SyncBaselineView { generation: Some(generation), baseline })
    }

    pub fn save_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
    ) -> Result<bool> {
        let roots = serde_json::to_string(root_keys)?;
        let json = serde_json::to_string(entries)?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if !self.get_pair(&pair.id)?.as_ref().is_some_and(|saved| same_sync_scope(saved, pair)) {
            tx.commit()?;
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE sync_baselines SET revision=CASE WHEN entries_json=?2 THEN revision ELSE ?1 END, entries_json=?2
            WHERE pair_id=?3 AND generation=?4 AND root_keys_json=?5 AND revision IS ?6",
            params![
                Uuid::new_v4().to_string(),
                json,
                pair.id,
                expected_generation,
                roots,
                expected_revision
            ],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_sync_baseline_and_run(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
        report: &RunReport,
    ) -> Result<bool> {
        if report.pair_id != pair.id
            || report.status != RunStatus::Completed
            || report.finished_at.is_none()
        {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        let roots = serde_json::to_string(root_keys)?;
        let json = serde_json::to_string(entries)?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if !self.get_pair(&pair.id)?.as_ref().is_some_and(|saved| same_sync_scope(saved, pair)) {
            tx.commit()?;
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE sync_baselines SET revision=CASE WHEN entries_json=?2 THEN revision ELSE ?1 END, entries_json=?2 WHERE pair_id=?3 AND generation=?4 AND root_keys_json=?5 AND revision IS ?6",
            params![Uuid::new_v4().to_string(), json, pair.id, expected_generation, roots, expected_revision],
        )?;
        if changed != 1 {
            tx.commit()?;
            return Ok(false);
        }
        self.save_run(report)?;
        tx.commit()?;
        Ok(true)
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
                    enabled, watch_enabled, schedule_enabled, schedule_cron, created_at, updated_at,
                    continue_on_error
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
                row.get(13)?,
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

    #[allow(dead_code)] // Supports the retained legacy history facade.
    fn latest_snapshot_json(&self, pair_id: &str) -> Result<Option<(String, String, i64, String)>> {
        self.conn
            .query_row(
                "SELECT id, pair_id, captured_at, entries_json FROM snapshots
                 WHERE pair_id = ?1 ORDER BY captured_at DESC LIMIT 1",
                params![pair_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(Into::into)
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

    /// Runs left as `running` after an app/process interruption are not active anymore.
    pub fn mark_sync_runs_interrupted(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE runs
             SET status = 'interrupted',
                 finished_at = COALESCE(finished_at, CAST(strftime('%s','now') AS INTEGER) * 1000),
                 summary_json = json_set(summary_json, '$.status', 'interrupted')
             WHERE status = 'running'",
            [],
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
        #[cfg(test)]
        {
            if FAIL_NEXT_RUN_ITEM_BATCH.with(|fail| fail.replace(false)) {
                return Err(rusqlite::Error::InvalidQuery.into());
            }
            RUN_ITEM_BATCH_INSERTS.with(|count| count.set(count.get() + 1));
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
    #[allow(dead_code)]
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

    fn list_run_json(&self, pair_id: Option<&str>) -> Result<Vec<String>> {
        let mut stmt = match pair_id {
            Some(_) => self.conn.prepare(
                "SELECT summary_json FROM runs WHERE pair_id = ?1
                 ORDER BY started_at DESC LIMIT ?2",
            )?,
            None => self
                .conn
                .prepare("SELECT summary_json FROM runs ORDER BY started_at DESC LIMIT ?1")?,
        };
        let mut values = Vec::new();
        match pair_id {
            Some(id) => {
                for row in stmt.query_map(params![id, HISTORY_RUNS_LIMIT], |row| row.get(0))? {
                    values.push(row?);
                }
            }
            None => {
                for row in stmt.query_map(params![HISTORY_RUNS_LIMIT], |row| row.get(0))? {
                    values.push(row?);
                }
            }
        }
        Ok(values)
    }

    fn get_run_json(&self, run_id: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT summary_json FROM runs WHERE id = ?1", params![run_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunReport>> {
        let mut stmt = self.conn.prepare("SELECT summary_json FROM runs WHERE id = ?1")?;
        let mut rows = stmt.query(params![run_id])?;
        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            return Ok(Some(serde_json::from_str(&json)?));
        }
        Ok(None)
    }

    #[allow(dead_code)]
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

    pub fn list_run_items_page_filtered(
        &self,
        run_id: &str,
        cursor: usize,
        limit: usize,
        action_filter: Option<&str>,
    ) -> Result<(Vec<RunItem>, bool)> {
        let limit = limit.min(RUN_ITEMS_PAGE_MAX);
        if limit == 0 {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        let cursor = i64::try_from(cursor).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let query_limit = i64::try_from(limit + 1).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let query = match action_filter {
            None => {
                "SELECT id, run_id, path, action, status, message, bytes
                     FROM run_items WHERE run_id = ?1
                     ORDER BY path COLLATE NOCASE, id LIMIT ?2 OFFSET ?3"
            }
            Some("deletions") => {
                // Search the two action ranges instead of walking every path in a large run.
                "SELECT id, run_id, path, action, status, message, bytes
                     FROM run_items INDEXED BY idx_run_items_run_action_path
                     WHERE run_id = ?1 AND action IN ('deleteLeft', 'deleteRight')
                     ORDER BY path COLLATE NOCASE, id LIMIT ?2 OFFSET ?3"
            }
            Some(_) => {
                "SELECT id, run_id, path, action, status, message, bytes
                     FROM run_items WHERE run_id = ?1 AND action = ?4
                     ORDER BY path COLLATE NOCASE, id LIMIT ?2 OFFSET ?3"
            }
        };
        let mut stmt = self.conn.prepare(query)?;
        let mut items = Vec::new();
        let mut rows = if let Some(filter) = action_filter.filter(|filter| *filter != "deletions") {
            stmt.query(params![run_id, query_limit, cursor, filter])?
        } else {
            stmt.query(params![run_id, query_limit, cursor])?
        };
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
        let has_more = items.len() > limit;
        items.truncate(limit);
        Ok((items, has_more))
    }
}

impl std::convert::From<std::io::Error> for PersistenceError {
    fn from(value: std::io::Error) -> Self {
        PersistenceError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(value)))
    }
}

impl DatabaseManager {
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.to_path_buf();
        let writer = Database::open(&path)?;
        Ok(Self { path, writer: Mutex::new(writer) })
    }

    fn read<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Database) -> Result<T>,
    {
        let conn = Database::open_connection(&self.path)?;
        let db = Database { conn };
        f(&db)
    }

    fn write<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Database) -> Result<T>,
    {
        let db = self.writer.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        f(&db)
    }

    pub fn list_pairs(&self) -> Result<Vec<FolderPair>> {
        self.read(Database::list_pairs)
    }
    pub fn last_synced_at_by_pair(&self) -> Result<HashMap<String, i64>> {
        self.read(Database::last_synced_at_by_pair)
    }
    pub fn save_pair(&self, pair: &FolderPair) -> Result<FolderPair> {
        self.write(|db| db.save_pair(pair))
    }
    pub fn delete_pair(&self, id: &str) -> Result<()> {
        self.write(|db| db.delete_pair(id))
    }
    pub fn get_pair(&self, id: &str) -> Result<Option<FolderPair>> {
        self.read(|db| db.get_pair(id))
    }
    #[allow(dead_code)] // Retained for legacy history consumers.
    pub fn latest_snapshot(&self, pair_id: &str) -> Result<Option<Snapshot>> {
        let raw = self.read(|db| db.latest_snapshot_json(pair_id))?;
        raw.map(|(id, pair_id, captured_at, entries_json)| {
            Ok(Snapshot { id, pair_id, captured_at, entries: serde_json::from_str(&entries_json)? })
        })
        .transpose()
    }
    pub fn save_duplicate_scan(&self, job: &DuplicateScanJob) -> Result<()> {
        let result_json = job.result.as_ref().map(serde_json::to_string).transpose()?;
        self.write(|db| db.save_duplicate_scan_json(job, result_json.as_deref())).map(|_| ())
    }
    pub fn save_duplicate_terminal(
        &self,
        job: &DuplicateScanJob,
    ) -> Result<duplicates::DuplicateJobSnapshot> {
        let sequence = self.write(|db| db.save_duplicate_scan_json(job, None))?;
        let summary = job
            .result
            .as_ref()
            .map(|r| duplicates::DuplicateResultSummary::from_result(&job.id, r));
        Ok(duplicates::DuplicateJobSnapshot { job: job.compact_copy(sequence), summary })
    }
    pub fn latest_duplicate_snapshot(&self) -> Result<Option<duplicates::DuplicateJobSnapshot>> {
        let Some(id) = self.read(Database::latest_duplicate_id)? else {
            return Ok(None);
        };
        self.write(|db| db.ensure_duplicate_index(&id))?;
        self.read(|db| db.duplicate_snapshot(&id))
    }
    pub fn duplicate_snapshot(&self, id: &str) -> Result<Option<duplicates::DuplicateJobSnapshot>> {
        self.write(|db| db.ensure_duplicate_index(id))?;
        self.read(|db| db.duplicate_snapshot(id))
    }
    pub fn interrupt_abandoned_duplicate(&self, id: &str, attempt: u64) -> Result<()> {
        self.write(|db| db.interrupt_abandoned_duplicate(id, attempt))
    }
    pub fn request_duplicate_cancel(&self, id: &str, attempt: u64) -> Result<bool> {
        self.write(|db| db.request_duplicate_cancel(id, attempt))
    }
    pub fn ensure_duplicate_index(&self, id: &str) -> Result<Option<String>> {
        self.write(|db| db.ensure_duplicate_index(id))
    }
    pub fn duplicate_groups_page(
        &self,
        id: &str,
        revision: &str,
        cursor: usize,
        limit: usize,
    ) -> Result<duplicates::DuplicateGroupsPage> {
        self.read(|db| db.duplicate_groups_page(id, revision, cursor, limit))
    }
    pub fn duplicate_member_by_id(
        &self,
        id: &str,
        revision: &str,
        item: &str,
    ) -> Result<Option<(String, duplicates::DuplicateFile)>> {
        self.read(|db| db.duplicate_member_by_id(id, revision, item))
    }
    pub fn duplicate_members_page(
        &self,
        id: &str,
        revision: &str,
        group: &str,
        cursor: usize,
        limit: usize,
    ) -> Result<duplicates::DuplicateMembersPage> {
        self.read(|db| db.duplicate_members_page(id, revision, group, cursor, limit))
    }
    pub fn get_duplicate_scan(&self, id: &str) -> Result<Option<DuplicateScanJob>> {
        self.read(|db| db.get_duplicate_scan(id))
    }
    pub fn mark_duplicate_scans_interrupted(&self) -> Result<()> {
        self.write(Database::mark_duplicate_scans_interrupted)
    }
    pub fn mark_sync_runs_interrupted(&self) -> Result<()> {
        self.write(Database::mark_sync_runs_interrupted)
    }
    pub fn list_runs(&self, pair_id: Option<&str>) -> Result<Vec<RunReport>> {
        let json = self.read(|db| db.list_run_json(pair_id))?;
        json.into_iter().map(|value| serde_json::from_str(&value).map_err(Into::into)).collect()
    }
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunReport>> {
        self.read(|db| db.get_run_json(run_id))?
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }
    /// Visit one immutable report in browse order without materializing its rows.
    /// The dedicated read connection keeps a single SQLite snapshot and index cursor.
    pub fn visit_run_items(
        &self,
        expected: &RunReport,
        mut visit: impl FnMut(RunItem) -> std::result::Result<(), String>,
    ) -> std::result::Result<(), String> {
        let conn = Database::open_connection(&self.path).map_err(|e| e.to_string())?;
        conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
        let summary: Option<String> = conn
            .query_row(
                "SELECT summary_json FROM runs WHERE id = ?1",
                params![expected.run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let actual: RunReport =
            serde_json::from_str(summary.as_deref().ok_or("Run no longer exists")?)
                .map_err(|e| e.to_string())?;
        if actual != *expected || actual.status == RunStatus::Running {
            return Err("Run changed while preparing export; try again after it finishes".into());
        }
        let mut stmt = conn
            .prepare(
                "SELECT id, run_id, path, action, status, message, bytes
             FROM run_items INDEXED BY idx_run_items_run_path WHERE run_id = ?1
             ORDER BY path COLLATE NOCASE, id",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query(params![expected.run_id]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let bytes: Option<i64> = row.get(6).map_err(|e| e.to_string())?;
            let item = RunItem {
                id: row.get(0).map_err(|e| e.to_string())?,
                run_id: row.get(1).map_err(|e| e.to_string())?,
                path: row.get(2).map_err(|e| e.to_string())?,
                action: row.get(3).map_err(|e| e.to_string())?,
                status: row.get(4).map_err(|e| e.to_string())?,
                message: row.get(5).map_err(|e| e.to_string())?,
                bytes: bytes.map(|b| b as u64),
            };
            visit(item)?;
        }
        Ok(())
    }

    pub fn list_run_items_page(
        &self,
        run_id: &str,
        cursor: usize,
        limit: usize,
    ) -> Result<(Vec<RunItem>, bool)> {
        self.read(|db| db.list_run_items_page_filtered(run_id, cursor, limit, None))
    }
    pub fn list_run_items_page_filtered(
        &self,
        run_id: &str,
        cursor: usize,
        limit: usize,
        action_filter: Option<&str>,
    ) -> Result<(Vec<RunItem>, bool)> {
        self.read(|db| db.list_run_items_page_filtered(run_id, cursor, limit, action_filter))
    }
}

fn same_sync_scope(a: &FolderPair, b: &FolderPair) -> bool {
    a.left_path == b.left_path
        && a.right_path == b.right_path
        && a.mode == b.mode
        && a.filters == b.filters
}

impl DatabaseHandle for Database {
    fn load_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
    ) -> Result<SyncBaselineView> {
        Database::load_sync_baseline(self, pair, root_keys)
    }
    fn save_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
    ) -> Result<bool> {
        Database::save_sync_baseline(
            self,
            pair,
            root_keys,
            expected_generation,
            expected_revision,
            entries,
        )
    }
    fn save_sync_baseline_and_run(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
        report: &RunReport,
    ) -> Result<bool> {
        Database::save_sync_baseline_and_run(
            self,
            pair,
            root_keys,
            expected_generation,
            expected_revision,
            entries,
            report,
        )
    }
    fn latest_snapshot(&self, pair_id: &str) -> Result<Option<Snapshot>> {
        Database::latest_snapshot(self, pair_id)
    }
    fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        Database::save_snapshot(self, snapshot)
    }
    fn save_run(&self, report: &RunReport) -> Result<()> {
        Database::save_run(self, report)
    }
    fn insert_run_items(&self, items: &[RunItem]) -> Result<()> {
        Database::insert_run_items(self, items)
    }
}

impl DatabaseHandle for DatabaseManager {
    fn load_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
    ) -> Result<SyncBaselineView> {
        self.write(|db| db.load_sync_baseline(pair, root_keys))
    }
    fn save_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
    ) -> Result<bool> {
        self.write(|db| {
            db.save_sync_baseline(pair, root_keys, expected_generation, expected_revision, entries)
        })
    }
    fn save_sync_baseline_and_run(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
        report: &RunReport,
    ) -> Result<bool> {
        self.write(|db| {
            db.save_sync_baseline_and_run(
                pair,
                root_keys,
                expected_generation,
                expected_revision,
                entries,
                report,
            )
        })
    }
    fn latest_snapshot(&self, pair_id: &str) -> Result<Option<Snapshot>> {
        self.latest_snapshot(pair_id)
    }
    fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let entries_json = serde_json::to_string(&snapshot.entries)?;
        self.write(|db| db.save_snapshot_json(snapshot, &entries_json))
    }
    fn save_run(&self, report: &RunReport) -> Result<()> {
        let summary_json = serde_json::to_string(report)?;
        self.write(|db| db.save_run_json(report, &summary_json))
    }
    fn insert_run_items(&self, items: &[RunItem]) -> Result<()> {
        self.write(|db| db.insert_run_items(items))
    }
}

#[cfg(test)]
impl DatabaseHandle for Mutex<Database> {
    fn load_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
    ) -> Result<SyncBaselineView> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.load_sync_baseline(pair, root_keys)
    }
    fn save_sync_baseline(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
    ) -> Result<bool> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.save_sync_baseline(
            pair,
            root_keys,
            expected_generation,
            expected_revision,
            entries,
        )
    }
    fn save_sync_baseline_and_run(
        &self,
        pair: &FolderPair,
        root_keys: &[String; 2],
        expected_generation: &str,
        expected_revision: Option<&str>,
        entries: &[(FileEntry, FileEntry)],
        report: &RunReport,
    ) -> Result<bool> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.save_sync_baseline_and_run(
            pair,
            root_keys,
            expected_generation,
            expected_revision,
            entries,
            report,
        )
    }
    fn latest_snapshot(&self, pair_id: &str) -> Result<Option<Snapshot>> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.latest_snapshot(pair_id)
    }
    fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.save_snapshot(snapshot)
    }
    fn save_run(&self, report: &RunReport) -> Result<()> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.save_run(report)
    }
    fn insert_run_items(&self, items: &[RunItem]) -> Result<()> {
        self.lock().map_err(|_| rusqlite::Error::InvalidQuery)?.insert_run_items(items)
    }
}

pub fn new_pair_id() -> String {
    Uuid::new_v4().to_string()
}

#[allow(clippy::too_many_arguments)]
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
    continue_on_error: i64,
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
        continue_on_error: continue_on_error != 0,
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
        attempt: row.get::<_, i64>(17)? as u64,
        sequence: row.get::<_, i64>(18)? as u64,
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
        RunStatus::CompletedWithErrors => "completedWithErrors",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Interrupted => "interrupted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duplicates::{
        DuplicateMatchMode, DuplicateScanJob, DuplicateScanPhase, DuplicateScanStatus,
    };
    use crate::models::Filters;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::open(&path).expect("open db");
        (dir, db)
    }

    #[test]
    fn trusted_baseline_is_scoped_cas_and_preserves_legacy_history() {
        let (_dir, db) = temp_db();
        let mut pair = FolderPair {
            id: new_pair_id(),
            name: "test".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::Ask,
            enabled: true,
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        db.save_pair(&pair).unwrap();
        let entry = FileEntry {
            relative_path: "f".into(),
            size: 1,
            modified_secs: 1,
            modified_nanos: 0,
            is_dir: false,
            hash: Some("hash".into()),
            deleted: false,
        };
        let snapshot = Snapshot {
            id: "legacy".into(),
            pair_id: pair.id.clone(),
            captured_at: 1,
            entries: vec![entry.clone()],
        };
        db.save_snapshot(&snapshot).unwrap();
        // Simulate an existing installation upgraded without a trusted row.
        db.conn.execute("DELETE FROM sync_baselines WHERE pair_id=?1", params![pair.id]).unwrap();
        let roots = ["root-a".into(), "root-b".into()];
        let view = db.load_sync_baseline(&pair, &roots).unwrap();
        assert!(view.baseline.is_none());
        let generation = view.generation.unwrap();
        let entries = vec![(entry.clone(), entry)];
        assert!(db.save_sync_baseline(&pair, &roots, &generation, None, &entries).unwrap());
        assert!(!db.save_sync_baseline(&pair, &roots, &generation, None, &entries).unwrap());
        let revision = db.load_sync_baseline(&pair, &roots).unwrap().baseline.unwrap().revision;
        let mut report = RunReport {
            run_id: "atomic-run".into(),
            pair_id: pair.id.clone(),
            started_at: 1,
            finished_at: None,
            status: RunStatus::Running,
            files_copied: 0,
            files_deleted: 0,
            bytes_transferred: 0,
            errors: vec![],
        };
        db.save_run(&report).unwrap();
        report.status = RunStatus::Completed;
        report.finished_at = Some(2);
        assert!(!db
            .save_sync_baseline_and_run(&pair, &roots, &generation, None, &entries, &report)
            .unwrap());
        assert_eq!(db.get_run("atomic-run").unwrap().unwrap().status, RunStatus::Running);
        assert!(db
            .save_sync_baseline_and_run(
                &pair,
                &roots,
                &generation,
                Some(&revision),
                &entries,
                &report
            )
            .unwrap());
        assert_eq!(
            db.load_sync_baseline(&pair, &roots).unwrap().baseline.unwrap().revision,
            revision
        );
        assert_eq!(db.get_run("atomic-run").unwrap().unwrap().status, RunStatus::Completed);
        db.conn.execute_batch("CREATE TRIGGER reject_terminal BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        let mut changed_entries = entries.clone();
        changed_entries[0].0.modified_secs += 1;
        assert!(db
            .save_sync_baseline_and_run(
                &pair,
                &roots,
                &generation,
                Some(&revision),
                &changed_entries,
                &report
            )
            .is_err());
        assert_eq!(
            db.load_sync_baseline(&pair, &roots).unwrap().baseline.unwrap().revision,
            revision
        );
        db.conn.execute_batch("DROP TRIGGER reject_terminal;").unwrap();
        let mut draft = pair.clone();
        draft.filters.exclude.push("*.tmp".into());
        assert!(db.load_sync_baseline(&draft, &roots).unwrap().generation.is_none());
        assert!(!db
            .save_sync_baseline(&draft, &roots, &generation, Some(&revision), &entries)
            .unwrap());
        pair.filters.exclude.push("*.tmp".into());
        db.save_pair(&pair).unwrap();
        pair.filters.exclude.clear();
        db.save_pair(&pair).unwrap();
        let after = db.load_sync_baseline(&pair, &roots).unwrap();
        assert_ne!(after.generation.as_deref(), Some(generation.as_str()));
        assert!(after.baseline.is_none());
        assert!(!db
            .save_sync_baseline(&pair, &roots, &generation, Some(&revision), &entries)
            .unwrap());
        assert_eq!(db.latest_snapshot(&pair.id).unwrap().unwrap(), snapshot);
        let changed_roots = ["other".into(), "root-b".into()];
        let changed = db.load_sync_baseline(&pair, &changed_roots).unwrap();
        let returned = db.load_sync_baseline(&pair, &roots).unwrap();
        assert_ne!(changed.generation, returned.generation);
    }

    #[test]
    fn action_filter_pages_sparse_matches_in_path_order() {
        let (_dir, db) = temp_db();
        db.conn.execute_batch("PRAGMA foreign_keys = OFF").expect("disable foreign keys");
        for index in 0..450 {
            let action = match index {
                40 => "deleteRight",
                240 => "deleteLeft",
                440 => "deleteRight",
                _ => "copyLeftToRight",
            };
            db.conn
                .execute(
                    "INSERT INTO run_items (id, run_id, path, action, status)
                 VALUES (?1, 'run', ?2, ?3, 'completed')",
                    params![format!("id-{index}"), format!("file-{index:03}"), action],
                )
                .expect("insert item");
        }
        let (first, has_more) =
            db.list_run_items_page_filtered("run", 0, 2, Some("deletions")).expect("first page");
        assert!(has_more);
        assert_eq!(
            first.iter().map(|item| item.path.as_str()).collect::<Vec<_>>(),
            vec!["file-040", "file-240"]
        );
        let (second, has_more) =
            db.list_run_items_page_filtered("run", 2, 2, Some("deletions")).expect("second page");
        assert!(!has_more);
        assert_eq!(second[0].path, "file-440");
        let (right, _) =
            db.list_run_items_page_filtered("run", 0, 10, Some("deleteRight")).expect("right");
        assert_eq!(right.len(), 2);
        let (all, has_more) = db.list_run_items_page_filtered("run", 0, 2, None).expect("all");
        assert!(has_more);
        assert_eq!(all[0].path, "file-000");
        let query_plan: String = db.conn.query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM run_items WHERE run_id = ?1 AND action = ?2 ORDER BY path COLLATE NOCASE, id LIMIT 200",
            params!["run", "deleteRight"], |row| row.get(3),
        ).expect("query plan");
        assert!(query_plan.contains("idx_run_items_run_action_path"), "{query_plan}");
    }

    #[test]
    fn existing_pairs_continue_on_error_by_default_and_can_opt_out() {
        let (dir, db) = temp_db();
        db.conn
            .execute_batch(
                "ALTER TABLE pairs DROP COLUMN continue_on_error;
                 INSERT INTO pairs (id, name, left_path, right_path, mode, filters_json,
                                    conflict_policy, created_at, updated_at)
                 VALUES ('legacy', 'Existing pair', 'left', 'right', 'echo',
                         '{\"include\":[],\"exclude\":[]}', 'left', 1, 2);",
            )
            .expect("legacy pairs table");
        drop(db);

        let path = dir.path().join("test.db");
        let db = Database::open(&path).expect("migrate database");
        let mut pair = db.list_pairs().expect("list migrated pairs").remove(0);
        assert!(pair.continue_on_error);

        pair.continue_on_error = false;
        db.save_pair(&pair).expect("save opt-out");
        drop(db);
        let db = Database::open(&path).expect("reopen database");
        assert!(!db.get_pair(&pair.id).expect("load opt-out").unwrap().continue_on_error);
        assert!(!db.list_pairs().expect("list pairs")[0].continue_on_error);
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
    fn manager_reader_runs_while_writer_connection_is_busy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(DatabaseManager::open(&dir.path().join("test.db")).expect("open"));
        manager
            .save_pair(&FolderPair {
                id: new_pair_id(),
                name: "concurrent".into(),
                left_path: "/a".into(),
                right_path: "/b".into(),
                mode: SyncMode::Echo,
                filters: Filters::default(),
                conflict_policy: ConflictPolicy::NewerWins,
                enabled: true,
                continue_on_error: true,
                watch_enabled: false,
                schedule_enabled: false,
                schedule_cron: None,
                created_at: 1,
                updated_at: 1,
            })
            .expect("seed pair");
        let barrier = Arc::new(Barrier::new(2));
        let writer_manager = Arc::clone(&manager);
        let writer_barrier = Arc::clone(&barrier);
        let writer = thread::spawn(move || {
            let db = writer_manager.writer.lock().expect("writer lock");
            db.conn.execute_batch("BEGIN IMMEDIATE").expect("begin");
            writer_barrier.wait();
            thread::sleep(Duration::from_millis(150));
            db.conn.execute_batch("COMMIT").expect("commit");
        });
        barrier.wait();
        let start = Instant::now();
        assert_eq!(manager.list_pairs().expect("read").len(), 1);
        assert!(start.elapsed() < Duration::from_secs(1));
        writer.join().expect("writer");
    }

    #[test]
    fn paging_queries_use_ordered_indexes() {
        let (_dir, db) = temp_db();
        let pair_id = new_pair_id();
        db.save_pair(&FolderPair {
            id: pair_id.clone(),
            name: "indexed".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        })
        .expect("pair");
        let run_id = new_pair_id();
        db.save_run(&RunReport {
            run_id: run_id.clone(),
            pair_id: pair_id.clone(),
            started_at: 1,
            finished_at: Some(2),
            status: RunStatus::Completed,
            files_copied: 0,
            files_deleted: 0,
            bytes_transferred: 0,
            errors: vec![],
        })
        .expect("run");
        let pair_plan: String = db.conn.query_row(
            "EXPLAIN QUERY PLAN SELECT summary_json FROM runs WHERE pair_id = ?1 ORDER BY started_at DESC LIMIT 100",
            params![pair_id], |row| row.get(3)).expect("history plan");
        assert!(pair_plan.contains("idx_runs_pair_started"), "{pair_plan}");
        let item_plan: String = db.conn.query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM run_items WHERE run_id = ?1 ORDER BY path COLLATE NOCASE, id LIMIT 100",
            params![run_id], |row| row.get(3)).expect("item plan");
        assert!(item_plan.contains("idx_run_items_run_path"), "{item_plan}");
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
            continue_on_error: true,
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
            continue_on_error: true,
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
            continue_on_error: true,
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

        fail_next_run_item_batch();
        let error = db.insert_run_items(&items).expect_err("injected batch failure");
        assert!(matches!(error, PersistenceError::Database(rusqlite::Error::InvalidQuery)));
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
            continue_on_error: true,
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
            continue_on_error: true,
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
            continue_on_error: true,
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
            continue_on_error: true,
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
                deleted: false,
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
            continue_on_error: true,
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
                continue_on_error: true,
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
            continue_on_error: true,
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
            attempt: 1,
            sequence: 0,
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
        assert_eq!(loaded.id, job.id);
        assert!(loaded.sequence > 0);

        db.mark_duplicate_scans_interrupted().expect("interrupt scan");
        let interrupted =
            db.get_duplicate_scan("scan-1").expect("load interrupted scan").expect("scan");
        assert_eq!(interrupted.status, DuplicateScanStatus::Interrupted);
        assert!(interrupted.error.unwrap_or_default().contains("interrupted"));
    }

    #[test]
    fn duplicate_terminal_and_attempt_transitions_reject_stale_writes() {
        let (_dir, db) = temp_db();
        let mut job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "race".into(),
            root: "root".into(),
            mode: DuplicateMatchMode::Size,
            status: DuplicateScanStatus::Running,
            phase: None,
            files_found: 0,
            total_files: None,
            hashed_files: 0,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.save_duplicate_scan(&job).unwrap();
        let stale = db.get_duplicate_scan("race").unwrap().unwrap();
        job.status = DuplicateScanStatus::Completed;
        db.save_duplicate_scan(&job).unwrap();
        let terminal = db.get_duplicate_scan("race").unwrap().unwrap();
        assert!(!db.request_duplicate_cancel("race", 1).unwrap());
        assert!(db.save_duplicate_scan(&stale).is_err());
        assert_eq!(
            db.get_duplicate_scan("race").unwrap().unwrap().status,
            DuplicateScanStatus::Completed
        );
        job.status = DuplicateScanStatus::Running;
        job.attempt = 2;
        db.save_duplicate_scan(&job).unwrap();
        assert!(db.save_duplicate_scan(&stale).is_err());
        let resumed = db.get_duplicate_scan("race").unwrap().unwrap();
        assert!(resumed.sequence > terminal.sequence);
        assert_eq!(resumed.attempt, 2);
        assert!(!db.request_duplicate_cancel("race", 1).unwrap());
        assert!(!db.get_duplicate_scan("race").unwrap().unwrap().cancel_requested);
        assert!(db.request_duplicate_cancel("race", 2).unwrap());
    }

    #[test]
    fn duplicate_cancel_after_barrier_observes_durable_terminal() {
        use std::sync::{mpsc, Arc, Barrier};
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("race.sqlite");
        let db = Database::open(&path).unwrap();
        let job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "barrier".into(),
            root: "root".into(),
            mode: DuplicateMatchMode::Size,
            status: DuplicateScanStatus::Running,
            phase: None,
            files_found: 0,
            total_files: None,
            hashed_files: 0,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.save_duplicate_scan(&job).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = mpsc::channel();
        let path2 = path.clone();
        let barrier2 = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            let db = Database::open(&path2).unwrap();
            let mut terminal = db.get_duplicate_scan("barrier").unwrap().unwrap();
            barrier2.wait();
            terminal.status = DuplicateScanStatus::Completed;
            db.save_duplicate_scan(&terminal).unwrap();
            done_tx.send(()).unwrap();
        });
        barrier.wait();
        done_rx.recv().unwrap();
        assert!(!db.request_duplicate_cancel("barrier", 1).unwrap());
        worker.join().unwrap();
        assert_eq!(
            db.get_duplicate_scan("barrier").unwrap().unwrap().status,
            DuplicateScanStatus::Completed
        );
    }

    #[test]
    fn duplicate_result_pages_are_bounded_and_legacy_identity_stays_absent() {
        let (_dir, db) = temp_db();
        let files = (0..100_001)
            .map(|i| duplicates::DuplicateFile {
                id: None,
                fingerprint: None,
                relative_path: format!("f{i}"),
                name: format!("f{i}"),
                size: 1,
                hash: None,
            })
            .collect::<Vec<_>>();
        let groups = (0..53)
            .map(|i| duplicates::DuplicateGroup {
                id: None,
                key: format!("g{i}"),
                files: if i == 0 { files.clone() } else { files[..2].to_vec() },
                potential_savings_bytes: 122,
            })
            .collect();
        let result = duplicates::DuplicateScanResult {
            revision: None,
            root_identity: None,
            root: "root".into(),
            mode: DuplicateMatchMode::Size,
            scanned_files: 123,
            hashed_files: 0,
            skipped_files: 0,
            potential_savings_bytes: 0,
            groups,
            warnings: vec![],
        };
        let job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "legacy".into(),
            root: "root".into(),
            mode: DuplicateMatchMode::Size,
            status: DuplicateScanStatus::Completed,
            phase: None,
            files_found: 123,
            total_files: None,
            hashed_files: 0,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: Some(result),
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.conn.execute("INSERT INTO duplicate_scans(id,root,mode,status,result_json,started_at,updated_at) VALUES(?1,?2,'size','completed',?3,1,1)", params![job.id,job.root,serde_json::to_string(job.result.as_ref().unwrap()).unwrap()]).unwrap();
        let revision = db.ensure_duplicate_index("legacy").unwrap().unwrap();
        let page = db.duplicate_groups_page("legacy", &revision, 0, 99999).unwrap();
        assert_eq!(page.groups.len(), 50);
        assert_eq!(page.next_cursor, Some(50));
        let second = db.duplicate_groups_page("legacy", &revision, 50, 99999).unwrap();
        assert_eq!(second.groups.len(), 3);
        let members =
            db.duplicate_members_page("legacy", &revision, &page.groups[0].id, 0, 99999).unwrap();
        assert_eq!(members.files.len(), 50);
        assert_eq!(members.next_cursor, Some(50));
        assert!(members.files.iter().all(|file| file.id.is_none()));
        assert!(db.duplicate_members_page("legacy", "old", &page.groups[0].id, 0, 50).is_err());
        let snapshot = db.duplicate_snapshot("legacy").unwrap().unwrap();
        assert!(snapshot.job.result.is_none());
        assert!(serde_json::to_vec(&snapshot).unwrap().len() < 4096);
        assert!(!snapshot.summary.unwrap().cleanup_available);
    }

    #[test]
    fn abandoned_attempt_recovery_is_scoped_and_repeatable() {
        let (_dir, db) = temp_db();
        let mut job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "abandoned".into(),
            root: "root".into(),
            mode: DuplicateMatchMode::Size,
            status: DuplicateScanStatus::Running,
            phase: None,
            files_found: 0,
            total_files: None,
            hashed_files: 0,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.save_duplicate_scan(&job).unwrap();
        assert!(db.request_duplicate_cancel("abandoned", 1).unwrap());
        db.interrupt_abandoned_duplicate("abandoned", 1).unwrap();
        db.interrupt_abandoned_duplicate("abandoned", 1).unwrap();
        let recovered = db.get_duplicate_scan("abandoned").unwrap().unwrap();
        assert_eq!(recovered.status, DuplicateScanStatus::Interrupted);
        assert!(!recovered.cancel_requested);
        assert!(recovered.error.unwrap().contains("before its results could be saved"));
        job.attempt = 2;
        db.save_duplicate_scan(&job).unwrap();
        db.interrupt_abandoned_duplicate("abandoned", 1).unwrap();
        let current = db.get_duplicate_scan("abandoned").unwrap().unwrap();
        assert_eq!(current.status, DuplicateScanStatus::Running);
        assert_eq!(current.attempt, 2);
    }

    #[test]
    fn new_terminal_snapshot_is_indexed_without_full_json_copy() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = DatabaseManager::open(&dir.path().join("db.sqlite")).unwrap();
        let fixtures = tempfile::TempDir::new().unwrap();
        std::fs::write(fixtures.path().join("a"), b"same").unwrap();
        std::fs::write(fixtures.path().join("b"), b"same").unwrap();
        let result =
            duplicates::find_duplicates(fixtures.path(), DuplicateMatchMode::Hash).unwrap();
        let mut job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "compact".into(),
            root: result.root.clone(),
            mode: result.mode,
            status: DuplicateScanStatus::Running,
            phase: None,
            files_found: 2,
            total_files: None,
            hashed_files: 2,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        manager.save_duplicate_scan(&job).unwrap();
        job.status = DuplicateScanStatus::Completed;
        job.result = Some(result);
        let snapshot = manager.save_duplicate_terminal(&job).unwrap();
        let revision = snapshot.summary.unwrap().revision;
        let raw: Option<String> = manager
            .read(|db| {
                db.conn
                    .query_row(
                        "SELECT result_json FROM duplicate_scans WHERE id='compact'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert!(raw.is_none());
        assert_eq!(
            manager.duplicate_groups_page("compact", &revision, 0, 50).unwrap().groups.len(),
            1
        );
    }

    #[test]
    fn terminal_index_failure_rolls_back_row_and_recovers_atomically() {
        let (_dir, db) = temp_db();
        let fixtures = tempfile::TempDir::new().unwrap();
        std::fs::write(fixtures.path().join("a"), b"same").unwrap();
        std::fs::write(fixtures.path().join("b"), b"same").unwrap();
        let result =
            duplicates::find_duplicates(fixtures.path(), DuplicateMatchMode::Hash).unwrap();
        let mut job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "atomic".into(),
            root: result.root.clone(),
            mode: result.mode,
            status: DuplicateScanStatus::Running,
            phase: None,
            files_found: 2,
            total_files: None,
            hashed_files: 2,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.save_duplicate_scan(&job).unwrap();
        db.conn.execute_batch("CREATE TRIGGER reject_index BEFORE INSERT ON duplicate_group_index BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        job.status = DuplicateScanStatus::Completed;
        job.result = Some(result);
        assert!(db.save_duplicate_scan(&job).is_err());
        let persisted = db.get_duplicate_scan("atomic").unwrap().unwrap();
        assert_eq!(persisted.status, DuplicateScanStatus::Running);
        assert!(persisted.result.is_none());
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM duplicate_group_index WHERE job_id='atomic'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        db.conn.execute_batch("DROP TRIGGER reject_index").unwrap();
        db.save_duplicate_scan(&job).unwrap();
        let saved = db.duplicate_snapshot("atomic").unwrap().unwrap();
        assert_eq!(saved.job.status, DuplicateScanStatus::Completed);
        assert!(saved.summary.is_some());
        assert!(!serde_json::to_value(saved).unwrap().as_object().unwrap().contains_key("result"));
    }

    #[test]
    fn indexed_reviewed_item_lookup_is_revision_scoped() {
        let (_db_dir, db) = temp_db();
        let fixtures = tempfile::TempDir::new().unwrap();
        std::fs::write(fixtures.path().join("a"), b"same").unwrap();
        std::fs::write(fixtures.path().join("b"), b"same").unwrap();
        let result =
            duplicates::find_duplicates(fixtures.path(), DuplicateMatchMode::Hash).unwrap();
        let revision = result.revision.clone().unwrap();
        let item = result.groups[0].files[0].clone();
        let item_id = item.id.clone().unwrap();
        let job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "modern".into(),
            root: result.root.clone(),
            mode: result.mode,
            status: DuplicateScanStatus::Completed,
            phase: None,
            files_found: 2,
            total_files: None,
            hashed_files: 2,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: Some(result),
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.save_duplicate_scan(&job).unwrap();
        db.ensure_duplicate_index("modern").unwrap();
        let (_, loaded) =
            db.duplicate_member_by_id("modern", &revision, &item_id).unwrap().unwrap();
        assert_eq!(loaded, item);
        assert!(db.duplicate_member_by_id("modern", "old", &item_id).is_err());
        assert!(db.duplicate_member_by_id("modern", &revision, "unknown").unwrap().is_none());
    }

    #[test]
    fn failed_terminal_write_does_not_create_terminal_state() {
        let (_dir, db) = temp_db();
        let mut job = DuplicateScanJob {
            attempt: 1,
            sequence: 0,
            id: "fail".into(),
            root: "root".into(),
            mode: DuplicateMatchMode::Size,
            status: DuplicateScanStatus::Running,
            phase: None,
            files_found: 0,
            total_files: None,
            hashed_files: 0,
            hash_total: None,
            bytes_processed: 0,
            bytes_total: None,
            current_path: None,
            cancel_requested: false,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
        };
        db.save_duplicate_scan(&job).unwrap();
        db.conn.execute_batch("CREATE TRIGGER fail_terminal BEFORE UPDATE ON duplicate_scans WHEN NEW.status='completed' BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        job.status = DuplicateScanStatus::Completed;
        assert!(db.save_duplicate_scan(&job).is_err());
        assert_eq!(
            db.get_duplicate_scan("fail").unwrap().unwrap().status,
            DuplicateScanStatus::Running
        );
    }

    #[test]
    fn last_synced_at_by_pair_uses_latest_completed_run() {
        let (_dir, db) = temp_db();
        let pair = FolderPair {
            id: new_pair_id(),
            name: "History lookup".into(),
            left_path: "/a".into(),
            right_path: "/b".into(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        db.save_pair(&pair).expect("save pair");

        for (run_id, finished_at, status) in [
            ("completed-old", Some(20), RunStatus::Completed),
            ("completed-new", Some(40), RunStatus::Completed),
            ("failed-newer", Some(60), RunStatus::Failed),
            ("cancelled-newer", Some(80), RunStatus::Cancelled),
        ] {
            db.save_run(&RunReport {
                run_id: run_id.into(),
                pair_id: pair.id.clone(),
                started_at: finished_at.unwrap_or(0) - 1,
                finished_at,
                status,
                files_copied: 0,
                files_deleted: 0,
                bytes_transferred: 0,
                errors: vec![],
            })
            .expect("save run");
        }

        let last_synced = db.last_synced_at_by_pair().expect("last synced lookup");
        assert_eq!(last_synced.get(&pair.id), Some(&40));
    }
}
