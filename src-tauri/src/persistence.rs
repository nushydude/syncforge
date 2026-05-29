use std::path::Path;

use rusqlite::{params, Connection};
use uuid::Uuid;

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

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
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
            self.conn.execute(
                "ALTER TABLE pairs ADD COLUMN schedule_cron TEXT",
                [],
            )?;
        }

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

        self.get_pair(&pair.id)?
            .ok_or_else(|| PersistenceError::PairNotFound(pair.id.clone()))
    }

    pub fn delete_pair(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM pairs WHERE id = ?1", params![id])?;
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
        self.conn.execute(
            "INSERT INTO snapshots (id, pair_id, captured_at, entries_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                pair_id = excluded.pair_id,
                captured_at = excluded.captured_at,
                entries_json = excluded.entries_json",
            params![
                snapshot.id,
                snapshot.pair_id,
                snapshot.captured_at,
                entries_json
            ],
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

    pub fn insert_run_item(&self, item: &RunItem) -> Result<()> {
        self.conn.execute(
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
        Ok(())
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
        other => Err(PersistenceError::Database(rusqlite::Error::InvalidParameterName(
            other.into(),
        ))),
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
        other => Err(PersistenceError::Database(rusqlite::Error::InvalidParameterName(
            other.into(),
        ))),
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
    use crate::models::Filters;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::open(&path).expect("open db");
        (dir, db)
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
}
