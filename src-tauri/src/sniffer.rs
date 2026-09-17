use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::run_coordinator::retry_pending_syncs;
use crate::state::{AppState, HeavyJobKind, WorkCoordinator, WorkRequest};

pub const PROGRESS_EVENT: &str = "syncforge://sniffer-progress";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
const PAGE_MAX: u32 = 200;
const ISSUE_DETAIL_LIMIT: i64 = 10_000;
const ACTION_TTL_MS: i64 = 2 * 60 * 1000;
const RETENTION_MS: i64 = 24 * 60 * 60 * 1000;
const INDEX_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
type ProgressSink<'a> = &'a dyn Fn(&ScanSnapshot);

struct IndexRollback<'a>(&'a Mutex<Connection>);
impl Drop for IndexRollback<'_> {
    fn drop(&mut self) {
        let db = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !db.is_autocommit() {
            let _ = db.execute_batch("ROLLBACK");
        }
        self.0.clear_poison();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ScanStatus {
    Queued,
    Scanning,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSnapshot {
    pub id: String,
    pub generation_id: String,
    pub root: String,
    pub root_node_id: Option<String>,
    pub status: ScanStatus,
    pub revision: u64,
    pub files_visited: String,
    pub folders_visited: String,
    pub logical_bytes: String,
    pub issue_count: String,
    pub coverage_complete: bool,
    pub stale: bool,
    pub current_directory: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<SnifferError>,
    /// The replacement subtree's root, for preserving the refresh location.
    #[serde(default)]
    pub refreshed_directory_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnifferError {
    pub code: String,
    pub operation: String,
    pub retryable: bool,
    pub message: String,
}

impl SnifferError {
    pub fn new(code: &str, operation: &str, retryable: bool, message: impl Into<String>) -> Self {
        Self { code: code.into(), operation: operation.into(), retryable, message: message.into() }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueryRequest {
    pub scan_id: String,
    pub generation_id: String,
    pub directory_id: String,
    pub scope: String,
    pub sort_by: String,
    pub sort_direction: String,
    pub search: String,
    pub extension: Option<String>,
    pub min_size: Option<String>,
    pub max_size: Option<String>,
    pub modified_from: Option<i64>,
    pub modified_to: Option<i64>,
    pub item_kind: Option<String>,
    pub cursor: Option<String>,
    pub limit: u32,
}

impl Default for QueryRequest {
    fn default() -> Self {
        Self {
            scan_id: String::new(),
            generation_id: String::new(),
            directory_id: String::new(),
            scope: "children".into(),
            sort_by: "size".into(),
            sort_direction: "desc".into(),
            search: String::new(),
            extension: None,
            min_size: None,
            max_size: None,
            modified_from: None,
            modified_to: None,
            item_kind: None,
            cursor: None,
            limit: 100,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRow {
    pub node_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub full_path: String,
    pub relative_path: String,
    pub kind: String,
    pub logical_size: String,
    pub files: Option<String>,
    pub folders: Option<String>,
    pub modified_at: Option<i64>,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPage {
    pub rows: Vec<EntryRow>,
    pub next_cursor: Option<String>,
    pub match_count: String,
    pub matched_bytes: String,
    pub directory_bytes: String,
    pub revision: u64,
    pub coverage_complete: bool,
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapTile {
    pub kind: String,
    pub node_id: Option<String>,
    pub name: String,
    pub logical_size: String,
    pub item_count: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub directory: EntryRow,
    pub logical_bytes: String,
    pub files: String,
    pub folders: String,
    pub zero_size_count: String,
    pub tiles: Vec<MapTile>,
    pub revision: u64,
    pub coverage_complete: bool,
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRow {
    pub id: String,
    pub node_id: Option<String>,
    pub category: String,
    pub path: String,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePage {
    pub rows: Vec<IssueRow>,
    pub next_cursor: Option<String>,
    pub total: String,
    pub omitted_details: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareActionRequest {
    pub scan_id: String,
    pub generation_id: String,
    pub node_id: String,
    pub action: String,
    pub new_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReview {
    pub token: String,
    pub action: String,
    pub full_path: String,
    pub kind: String,
    pub new_path: Option<String>,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub action: String,
    pub path: String,
    pub new_path: Option<String>,
    pub stale_directory_id: String,
    pub warning: Option<String>,
}

#[derive(Clone)]
struct JobControl {
    snapshot: ScanSnapshot,
    cancel: Arc<AtomicBool>,
    last_emitted: Instant,
    refresh: Option<(String, i64)>,
}
struct PreparedAction {
    review: ActionReview,
    scan_id: String,
    generation_id: String,
    parent_id: i64,
    native_path: Vec<u8>,
    fingerprint: String,
    destination: Option<PathBuf>,
    root: PathBuf,
}

pub struct SnifferService {
    db_path: PathBuf,
    db: Mutex<Connection>,
    jobs: Mutex<HashMap<String, JobControl>>,
    active: Mutex<Option<String>>,
    actions: Mutex<HashMap<String, PreparedAction>>,
    // Serializes publication and reads across the small SQLite transactions.
    publication: Mutex<()>,
    displayed: Mutex<Option<String>>,
    #[cfg(not(test))]
    event_app: Mutex<Option<AppHandle>>,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
fn sql_error(operation: &str, error: impl std::fmt::Display) -> SnifferError {
    SnifferError::new("internal", operation, true, error.to_string())
}

#[cfg(windows)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}
#[cfg(windows)]
fn bytes_os(value: &[u8]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(
        &value.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>(),
    )
}
#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}
#[cfg(unix)]
fn bytes_os(value: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(value.to_vec())
}
fn path_bytes(path: &Path) -> Vec<u8> {
    os_bytes(path.as_os_str())
}
fn bytes_path(value: &[u8]) -> PathBuf {
    PathBuf::from(bytes_os(value))
}
fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
    metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}
fn fingerprint(path: &Path, metadata: &fs::Metadata) -> String {
    let identity = file_identity(path).unwrap_or_default();
    format!(
        "{}:{}:{}:{}",
        identity,
        if metadata.is_dir() { 0 } else { metadata.len() },
        if metadata.is_dir() { 0 } else { modified_ms(metadata).unwrap_or(-1) },
        if metadata.is_dir() { "d" } else { "f" }
    )
}

#[cfg(windows)]
fn open_identity(
    path: &Path,
    exclusive_delete: bool,
    for_rename: bool,
) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .access_mode(0x80 | if for_rename { 0x10000 } else { 0 })
        .share_mode(if exclusive_delete { 3 } else { 7 })
        .custom_flags(0x0020_0000 | 0x0200_0000)
        .open(path)
}
#[cfg(windows)]
fn handle_identity(file: &fs::File) -> std::io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(format!("{}:{}:{}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}
#[cfg(windows)]
fn file_identity(path: &Path) -> std::io::Result<String> {
    handle_identity(&open_identity(path, false, false)?)
}
#[cfg(unix)]
fn file_identity(path: &Path) -> std::io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path)?;
    Ok(format!("{}:{}", meta.dev(), meta.ino()))
}

fn validate_target(root: &Path, path: &Path, expected: &str) -> Result<(), SnifferError> {
    if path == root || !path.starts_with(root) || expected.starts_with(':') {
        return Err(SnifferError::new(
            "staleTarget",
            "action",
            false,
            "The indexed target identity or root membership cannot be verified.",
        ));
    }
    for ancestor in path.ancestors() {
        let meta = fs::symlink_metadata(ancestor)
            .map_err(|e| SnifferError::new("staleTarget", "action", true, e.to_string()))?;
        if is_link(ancestor, &meta) {
            return Err(SnifferError::new(
                "staleTarget",
                "action",
                false,
                "A target or ancestor is now a link or reparse point. Scan again.",
            ));
        }
    }
    let meta = fs::symlink_metadata(path)
        .map_err(|e| SnifferError::new("staleTarget", "action", true, e.to_string()))?;
    if fingerprint(path, &meta) != expected {
        return Err(SnifferError::new(
            "staleTarget",
            "action",
            true,
            "The item changed since it was indexed. Scan again before reviewing it.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn reparse_tag(path: &Path) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe { FindFirstFileW(wide.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe { FindClose(handle) };
    Some(data.dwReserved0)
}
fn is_link(path: &Path, metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
            && reparse_tag(path).map(|tag| tag & 0x2000_0000 != 0).unwrap_or(metadata.is_dir())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

impl SnifferService {
    pub fn open(cache_dir: &Path) -> Result<Self, rusqlite::Error> {
        fs::create_dir_all(cache_dir)
            .map_err(|_| rusqlite::Error::InvalidPath(cache_dir.into()))?;
        let db_path = cache_dir.join("index.sqlite3");
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS scans(id TEXT PRIMARY KEY,generation_id TEXT NOT NULL,root_display TEXT NOT NULL,root_native BLOB NOT NULL,root_node_id INTEGER,status TEXT NOT NULL,revision INTEGER NOT NULL,files INTEGER NOT NULL DEFAULT 0,folders INTEGER NOT NULL DEFAULT 0,bytes INTEGER NOT NULL DEFAULT 0,issues INTEGER NOT NULL DEFAULT 0,coverage INTEGER NOT NULL DEFAULT 1,stale INTEGER NOT NULL DEFAULT 0,current_directory TEXT,started_at INTEGER NOT NULL,finished_at INTEGER,error TEXT,last_accessed INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS nodes(id INTEGER PRIMARY KEY,scan_id TEXT NOT NULL,parent_id INTEGER,name_display TEXT NOT NULL,path_display TEXT NOT NULL,path_native BLOB NOT NULL,path_key TEXT NOT NULL,depth INTEGER NOT NULL,kind TEXT NOT NULL,size INTEGER NOT NULL DEFAULT 0,files INTEGER NOT NULL DEFAULT 0,folders INTEGER NOT NULL DEFAULT 0,modified_at INTEGER,state TEXT NOT NULL DEFAULT 'complete',fingerprint TEXT,extension TEXT,FOREIGN KEY(scan_id) REFERENCES scans(id) ON DELETE CASCADE);
          CREATE INDEX IF NOT EXISTS nodes_parent ON nodes(scan_id,parent_id); CREATE INDEX IF NOT EXISTS nodes_subtree ON nodes(scan_id,path_key); CREATE INDEX IF NOT EXISTS nodes_size ON nodes(scan_id,size DESC,id); CREATE INDEX IF NOT EXISTS nodes_name ON nodes(scan_id,name_display COLLATE NOCASE,id);
          CREATE TABLE IF NOT EXISTS issues(id INTEGER PRIMARY KEY,scan_id TEXT NOT NULL,node_id INTEGER,category TEXT NOT NULL,path_display TEXT NOT NULL,code TEXT,message TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS issues_scan ON issues(scan_id,category,id);
          CREATE TABLE IF NOT EXISTS pending_directories(scan_id TEXT NOT NULL,node_id INTEGER NOT NULL,path_native BLOB NOT NULL,depth INTEGER NOT NULL,PRIMARY KEY(scan_id,node_id));")?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS pending_order ON pending_directories(scan_id,depth DESC,node_id DESC);")?;
        let columns = conn
            .prepare("PRAGMA table_info(nodes)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "own_incomplete") {
            conn.execute_batch("ALTER TABLE nodes ADD COLUMN own_incomplete INTEGER NOT NULL DEFAULT 0; UPDATE nodes SET own_incomplete=1 WHERE EXISTS(SELECT 1 FROM issues WHERE issues.node_id=nodes.id AND issues.category<>'skippedLink');")?;
        }
        conn.execute("UPDATE scans SET status='failed',finished_at=?1,error='Scan interrupted when SyncForge stopped' WHERE status IN ('queued','scanning','cancelling')", [now_ms()])?;
        conn.execute("DELETE FROM scans WHERE status NOT IN ('queued','scanning','cancelling') AND last_accessed<?1", [now_ms() - RETENTION_MS])?;
        conn.execute("DELETE FROM scans WHERE id IN (SELECT id FROM scans WHERE status NOT IN ('queued','scanning','cancelling') ORDER BY last_accessed DESC LIMIT -1 OFFSET 8)", [])?;
        conn.execute("DELETE FROM issues WHERE scan_id NOT IN (SELECT id FROM scans)", [])?;
        conn.execute(
            "DELETE FROM pending_directories WHERE scan_id NOT IN (SELECT id FROM scans)",
            [],
        )?;
        Ok(Self {
            db_path,
            db: Mutex::new(conn),
            jobs: Mutex::new(HashMap::new()),
            active: Mutex::new(None),
            actions: Mutex::new(HashMap::new()),
            publication: Mutex::new(()),
            displayed: Mutex::new(None),
            #[cfg(not(test))]
            event_app: Mutex::new(None),
        })
    }

    pub fn start(
        self: &Arc<Self>,
        root: &str,
        app: AppHandle,
        state: Arc<AppState>,
    ) -> Result<ScanSnapshot, SnifferError> {
        self.start_native(Path::new(root), app, state, None)
    }

    pub fn refresh(
        self: &Arc<Self>,
        scan: &str,
        generation: &str,
        directory: &str,
        app: AppHandle,
        state: Arc<AppState>,
    ) -> Result<ScanSnapshot, SnifferError> {
        let old = self.validate_generation(scan, generation)?;
        if !matches!(old.status, ScanStatus::Completed | ScanStatus::Cancelled | ScanStatus::Failed)
        {
            return Err(SnifferError::new(
                "busy",
                "refresh",
                true,
                "Wait for the current scan to finish.",
            ));
        }
        let node = self.node(scan, generation, directory)?;
        if node.kind != "directory" {
            return Err(SnifferError::new(
                "unsupported",
                "refresh",
                false,
                "Refresh requires a directory.",
            ));
        }
        if old.issue_count.parse::<i64>().unwrap_or(0) > ISSUE_DETAIL_LIMIT
            && old.root_node_id.as_deref() != Some(directory)
        {
            return Err(SnifferError::new(
                "unsupported",
                "refresh",
                false,
                "Refresh the scan root to rebuild a scan whose issue details were capped.",
            ));
        }
        let path = self.native_node_path(scan, directory)?;
        self.start_native(
            &path,
            app,
            state,
            Some((scan.into(), directory.parse().map_err(|e| sql_error("refresh", e))?)),
        )
    }

    fn start_native(
        self: &Arc<Self>,
        root: &Path,
        app: AppHandle,
        state: Arc<AppState>,
        refresh: Option<(String, i64)>,
    ) -> Result<ScanSnapshot, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("scan", e))?;
        self.set_app(app.clone());
        if refresh.is_none() {
            self.prune(7)?;
        }
        let root = root.canonicalize().map_err(|e| {
            SnifferError::new("notFound", "scan", true, format!("Could not open folder: {e}"))
        })?;
        if !root.is_dir() {
            return Err(SnifferError::new(
                "unsupported",
                "scan",
                false,
                "The selected path is not a folder.",
            ));
        }
        let mut active = self.active.lock().map_err(|e| sql_error("scan", e))?;
        if let Some(id) = active.as_ref() {
            return Err(SnifferError::new(
                "busy",
                "scan",
                true,
                format!("Folder Sniffer scan {id} is already queued or running."),
            ));
        }
        let id = Uuid::new_v4().to_string();
        let generation_id = Uuid::new_v4().to_string();
        let now = now_ms();
        let snapshot = ScanSnapshot {
            id: id.clone(),
            generation_id: generation_id.clone(),
            root: display(&root),
            root_node_id: None,
            status: ScanStatus::Queued,
            revision: 1,
            files_visited: "0".into(),
            folders_visited: "0".into(),
            logical_bytes: "0".into(),
            issue_count: "0".into(),
            coverage_complete: true,
            stale: false,
            current_directory: Some(display(&root)),
            started_at: now,
            finished_at: None,
            error: None,
            refreshed_directory_id: None,
        };
        self.db.lock().map_err(|e| sql_error("scan", e))?.execute("INSERT INTO scans(id,generation_id,root_display,root_native,status,revision,started_at,last_accessed) VALUES(?1,?2,?3,?4,'queued',1,?5,?5)", params![id,generation_id,display(&root),path_bytes(&root),now]).map_err(|e| sql_error("scan",e))?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.lock().map_err(|e| sql_error("scan", e))?.insert(
            id.clone(),
            JobControl {
                snapshot: snapshot.clone(),
                cancel: cancel.clone(),
                last_emitted: Instant::now(),
                refresh,
            },
        );
        *active = Some(id.clone());
        drop(active);
        let service = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || {
            let worker = Arc::clone(&service);
            let worker_id = id.clone();
            let worker_app = app.clone();
            let coordinator = Arc::clone(&state.work_coordinator);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                worker.run_scan(worker_id, root, cancel, worker_app, coordinator)
            }));
            if outcome.is_err() {
                service.finish(
                    &id,
                    ScanStatus::Failed,
                    Some(SnifferError::new(
                        "failed",
                        "scan",
                        true,
                        "The scan worker stopped unexpectedly.",
                    )),
                    &|snapshot| {
                        let _ = app.emit(PROGRESS_EVENT, snapshot);
                    },
                );
            }
            retry_pending_syncs(app, &state);
        });
        Ok(snapshot)
    }

    pub fn pin(&self, scan: Option<String>) -> Result<(), SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("pin", e))?;
        if let Some(id) = &scan {
            if self.get(Some(id))?.is_none() {
                return Err(SnifferError::new("expired", "pin", false, "Scan expired."));
            }
        }
        *self.displayed.lock().map_err(|e| sql_error("pin", e))? = scan;
        self.prune(8)
    }

    pub fn set_app(&self, _app: AppHandle) {
        #[cfg(not(test))]
        if let Ok(mut current) = self.event_app.lock() {
            *current = Some(_app);
        }
    }

    pub fn mark_writes_stale(&self, roots: &[PathBuf]) {
        let Ok(_publication) = self.publication.lock() else {
            return;
        };
        let affected = (|| -> rusqlite::Result<Vec<String>> {
            let db = self.db.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
            let mut statement = db.prepare("SELECT id,root_native FROM scans")?;
            let scans = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut affected = Vec::new();
            for (id, native) in scans {
                let path = bytes_path(&native);
                if roots.iter().any(|root| path.starts_with(root) || root.starts_with(&path)) {
                    db.execute("UPDATE scans SET stale=1,revision=revision+1 WHERE id=?1", [&id])?;
                    affected.push(id);
                }
            }
            Ok(affected)
        })()
        .unwrap_or_default();
        for id in affected {
            self.update_job_quiet(&id, |s| {
                s.stale = true;
                s.revision += 1;
            });
            #[cfg(not(test))]
            if let Ok(Some(snapshot)) = self.get(Some(&id)) {
                if let Ok(app) = self.event_app.lock() {
                    if let Some(app) = app.as_ref() {
                        let _ = app.emit(PROGRESS_EVENT, &snapshot);
                    }
                }
            }
        }
    }

    fn prune(&self, maximum: usize) -> Result<(), SnifferError> {
        let displayed = self.displayed.lock().map_err(|e| sql_error("retention", e))?.clone();
        let mut protected = {
            let mut actions = self.actions.lock().map_err(|e| sql_error("retention", e))?;
            actions.retain(|_, action| action.review.expires_at >= now_ms());
            actions.values().map(|action| action.scan_id.clone()).collect::<Vec<_>>()
        };
        protected.extend(
            self.jobs
                .lock()
                .map_err(|e| sql_error("retention", e))?
                .values()
                .filter(|job| {
                    matches!(
                        job.snapshot.status,
                        ScanStatus::Queued | ScanStatus::Scanning | ScanStatus::Cancelling
                    )
                })
                .filter_map(|job| job.refresh.as_ref().map(|(scan, _)| scan.clone())),
        );
        let removed = {
            let db = self.db.lock().map_err(|e| sql_error("retention", e))?;
            let mut statement = db
                .prepare("SELECT id,status,last_accessed FROM scans ORDER BY last_accessed ASC")
                .map_err(|e| sql_error("retention", e))?;
            let scans = statement
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                })
                .map_err(|e| sql_error("retention", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| sql_error("retention", e))?;
            let mut count = scans.len();
            let mut removed = Vec::new();
            for (id, status, accessed) in scans {
                if matches!(status.as_str(), "queued" | "scanning" | "cancelling")
                    || displayed.as_deref() == Some(&id)
                    || protected.contains(&id)
                {
                    continue;
                }
                if count > maximum || accessed < now_ms() - RETENTION_MS {
                    db.execute("DELETE FROM issues WHERE scan_id=?1", [&id])
                        .map_err(|e| sql_error("retention", e))?;
                    db.execute("DELETE FROM pending_directories WHERE scan_id=?1", [&id])
                        .map_err(|e| sql_error("retention", e))?;
                    db.execute("DELETE FROM scans WHERE id=?1", [&id])
                        .map_err(|e| sql_error("retention", e))?;
                    removed.push(id);
                    count -= 1;
                }
            }
            if !removed.is_empty() {
                let _ =
                    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA incremental_vacuum;");
            }
            removed
        };
        let mut jobs = self.jobs.lock().map_err(|e| sql_error("retention", e))?;
        for id in removed {
            jobs.remove(&id);
        }
        Ok(())
    }

    pub fn get(&self, id: Option<&str>) -> Result<Option<ScanSnapshot>, SnifferError> {
        if let Some(found) = id.and_then(|id| self.jobs.lock().ok()?.get(id).cloned()) {
            return Ok(Some(found.snapshot));
        }
        let db = self.db.lock().map_err(|e| sql_error("getScan", e))?;
        let result = if let Some(id) = id {
            db.query_row("SELECT id,generation_id,root_display,root_node_id,status,revision,files,folders,bytes,issues,coverage,stale,current_directory,started_at,finished_at,error FROM scans WHERE id=?1 ORDER BY started_at DESC LIMIT 1", [id], row_snapshot).optional()
        } else {
            db.query_row("SELECT id,generation_id,root_display,root_node_id,status,revision,files,folders,bytes,issues,coverage,stale,current_directory,started_at,finished_at,error FROM scans ORDER BY started_at DESC LIMIT 1", [], row_snapshot).optional()
        };
        result.map_err(|e| sql_error("getScan", e))
    }

    pub fn get_published(&self, id: Option<&str>) -> Result<Option<ScanSnapshot>, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("getScan", e))?;
        self.get(id)
    }

    pub fn cancel(&self, id: &str, app: &AppHandle) -> Result<ScanSnapshot, SnifferError> {
        let mut jobs = self.jobs.lock().map_err(|e| sql_error("cancel", e))?;
        let job = jobs.get_mut(id).ok_or_else(|| {
            SnifferError::new("notFound", "cancel", false, "The scan is no longer active.")
        })?;
        if matches!(job.snapshot.status, ScanStatus::Queued | ScanStatus::Scanning) {
            job.cancel.store(true, Ordering::Release);
            job.snapshot.status = ScanStatus::Cancelling;
            job.snapshot.revision += 1;
            self.persist_snapshot(&job.snapshot)?;
            let _ = app.emit(PROGRESS_EVENT, &job.snapshot);
        }
        Ok(job.snapshot.clone())
    }

    fn run_scan(
        self: Arc<Self>,
        id: String,
        root: PathBuf,
        cancel: Arc<AtomicBool>,
        app: AppHandle,
        coordinator: Arc<WorkCoordinator>,
    ) {
        let emit = |snapshot: &ScanSnapshot| {
            let _ = app.emit(PROGRESS_EVENT, snapshot);
        };
        let mut permit = None;
        while permit.is_none() && !cancel.load(Ordering::Acquire) {
            permit = match coordinator.try_acquire(WorkRequest::new(
                Vec::new(),
                false,
                HeavyJobKind::Sniffer,
            )) {
                Ok(permit) => permit,
                Err(error) => {
                    self.finish(&id, ScanStatus::Failed, Some(sql_error("scan", error)), &emit);
                    return;
                }
            };
            if permit.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        if cancel.load(Ordering::Acquire) {
            self.finish(&id, ScanStatus::Cancelled, None, &emit);
            return;
        }
        self.update_job(&id, &emit, true, |s| {
            s.status = ScanStatus::Scanning;
        });
        let result = self.traverse(&id, &root, &cancel, &emit).and_then(|()| {
            if !cancel.load(Ordering::Acquire) {
                self.publish_refresh(&id, &cancel)?;
            }
            Ok(())
        });
        drop(permit);
        match result {
            Ok(()) if cancel.load(Ordering::Acquire) => {
                self.finish(&id, ScanStatus::Cancelled, None, &emit)
            }
            Ok(()) => self.finish(&id, ScanStatus::Completed, None, &emit),
            Err(_e) if cancel.load(Ordering::Acquire) => {
                self.finish(&id, ScanStatus::Cancelled, None, &emit)
            }
            Err(e) => self.finish(&id, ScanStatus::Failed, Some(e), &emit),
        }
    }

    fn publish_refresh(&self, scan: &str, cancel: &AtomicBool) -> Result<(), SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("refresh", e))?;
        let _rollback = IndexRollback(&self.db);
        let refresh = self
            .jobs
            .lock()
            .map_err(|e| sql_error("refresh", e))?
            .get(scan)
            .and_then(|job| job.refresh.clone());
        let Some((old_scan, old_directory)) = refresh else {
            return Ok(());
        };
        let old = self.get(Some(&old_scan))?.ok_or_else(|| {
            SnifferError::new("expired", "refresh", false, "Previous scan expired.")
        })?;
        let staged =
            self.get(Some(scan))?.ok_or_else(|| sql_error("refresh", "Missing staged scan"))?;
        let staged_root = staged
            .root_node_id
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| sql_error("refresh", "Missing staged root"))?;
        let db = self.db.lock().map_err(|e| sql_error("refresh", e))?;
        let (old_key, old_parent): (String, Option<i64>) = db
            .query_row(
                "SELECT path_key,parent_id FROM nodes WHERE scan_id=?1 AND id=?2",
                params![old_scan, old_directory],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| sql_error("refresh", e))?;
        let offset: i64 = db
            .query_row("SELECT COALESCE(MAX(id),0) FROM nodes", [], |r| r.get(0))
            .map_err(|e| sql_error("refresh", e))?;
        let old_root = old
            .root_node_id
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| sql_error("refresh", "Missing old root"))?;
        let new_root = if old_parent.is_none() {
            staged_root
        } else {
            offset
                .checked_add(old_root)
                .ok_or_else(|| sql_error("refresh", "Node identity overflow"))?
        };
        let root_native: Vec<u8> = db
            .query_row("SELECT root_native FROM scans WHERE id=?1", [&old_scan], |r| r.get(0))
            .map_err(|e| sql_error("refresh", e))?;
        let staged_retained: i64 = db
            .query_row("SELECT COUNT(*) FROM issues WHERE scan_id=?1", [scan], |r| r.get(0))
            .map_err(|e| sql_error("refresh", e))?;
        db.execute_batch("BEGIN IMMEDIATE").map_err(|e| sql_error("refresh", e))?;
        let result = (|| -> Result<(), SnifferError> {
            db.execute("INSERT INTO nodes(id,scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files,folders,modified_at,state,fingerprint,extension,own_incomplete) SELECT id+?1,?2,CASE WHEN parent_id IS NULL THEN NULL ELSE parent_id+?1 END,name_display,path_display,path_native,path_key,depth,kind,size,files,folders,modified_at,state,fingerprint,extension,own_incomplete FROM nodes WHERE scan_id=?3 AND path_key NOT LIKE ?4",params![offset,scan,old_scan,format!("{old_key}%")]).map_err(|e|sql_error("refresh",e))?;
            db.execute(
                "UPDATE nodes SET parent_id=?2 WHERE id=?1",
                params![staged_root, old_parent.map(|parent| parent + offset)],
            )
            .map_err(|e| sql_error("refresh", e))?;
            db.execute("WITH RECURSIVE tree(id,key,depth) AS (SELECT id,'/',0 FROM nodes WHERE scan_id=?1 AND parent_id IS NULL UNION ALL SELECT n.id,tree.key||n.id||'/',tree.depth+1 FROM nodes n JOIN tree ON n.parent_id=tree.id WHERE n.scan_id=?1) UPDATE nodes SET path_key=(SELECT key FROM tree WHERE tree.id=nodes.id),depth=(SELECT depth FROM tree WHERE tree.id=nodes.id) WHERE scan_id=?1",[scan]).map_err(|e|sql_error("refresh",e))?;
            db.execute("INSERT INTO issues(scan_id,node_id,category,path_display,code,message) SELECT ?1,i.node_id+?2,i.category,i.path_display,i.code,i.message FROM issues i JOIN nodes n ON n.id=i.node_id WHERE i.scan_id=?3 AND n.path_key NOT LIKE ?4",params![scan,offset,old_scan,format!("{old_key}%")]).map_err(|e|sql_error("refresh",e))?;
            db.execute(
                "UPDATE scans SET root_display=?2,root_native=?3,root_node_id=?4 WHERE id=?1",
                params![scan, old.root, root_native, new_root],
            )
            .map_err(|e| sql_error("refresh", e))?;
            if cancel.load(Ordering::Acquire) {
                return Err(SnifferError::new(
                    "cancelled",
                    "refresh",
                    false,
                    "Refresh cancelled before publishing.",
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = db.execute_batch("ROLLBACK");
            return result;
        }
        drop(db);
        if let Err(error) = self.aggregate_ancestors(scan, staged_root) {
            let _ = self.db.lock().map(|db| db.execute_batch("ROLLBACK"));
            return Err(error);
        }
        let db = self.db.lock().map_err(|e| sql_error("refresh", e))?;
        let (bytes, files, folders, root_state): (i64, i64, i64, String) = db
            .query_row("SELECT size,files,folders,state FROM nodes WHERE id=?1", [new_root], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(|e| sql_error("refresh", e))?;
        let merged_retained: i64 = db
            .query_row("SELECT COUNT(*) FROM issues WHERE scan_id=?1", [scan], |r| r.get(0))
            .map_err(|e| sql_error("refresh", e))?;
        let staged_total = staged.issue_count.parse::<i64>().unwrap_or(staged_retained);
        let issues = merged_retained
            .checked_add(staged_total.saturating_sub(staged_retained))
            .ok_or_else(|| sql_error("refresh", "Issue counter overflow"))?;
        db.execute(
            "DELETE FROM issues WHERE scan_id=?1 AND id NOT IN (SELECT id FROM issues WHERE scan_id=?1 ORDER BY id LIMIT ?2)",
            params![scan, ISSUE_DETAIL_LIMIT],
        )
        .map_err(|e| sql_error("refresh", e))?;
        db.execute_batch("COMMIT").map_err(|e| sql_error("refresh", e))?;
        drop(db);
        self.update_job_quiet(scan, |snapshot| {
            snapshot.root = old.root;
            snapshot.root_node_id = Some(new_root.to_string());
            snapshot.refreshed_directory_id = Some(staged_root.to_string());
            snapshot.logical_bytes = bytes.to_string();
            snapshot.files_visited = files.to_string();
            snapshot.folders_visited = folders.to_string();
            snapshot.issue_count = issues.to_string();
            snapshot.coverage_complete = root_state == "complete";
            snapshot.stale |= old.stale && old_parent.is_some();
            snapshot.revision += 1;
        });
        Ok(())
    }

    fn traverse(
        &self,
        scan_id: &str,
        root: &Path,
        cancel: &AtomicBool,
        app: ProgressSink<'_>,
    ) -> Result<(), SnifferError> {
        let root_meta = fs::symlink_metadata(root)
            .map_err(|e| SnifferError::new("permissionDenied", "scan", true, e.to_string()))?;
        let root_id = {
            let db = self.db.lock().map_err(|e| sql_error("scan", e))?;
            db.execute("INSERT INTO nodes(scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,modified_at,fingerprint,state) VALUES(?1,NULL,?2,?3,?4,'/',0,'directory',?5,?6,'scanning')",params![scan_id,root.file_name().unwrap_or(root.as_os_str()).to_string_lossy(),display(root),path_bytes(root),modified_ms(&root_meta),fingerprint(root, &root_meta)]).map_err(|e|sql_error("scan",e))?;
            let node = db.last_insert_rowid();
            db.execute("UPDATE scans SET root_node_id=?2 WHERE id=?1", params![scan_id, node])
                .map_err(|e| sql_error("scan", e))?;
            db.execute(
                "INSERT INTO pending_directories VALUES(?1,?2,?3,0)",
                params![scan_id, node, path_bytes(root)],
            )
            .map_err(|e| sql_error("scan", e))?;
            node
        };
        self.update_job(scan_id, app, true, |s| {
            s.root_node_id = Some(root_id.to_string());
        });
        let mut last = Instant::now() - PROGRESS_INTERVAL;
        loop {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            if self.index_bytes() > INDEX_BUDGET_BYTES {
                self.add_issue(
                    scan_id,
                    None,
                    "resourceLimit",
                    root,
                    &std::io::Error::other("Folder Sniffer's 2 GiB index budget was reached"),
                )?;
                return Err(SnifferError::new(
                    "resourceLimit",
                    "scan",
                    false,
                    "Folder Sniffer's index budget was reached. Committed partial results were retained.",
                ));
            }
            let pending: Option<(i64, Vec<u8>, i64)> = {
                let db = self.db.lock().map_err(|e| sql_error("scan", e))?;
                db.query_row("SELECT node_id,path_native,depth FROM pending_directories WHERE scan_id=?1 ORDER BY depth DESC,node_id DESC LIMIT 1",[scan_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|sql_error("scan",e))?
            };
            let Some((parent_id, native, depth)) = pending else {
                break;
            };
            let dir = bytes_path(&native);
            self.update_job(scan_id, app, last.elapsed() >= PROGRESS_INTERVAL, |s| {
                s.current_directory = Some(display(&dir))
            });
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
            }
            // A queued directory may have been replaced since indexing it.
            let entries = fs::symlink_metadata(&dir).and_then(|metadata| {
                if is_link(&dir, &metadata) {
                    Err(std::io::Error::other(
                        "Directory became a link or reparse point and was not followed",
                    ))
                } else {
                    fs::read_dir(&dir)
                }
            });
            match entries {
                Ok(entries) => {
                    let mut batch = Vec::with_capacity(1000);
                    for child in entries {
                        if cancel.load(Ordering::Acquire) {
                            break;
                        }
                        batch.push(child.map(|entry| entry.path()));
                        if batch.len() >= 1000 || last.elapsed() >= PROGRESS_INTERVAL {
                            self.publish_batch(
                                scan_id,
                                parent_id,
                                depth + 1,
                                &dir,
                                &mut batch,
                                cancel,
                                app,
                            )?;
                            last = Instant::now();
                        }
                    }
                    self.publish_batch(
                        scan_id,
                        parent_id,
                        depth + 1,
                        &dir,
                        &mut batch,
                        cancel,
                        app,
                    )?;
                }
                Err(e) => self.add_issue(
                    scan_id,
                    Some(parent_id),
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        "permissionDenied"
                    } else {
                        "unreadableDirectory"
                    },
                    &dir,
                    &e,
                )?,
            }
            let _publication = self.publication.lock().map_err(|e| sql_error("scan", e))?;
            if !cancel.load(Ordering::Acquire) {
                self.db
                    .lock()
                    .map_err(|e| sql_error("scan", e))?
                    .execute(
                        "DELETE FROM pending_directories WHERE scan_id=?1 AND node_id=?2",
                        params![scan_id, parent_id],
                    )
                    .map_err(|e| sql_error("scan", e))?;
            }
            self.aggregate_ancestors(scan_id, parent_id)?;
            self.update_job_quiet(scan_id, |s| s.revision += 1);
            if let Some(snapshot) = self.get(Some(scan_id))? {
                self.persist_snapshot(&snapshot)?;
            }
            self.emit_progress_due(scan_id, app);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_batch(
        &self,
        scan: &str,
        parent: i64,
        depth: i64,
        dir: &Path,
        batch: &mut Vec<std::io::Result<PathBuf>>,
        cancel: &AtomicBool,
        app: ProgressSink<'_>,
    ) -> Result<(), SnifferError> {
        if batch.is_empty() {
            return Ok(());
        }
        let _publication = self.publication.lock().map_err(|e| sql_error("scan", e))?;
        let _rollback = IndexRollback(&self.db);
        let previous = self.get(Some(scan))?.ok_or_else(|| sql_error("scan", "Missing job"))?;
        self.db
            .lock()
            .map_err(|e| sql_error("scan", e))?
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| sql_error("scan", e))?;
        let result = (|| {
            for child in batch.drain(..) {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                if self.index_bytes() > INDEX_BUDGET_BYTES {
                    return Err(SnifferError::new("resourceLimit", "scan", false, "Folder Sniffer's index budget was reached. Committed partial results were retained."));
                }
                match child {
                    Ok(path) => self.index_child(scan, parent, depth, &path)?,
                    Err(error) => {
                        self.add_issue(scan, Some(parent), "unreadableDirectory", dir, &error)?
                    }
                }
            }
            self.aggregate_ancestors(scan, parent)?;
            if cancel.load(Ordering::Acquire) {
                return Err(SnifferError::new(
                    "cancelled",
                    "scan",
                    false,
                    "Scan cancelled before committing this batch.",
                ));
            }
            // The revision is published with the rows, never ahead of them.
            self.update_job_quiet(scan, |snapshot| snapshot.revision += 1);
            let snapshot = self.get(Some(scan))?.ok_or_else(|| sql_error("scan", "Missing job"))?;
            self.persist_snapshot(&snapshot)?;
            self.db
                .lock()
                .map_err(|e| sql_error("scan", e))?
                .execute_batch("COMMIT")
                .map_err(|e| sql_error("scan", e))?;
            self.emit_progress_due(scan, app);
            Ok(())
        })();
        if result.is_err() {
            if let Ok(db) = self.db.lock() {
                let _ = db.execute_batch("ROLLBACK");
            }
            self.update_job_quiet(scan, |snapshot| {
                let status = snapshot.status.clone();
                let revision = snapshot.revision.max(previous.revision);
                *snapshot = previous;
                snapshot.status = status;
                snapshot.revision = revision;
            });
        }
        result
    }

    fn aggregate_ancestors(&self, scan: &str, mut node: i64) -> Result<(), SnifferError> {
        let db = self.db.lock().map_err(|e| sql_error("scan", e))?;
        loop {
            db.execute("UPDATE nodes AS n SET size=COALESCE((SELECT SUM(c.size) FROM nodes c WHERE c.parent_id=n.id),0),files=COALESCE((SELECT SUM(c.files) FROM nodes c WHERE c.parent_id=n.id),0),folders=COALESCE((SELECT SUM(c.folders+CASE WHEN c.kind='directory' THEN 1 ELSE 0 END) FROM nodes c WHERE c.parent_id=n.id),0),state=CASE WHEN n.own_incomplete=1 OR EXISTS(SELECT 1 FROM nodes c WHERE c.parent_id=n.id AND c.state='unreadable') THEN 'unreadable' WHEN EXISTS(SELECT 1 FROM pending_directories p WHERE p.scan_id=n.scan_id AND p.node_id=n.id) OR EXISTS(SELECT 1 FROM nodes c WHERE c.parent_id=n.id AND c.state='scanning') THEN 'scanning' WHEN n.state='partial' OR EXISTS(SELECT 1 FROM nodes c WHERE c.parent_id=n.id AND c.state='partial') THEN 'partial' ELSE 'complete' END WHERE n.scan_id=?1 AND n.id=?2", params![scan,node]).map_err(|e|sql_error("scan",e))?;
            let parent: Option<i64> = db
                .query_row("SELECT parent_id FROM nodes WHERE id=?1", [node], |r| r.get(0))
                .map_err(|e| sql_error("scan", e))?;
            match parent {
                Some(parent) => node = parent,
                None => break,
            }
        }
        Ok(())
    }

    fn index_bytes(&self) -> u64 {
        let main = fs::metadata(&self.db_path).map(|metadata| metadata.len()).unwrap_or(0);
        let wal = fs::metadata(format!("{}-wal", self.db_path.display()))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        main.saturating_add(wal)
    }

    fn index_child(
        &self,
        scan_id: &str,
        parent: i64,
        depth: i64,
        path: &Path,
    ) -> Result<(), SnifferError> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.add_issue(scan_id, Some(parent), "unreadableMetadata", path, &e)?;
                return Ok(());
            }
        };
        if is_link(path, &meta) {
            self.insert_node(scan_id, parent, depth, path, "link", 0, &meta, "excluded")?;
            self.add_issue(
                scan_id,
                Some(parent),
                "skippedLink",
                path,
                &std::io::Error::other("Filesystem link was not followed"),
            )?;
            return Ok(());
        }
        let kind = if meta.is_dir() {
            "directory"
        } else if meta.is_file() {
            "file"
        } else {
            "unsupported"
        };
        let size = if meta.is_file() { meta.len() } else { 0 };
        let node = self.insert_node(
            scan_id,
            parent,
            depth,
            path,
            kind,
            size,
            &meta,
            if kind == "unsupported" { "excluded" } else { "complete" },
        )?;
        if kind == "directory" {
            self.db
                .lock()
                .map_err(|e| sql_error("scan", e))?
                .execute(
                    "INSERT INTO pending_directories VALUES(?1,?2,?3,?4)",
                    params![scan_id, node, path_bytes(path), depth],
                )
                .map_err(|e| sql_error("scan", e))?;
        }
        if kind == "unsupported" {
            self.add_issue(
                scan_id,
                Some(node),
                "unsupportedType",
                path,
                &std::io::Error::other("Unsupported filesystem entry"),
            )?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn insert_node(
        &self,
        scan_id: &str,
        parent: i64,
        depth: i64,
        path: &Path,
        kind: &str,
        size: u64,
        meta: &fs::Metadata,
        state: &str,
    ) -> Result<i64, SnifferError> {
        let indexed_size = i64::try_from(size).map_err(|_| {
            SnifferError::new(
                "resourceLimit",
                "scan",
                false,
                "An item was too large for the Folder Sniffer index.",
            )
        })?;
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
        let ext = if kind == "file" {
            path.extension().and_then(OsStr::to_str).map(str::to_lowercase)
        } else {
            None
        };
        let db = self.db.lock().map_err(|e| sql_error("scan", e))?;
        let parent_key: String = db
            .query_row("SELECT path_key FROM nodes WHERE id=?1", [parent], |r| r.get(0))
            .map_err(|e| sql_error("scan", e))?;
        db.execute("INSERT INTO nodes(scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files,modified_at,state,fingerprint,extension) VALUES(?1,?2,?3,?4,?5,'',?6,?7,?8,?9,?10,?11,?12,?13)",params![scan_id,parent,name,display(path),path_bytes(path),depth,kind,indexed_size,if kind=="file"{1}else{0},modified_ms(meta),if kind == "directory" { "scanning" } else { state },fingerprint(path, meta),ext]).map_err(|e|sql_error("scan",e))?;
        let id = db.last_insert_rowid();
        db.execute(
            "UPDATE nodes SET path_key=?2 WHERE id=?1",
            params![id, format!("{}{}/", parent_key, id)],
        )
        .map_err(|e| sql_error("scan", e))?;
        drop(db);
        self.update_counters(scan_id, kind, size)?;
        Ok(id)
    }
    fn update_counters(&self, scan_id: &str, kind: &str, size: u64) -> Result<(), SnifferError> {
        let mut jobs = self.jobs.lock().map_err(|e| sql_error("scan", e))?;
        let snapshot =
            &mut jobs.get_mut(scan_id).ok_or_else(|| sql_error("scan", "missing job"))?.snapshot;
        if kind == "file" {
            let files = snapshot.files_visited.parse::<u64>().unwrap_or(0);
            let bytes = snapshot.logical_bytes.parse::<u64>().unwrap_or(0);
            snapshot.files_visited = files
                .checked_add(1)
                .ok_or_else(|| {
                    SnifferError::new("resourceLimit", "scan", false, "File counter overflowed.")
                })?
                .to_string();
            snapshot.logical_bytes = bytes
                .checked_add(size)
                .ok_or_else(|| {
                    SnifferError::new(
                        "resourceLimit",
                        "scan",
                        false,
                        "Logical byte total overflowed.",
                    )
                })?
                .to_string();
        } else if kind == "directory" {
            let folders = snapshot.folders_visited.parse::<u64>().unwrap_or(0);
            snapshot.folders_visited = folders
                .checked_add(1)
                .ok_or_else(|| {
                    SnifferError::new("resourceLimit", "scan", false, "Folder counter overflowed.")
                })?
                .to_string();
        }
        Ok(())
    }
    fn add_issue(
        &self,
        scan_id: &str,
        node: Option<i64>,
        category: &str,
        path: &Path,
        error: &std::io::Error,
    ) -> Result<(), SnifferError> {
        let mut jobs = self.jobs.lock().map_err(|e| sql_error("scan", e))?;
        let job = jobs.get_mut(scan_id).ok_or_else(|| sql_error("scan", "missing job"))?;
        job.snapshot.issue_count = job
            .snapshot
            .issue_count
            .parse::<u64>()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                SnifferError::new("resourceLimit", "scan", false, "Issue counter overflowed.")
            })?
            .to_string();
        if category != "skippedLink" {
            job.snapshot.coverage_complete = false;
        }
        let count = job.snapshot.issue_count.parse::<i64>().unwrap_or(0);
        drop(jobs);
        if category != "skippedLink" {
            self.db
                .lock()
                .map_err(|e| sql_error("scan", e))?
                .execute(
                    "UPDATE nodes SET state='unreadable',own_incomplete=1 WHERE scan_id=?1 AND id=?2",
                    params![scan_id, node],
                )
                .map_err(|e| sql_error("scan", e))?;
        }
        if count <= ISSUE_DETAIL_LIMIT {
            self.db.lock().map_err(|e|sql_error("scan",e))?.execute("INSERT INTO issues(scan_id,node_id,category,path_display,code,message)VALUES(?1,?2,?3,?4,?5,?6)",params![scan_id,node,category,display(path),error.raw_os_error().map(|v|v.to_string()),error.to_string()]).map_err(|e|sql_error("scan",e))?;
        }
        Ok(())
    }

    fn update_job_quiet(&self, id: &str, f: impl FnOnce(&mut ScanSnapshot)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.get_mut(id) {
                f(&mut j.snapshot);
            }
        }
    }
    fn emit_progress_due(&self, id: &str, app: ProgressSink<'_>) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.get_mut(id) {
                if job.last_emitted.elapsed() >= PROGRESS_INTERVAL {
                    job.last_emitted = Instant::now();
                    app(&job.snapshot);
                }
            }
        }
    }
    fn update_job(
        &self,
        id: &str,
        app: ProgressSink<'_>,
        emit: bool,
        f: impl FnOnce(&mut ScanSnapshot),
    ) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.get_mut(id) {
                f(&mut j.snapshot);
                if emit {
                    j.last_emitted = Instant::now();
                    j.snapshot.revision += 1;
                    let _ = self.persist_snapshot(&j.snapshot);
                    app(&j.snapshot);
                }
            }
        }
    }
    fn finish(
        &self,
        id: &str,
        status: ScanStatus,
        error: Option<SnifferError>,
        app: ProgressSink<'_>,
    ) {
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        self.publication.clear_poison();
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            self.jobs.clear_poison();
            if let Some(j) = jobs.get_mut(id) {
                if j.snapshot.status == ScanStatus::Cancelling && status == ScanStatus::Completed {
                    j.snapshot.status = ScanStatus::Cancelled
                } else {
                    j.snapshot.status = status
                }
                j.snapshot.finished_at = Some(now_ms());
                if j.snapshot.status != ScanStatus::Completed {
                    j.snapshot.coverage_complete = false;
                }
                j.snapshot.current_directory = None;
                j.snapshot.error = error;
                j.snapshot.revision += 1;
                let _ = self.persist_snapshot(&j.snapshot);
                app(&j.snapshot);
            }
        }
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
        if let Ok(db) = self.db.lock() {
            let _ = db.execute(
                "UPDATE nodes SET state='partial' WHERE scan_id=?1 AND state='scanning'",
                [id],
            );
            let _ = db.execute("DELETE FROM pending_directories WHERE scan_id=?1", [id]);
        }
    }
    fn persist_snapshot(&self, s: &ScanSnapshot) -> Result<(), SnifferError> {
        self.db.lock().map_err(|e|sql_error("scan",e))?.execute("UPDATE scans SET root_node_id=?2,status=?3,revision=?4,files=?5,folders=?6,bytes=?7,issues=?8,coverage=?9,stale=?10,current_directory=?11,finished_at=?12,error=?13,last_accessed=?14 WHERE id=?1",params![s.id,s.root_node_id.as_deref().and_then(|v|v.parse::<i64>().ok()),status_text(&s.status),s.revision as i64,s.files_visited.parse::<i64>().unwrap_or(i64::MAX),s.folders_visited.parse::<i64>().unwrap_or(i64::MAX),s.logical_bytes.parse::<i64>().unwrap_or(i64::MAX),s.issue_count.parse::<i64>().unwrap_or(i64::MAX),s.coverage_complete,s.stale,s.current_directory,s.finished_at,s.error.as_ref().map(|e|e.message.clone()),now_ms()]).map_err(|e|sql_error("scan",e))?;
        Ok(())
    }

    pub fn query(&self, q: QueryRequest) -> Result<EntryPage, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("query", e))?;
        self.query_published(q)
    }
    fn query_published(&self, q: QueryRequest) -> Result<EntryPage, SnifferError> {
        let snapshot = self.validate_generation(&q.scan_id, &q.generation_id)?;
        let limit = q.limit.clamp(1, PAGE_MAX) as usize;
        let signature = serde_json::to_string(&(
            q.directory_id.clone(),
            q.scope.clone(),
            q.sort_by.clone(),
            q.sort_direction.clone(),
            q.search.clone(),
            q.extension.clone(),
            q.min_size.clone(),
            q.max_size.clone(),
            q.modified_from,
            q.modified_to,
            q.item_kind.clone(),
        ))
        .unwrap_or_default();
        let offset = decode_cursor(q.cursor.as_deref(), &q.scan_id, snapshot.revision, &signature)?;
        let directory = q
            .directory_id
            .parse::<i64>()
            .map_err(|_| SnifferError::new("notFound", "query", false, "Invalid directory."))?;
        let db = self.db.lock().map_err(|e| sql_error("query", e))?;
        db.execute("UPDATE scans SET last_accessed=?2 WHERE id=?1", params![q.scan_id, now_ms()])
            .map_err(|e| sql_error("query", e))?;
        let dir_key: String = db
            .query_row(
                "SELECT path_key FROM nodes WHERE scan_id=?1 AND id=?2 AND kind='directory'",
                params![q.scan_id, directory],
                |r| r.get(0),
            )
            .map_err(|_| {
                SnifferError::new(
                    "notFound",
                    "query",
                    false,
                    "Directory was not found in this scan.",
                )
            })?;
        let mut conditions = vec!["scan_id=?".to_string(), "id<>?".to_string()];
        let mut values = vec![Value::Text(q.scan_id.clone()), Value::Integer(directory)];
        if q.scope == "subtreeFiles" {
            conditions.push("kind='file' AND path_key LIKE ?".into());
            values.push(Value::Text(format!("{dir_key}%")));
        } else {
            conditions.push("parent_id=?".into());
            values.push(Value::Integer(directory));
        }
        if !q.search.is_empty() {
            conditions.push("(name_display LIKE ? ESCAPE '\\' COLLATE NOCASE OR path_display LIKE ? ESCAPE '\\' COLLATE NOCASE)".into());
            let escaped = q.search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            let pattern = Value::Text(format!("%{escaped}%"));
            values.push(pattern.clone());
            values.push(pattern);
        }
        if let Some(extension) = q.extension.as_deref() {
            if extension == "__none__" {
                conditions.push("extension IS NULL".into());
            } else {
                conditions.push("extension=? COLLATE NOCASE".into());
                values.push(Value::Text(extension.trim_start_matches('.').to_lowercase()));
            }
        }
        if let Some(value) = q.min_size.as_deref().and_then(|v| v.parse::<i64>().ok()) {
            conditions.push("size>=?".into());
            values.push(Value::Integer(value));
        }
        if let Some(value) = q.max_size.as_deref().and_then(|v| v.parse::<i64>().ok()) {
            conditions.push("size<=?".into());
            values.push(Value::Integer(value));
        }
        if let Some(value) = q.modified_from {
            conditions.push("modified_at IS NOT NULL AND modified_at>=?".into());
            values.push(Value::Integer(value));
        }
        if let Some(value) = q.modified_to {
            conditions.push("modified_at IS NOT NULL AND modified_at<?".into());
            values.push(Value::Integer(value));
        }
        if let Some(kind) = q.item_kind.as_deref().filter(|kind| *kind != "all") {
            conditions.push("kind=?".into());
            values.push(Value::Text(kind.into()));
        }
        let where_sql = conditions.join(" AND ");
        let (count, bytes): (i64, i64) = db
            .query_row(
                &format!("SELECT COUNT(*),COALESCE(SUM(size),0) FROM nodes WHERE {where_sql}"),
                params_from_iter(values.iter()),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| sql_error("query", e))?;
        let direction = if q.sort_direction == "asc" { "ASC" } else { "DESC" };
        let primary = match q.sort_by.as_str() {
            "name" => format!("name_display COLLATE NOCASE {direction}"),
            "files" => format!("files {direction}"),
            "modified" => format!("modified_at IS NULL ASC,modified_at {direction}"),
            _ => format!("size {direction}"),
        };
        let sql = format!("SELECT id,parent_id,name_display,path_display,kind,size,files,folders,modified_at,state FROM nodes WHERE {where_sql} ORDER BY {primary},name_display COLLATE NOCASE ASC,id ASC LIMIT ? OFFSET ?");
        let mut page_values = values;
        page_values.push(Value::Integer(limit as i64));
        page_values.push(Value::Integer(offset as i64));
        let mut statement = db.prepare(&sql).map_err(|e| sql_error("query", e))?;
        let page = statement
            .query_map(params_from_iter(page_values.iter()), |row| {
                let path: String = row.get(3)?;
                Ok(EntryRow {
                    node_id: row.get::<_, i64>(0)?.to_string(),
                    parent_id: row.get::<_, Option<i64>>(1)?.map(|value| value.to_string()),
                    name: row.get(2)?,
                    relative_path: path
                        .strip_prefix(&snapshot.root)
                        .unwrap_or(&path)
                        .trim_start_matches(['\\', '/'])
                        .to_string(),
                    full_path: path,
                    kind: row.get(4)?,
                    logical_size: row.get::<_, i64>(5)?.max(0).to_string(),
                    files: Some(row.get::<_, i64>(6)?.max(0).to_string()),
                    folders: Some(row.get::<_, i64>(7)?.max(0).to_string()),
                    modified_at: row.get(8)?,
                    status: row.get(9)?,
                })
            })
            .map_err(|e| sql_error("query", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| sql_error("query", e))?;
        let next = (offset + limit < count as usize)
            .then(|| encode_cursor(&q.scan_id, snapshot.revision, &signature, offset + limit));
        let directory_bytes: String = db
            .query_row("SELECT size FROM nodes WHERE id=?1", [directory], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0)
            .to_string();
        Ok(EntryPage {
            rows: page,
            next_cursor: next,
            match_count: count.max(0).to_string(),
            matched_bytes: bytes.to_string(),
            directory_bytes,
            revision: snapshot.revision,
            coverage_complete: snapshot.coverage_complete,
            stale: snapshot.stale,
        })
    }

    pub fn node(
        &self,
        scan_id: &str,
        generation: &str,
        node_id: &str,
    ) -> Result<EntryRow, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("node", e))?;
        self.node_published(scan_id, generation, node_id)
    }
    fn node_published(
        &self,
        scan_id: &str,
        generation: &str,
        node_id: &str,
    ) -> Result<EntryRow, SnifferError> {
        self.validate_generation(scan_id, generation)?;
        let id = node_id
            .parse::<i64>()
            .map_err(|_| SnifferError::new("notFound", "node", false, "Invalid node."))?;
        let db = self.db.lock().map_err(|e| sql_error("node", e))?;
        db.query_row("SELECT id,parent_id,name_display,path_display,kind,size,files,folders,modified_at,state FROM nodes WHERE scan_id=?1 AND id=?2",params![scan_id,id],|r|Ok(EntryRow{node_id:r.get::<_,i64>(0)?.to_string(),parent_id:r.get::<_,Option<i64>>(1)?.map(|v|v.to_string()),name:r.get(2)?,full_path:r.get(3)?,relative_path:String::new(),kind:r.get(4)?,logical_size:r.get::<_,i64>(5)?.max(0).to_string(),files:Some(r.get::<_,i64>(6)?.max(0).to_string()),folders:Some(r.get::<_,i64>(7)?.max(0).to_string()),modified_at:r.get(8)?,status:r.get(9)?})).map_err(|_|SnifferError::new("notFound","node",false,"Node was not found."))
    }
    pub fn summary(
        &self,
        scan_id: &str,
        generation: &str,
        directory: &str,
        request: Option<QueryRequest>,
    ) -> Result<Summary, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("summary", e))?;
        let snapshot = self.validate_generation(scan_id, generation)?;
        let dir = self.node_published(scan_id, generation, directory)?;
        if dir.kind != "directory" {
            return Err(SnifferError::new(
                "unsupported",
                "summary",
                false,
                "Summary requires a directory.",
            ));
        }
        if let Some(mut query) = request {
            query.scan_id = scan_id.into();
            query.generation_id = generation.into();
            query.directory_id = directory.into();
            query.cursor = None;
            query.limit = 40;
            query.sort_by = "size".into();
            query.sort_direction = "desc".into();
            let all = self.query_published(query.clone())?;
            let zero =
                if query.min_size.as_deref().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) > 0 {
                    "0".to_string()
                } else {
                    let mut zero_query = query.clone();
                    zero_query.max_size = Some("0".into());
                    self.query_published(zero_query)?.match_count
                };
            let mut tiles: Vec<MapTile> = all
                .rows
                .iter()
                .filter(|row| row.logical_size != "0")
                .map(|row| MapTile {
                    kind: "entry".into(),
                    node_id: Some(row.node_id.clone()),
                    name: row.name.clone(),
                    logical_size: row.logical_size.clone(),
                    item_count: "1".into(),
                })
                .collect();
            let displayed: u64 =
                tiles.iter().map(|tile| tile.logical_size.parse::<u64>().unwrap_or(0)).sum();
            let omitted = all.matched_bytes.parse::<u64>().unwrap_or(0).saturating_sub(displayed);
            if omitted > 0 {
                tiles.push(MapTile {
                    kind: "other".into(),
                    node_id: None,
                    name: "Other".into(),
                    logical_size: omitted.to_string(),
                    item_count: all
                        .match_count
                        .parse::<u64>()
                        .unwrap_or(0)
                        .saturating_sub(zero.parse::<u64>().unwrap_or(0))
                        .saturating_sub(tiles.len() as u64)
                        .to_string(),
                });
            }
            return Ok(Summary {
                logical_bytes: dir.logical_size.clone(),
                files: dir.files.clone().unwrap_or_default(),
                folders: dir.folders.clone().unwrap_or_default(),
                directory: dir,
                zero_size_count: zero,
                tiles,
                revision: all.revision,
                coverage_complete: all.coverage_complete,
                stale: all.stale,
            });
        }
        let db = self.db.lock().map_err(|e| sql_error("summary", e))?;
        let did = directory.parse::<i64>().unwrap_or(-1);
        let zero: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE scan_id=?1 AND parent_id=?2 AND size=0",
                params![scan_id, did],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let mut stmt=db.prepare("SELECT id,name_display,size FROM nodes WHERE scan_id=?1 AND parent_id=?2 AND size>0 ORDER BY size DESC,name_display COLLATE NOCASE,id LIMIT 41").map_err(|e|sql_error("summary",e))?;
        let raw = stmt
            .query_map(params![scan_id, did], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })
            .map_err(|e| sql_error("summary", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| sql_error("summary", e))?;
        let mut tiles = raw
            .iter()
            .take(40)
            .map(|(id, name, size)| MapTile {
                kind: "entry".into(),
                node_id: Some(id.to_string()),
                name: name.clone(),
                logical_size: size.to_string(),
                item_count: "1".into(),
            })
            .collect::<Vec<_>>();
        if raw.len() > 40 {
            let omitted:i64=db.query_row("SELECT COALESCE(SUM(size),0) FROM nodes WHERE scan_id=?1 AND parent_id=?2 AND size>0 AND id NOT IN (SELECT id FROM nodes WHERE scan_id=?1 AND parent_id=?2 AND size>0 ORDER BY size DESC,name_display COLLATE NOCASE,id LIMIT 40)",params![scan_id,did],|r|r.get(0)).unwrap_or(0);
            let count:i64=db.query_row("SELECT MAX(COUNT(*)-40,0) FROM nodes WHERE scan_id=?1 AND parent_id=?2 AND size>0",params![scan_id,did],|r|r.get(0)).unwrap_or(0);
            tiles.push(MapTile {
                kind: "other".into(),
                node_id: None,
                name: "Other".into(),
                logical_size: omitted.max(0).to_string(),
                item_count: count.to_string(),
            });
        }
        Ok(Summary {
            logical_bytes: dir.logical_size.clone(),
            files: dir.files.clone().unwrap_or_default(),
            folders: dir.folders.clone().unwrap_or_default(),
            directory: dir,
            zero_size_count: zero.to_string(),
            tiles,
            revision: snapshot.revision,
            coverage_complete: snapshot.coverage_complete,
            stale: snapshot.stale,
        })
    }
    pub fn issues(
        &self,
        scan_id: &str,
        category: Option<&str>,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<IssuePage, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("issues", e))?;
        let offset = cursor.and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let limit = limit.clamp(1, PAGE_MAX) as usize;
        let db = self.db.lock().map_err(|e| sql_error("issues", e))?;
        let total: i64 = db
            .query_row("SELECT issues FROM scans WHERE id=?1", [scan_id], |r| r.get(0))
            .map_err(|_| SnifferError::new("expired", "issues", false, "Scan expired."))?;
        let mut stmt=db.prepare("SELECT id,node_id,category,path_display,code,message FROM issues WHERE scan_id=?1 AND (?2 IS NULL OR category=?2) ORDER BY id LIMIT ?3 OFFSET ?4").map_err(|e|sql_error("issues",e))?;
        let rows = stmt
            .query_map(params![scan_id, category, limit as i64, offset as i64], |r| {
                Ok(IssueRow {
                    id: r.get::<_, i64>(0)?.to_string(),
                    node_id: r.get::<_, Option<i64>>(1)?.map(|v| v.to_string()),
                    category: r.get(2)?,
                    path: r.get(3)?,
                    code: r.get(4)?,
                    message: r.get(5)?,
                })
            })
            .map_err(|e| sql_error("issues", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| sql_error("issues", e))?;
        let retained: i64 = db
            .query_row("SELECT COUNT(*) FROM issues WHERE scan_id=?1", [scan_id], |r| r.get(0))
            .unwrap_or(0);
        let matching: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM issues WHERE scan_id=?1 AND (?2 IS NULL OR category=?2)",
                params![scan_id, category],
                |r| r.get(0),
            )
            .map_err(|e| sql_error("issues", e))?;
        let next =
            (offset + rows.len() < matching as usize).then(|| (offset + rows.len()).to_string());
        Ok(IssuePage {
            rows,
            next_cursor: next,
            total: total.to_string(),
            omitted_details: (total - retained).max(0).to_string(),
        })
    }

    fn validate_generation(
        &self,
        scan_id: &str,
        generation: &str,
    ) -> Result<ScanSnapshot, SnifferError> {
        let s = self
            .get(Some(scan_id))?
            .ok_or_else(|| SnifferError::new("expired", "query", false, "Scan expired."))?;
        if s.generation_id != generation {
            return Err(SnifferError::new("expired", "query", false, "Scan generation expired."));
        }
        Ok(s)
    }
    pub fn prepare_action(&self, r: PrepareActionRequest) -> Result<ActionReview, SnifferError> {
        let _publication = self.publication.lock().map_err(|e| sql_error("action", e))?;
        let snapshot = self.validate_generation(&r.scan_id, &r.generation_id)?;
        let id = r
            .node_id
            .parse::<i64>()
            .map_err(|_| SnifferError::new("notFound", "action", false, "Invalid node."))?;
        if snapshot.root_node_id.as_deref() == Some(&r.node_id) {
            return Err(SnifferError::new(
                "unsupported",
                "action",
                false,
                "The scan root cannot be changed.",
            ));
        }
        if r.action != "rename" && r.action != "recycle" {
            return Err(SnifferError::new("unsupported", "action", false, "Unsupported action."));
        }
        if r.action == "recycle" || !cfg!(windows) {
            return Err(SnifferError::new("unsupported", &r.action, false, "This operation is unavailable because the platform cannot guarantee an identity-bound filesystem operation. Use the operating system file manager."));
        }
        let db = self.db.lock().map_err(|e| sql_error("action", e))?;
        let (parent, native, kind, indexed_fingerprint): (i64, Vec<u8>, String, String) = db
            .query_row(
                "SELECT parent_id,path_native,kind,fingerprint FROM nodes WHERE scan_id=?1 AND id=?2",
                params![r.scan_id, id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|_| SnifferError::new("notFound", "action", false, "Node was not found."))?;
        let root_native: Vec<u8> = db
            .query_row("SELECT root_native FROM scans WHERE id=?1", [&r.scan_id], |row| row.get(0))
            .map_err(|e| sql_error("action", e))?;
        drop(db);
        if kind == "link" {
            return Err(SnifferError::new(
                "unsupported",
                "action",
                false,
                "Links and reparse points cannot be changed from Folder Sniffer.",
            ));
        }
        let path = bytes_path(&native);
        let root = bytes_path(&root_native);
        validate_target(&root, &path, &indexed_fingerprint)?;
        let destination = if r.action == "rename" {
            let name = validate_name(r.new_name.as_deref().unwrap_or(""))?;
            Some(
                path.parent()
                    .ok_or_else(|| {
                        SnifferError::new(
                            "unsupported",
                            "rename",
                            false,
                            "Cannot rename this item.",
                        )
                    })?
                    .join(name),
            )
        } else {
            None
        };
        let expires = now_ms() + ACTION_TTL_MS;
        let token = Uuid::new_v4().to_string();
        let review = ActionReview {
            token: token.clone(),
            action: r.action,
            full_path: display(&path),
            kind,
            new_path: destination.as_deref().map(display),
            expires_at: expires,
        };
        let mut actions = self.actions.lock().map_err(|e| sql_error("action", e))?;
        actions.retain(|_, action| action.review.expires_at >= now_ms());
        actions.insert(
            token,
            PreparedAction {
                review: review.clone(),
                scan_id: r.scan_id,
                generation_id: r.generation_id,
                parent_id: parent,
                native_path: native,
                fingerprint: indexed_fingerprint,
                destination,
                root,
            },
        );
        Ok(review)
    }
    pub fn execute_action(
        &self,
        token: &str,
        coordinator: &Arc<WorkCoordinator>,
    ) -> Result<ActionResult, SnifferError> {
        let action =
            self.actions.lock().map_err(|e| sql_error("action", e))?.remove(token).ok_or_else(
                || {
                    SnifferError::new(
                        "expired",
                        "action",
                        false,
                        "Action review expired or was already used.",
                    )
                },
            )?;
        if now_ms() > action.review.expires_at {
            return Err(SnifferError::new("expired", "action", false, "Action review expired."));
        }
        self.validate_generation(&action.scan_id, &action.generation_id)?;
        let path = bytes_path(&action.native_path);
        let _permit = coordinator
            .acquire_manual(WorkRequest::new(
                vec![path.parent().unwrap_or(&path).to_path_buf()],
                true,
                HeavyJobKind::Sniffer,
            ))
            .map_err(|e| sql_error("action", e))?;
        let _publication = self.publication.lock().map_err(|e| sql_error("action", e))?;
        self.validate_generation(&action.scan_id, &action.generation_id)?;
        if now_ms() > action.review.expires_at {
            return Err(SnifferError::new(
                "expired",
                "action",
                false,
                "Action review expired while waiting for other file operations.",
            ));
        }
        let dest = action.destination.as_ref().ok_or_else(|| {
            SnifferError::new("unsupported", "action", false, "No safe destination is available.")
        })?;
        safe_rename(&action.root, &path, dest, &action.fingerprint)?;
        let new_path = Some(display(dest));
        // Filesystem success must never be turned into a retryable mutation failure.
        let warning = match self.db.lock() {
            Ok(db) => db.execute(
                "UPDATE scans SET stale=1,revision=revision+1 WHERE id=?1",
                [&action.scan_id],
            ).err().map(|error|format!("Rename succeeded, but the index could not be marked stale: {error}. Refresh before relying on these results.")),
            Err(error) => Some(format!("Rename succeeded, but the index is unavailable: {error}. Refresh before relying on these results.")),
        };
        self.update_job_quiet(&action.scan_id, |s| {
            s.stale = true;
            s.revision += 1;
        });
        Ok(ActionResult {
            action: action.review.action,
            path: action.review.full_path,
            new_path,
            stale_directory_id: action.parent_id.to_string(),
            warning,
        })
    }
    pub fn native_node_path(&self, scan_id: &str, node_id: &str) -> Result<PathBuf, SnifferError> {
        let id = node_id
            .parse::<i64>()
            .map_err(|_| SnifferError::new("notFound", "properties", false, "Invalid node."))?;
        let native: Vec<u8> = self
            .db
            .lock()
            .map_err(|e| sql_error("properties", e))?
            .query_row(
                "SELECT path_native FROM nodes WHERE scan_id=?1 AND id=?2",
                params![scan_id, id],
                |r| r.get(0),
            )
            .map_err(|_| {
                SnifferError::new("notFound", "properties", false, "Node was not found.")
            })?;
        Ok(bytes_path(&native))
    }
}

fn status_text(s: &ScanStatus) -> &'static str {
    match s {
        ScanStatus::Queued => "queued",
        ScanStatus::Scanning => "scanning",
        ScanStatus::Cancelling => "cancelling",
        ScanStatus::Completed => "completed",
        ScanStatus::Cancelled => "cancelled",
        ScanStatus::Failed => "failed",
    }
}
fn row_snapshot(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScanSnapshot> {
    let status: String = r.get(4)?;
    Ok(ScanSnapshot {
        id: r.get(0)?,
        generation_id: r.get(1)?,
        root: r.get(2)?,
        root_node_id: r.get::<_, Option<i64>>(3)?.map(|v| v.to_string()),
        status: match status.as_str() {
            "queued" => ScanStatus::Queued,
            "scanning" => ScanStatus::Scanning,
            "cancelling" => ScanStatus::Cancelling,
            "completed" => ScanStatus::Completed,
            "cancelled" => ScanStatus::Cancelled,
            _ => ScanStatus::Failed,
        },
        revision: r.get::<_, i64>(5)? as u64,
        files_visited: r.get::<_, i64>(6)?.to_string(),
        folders_visited: r.get::<_, i64>(7)?.to_string(),
        logical_bytes: r.get::<_, i64>(8)?.to_string(),
        issue_count: r.get::<_, i64>(9)?.to_string(),
        coverage_complete: r.get(10)?,
        stale: r.get(11)?,
        current_directory: r.get(12)?,
        started_at: r.get(13)?,
        finished_at: r.get(14)?,
        error: r
            .get::<_, Option<String>>(15)?
            .map(|m| SnifferError::new("failed", "scan", true, m)),
        refreshed_directory_id: None,
    })
}
fn encode_cursor(scan: &str, revision: u64, signature: &str, offset: usize) -> String {
    serde_json::to_string(&(scan, revision, signature, offset)).unwrap_or_default()
}
fn decode_cursor(
    cursor: Option<&str>,
    scan: &str,
    revision: u64,
    signature: &str,
) -> Result<usize, SnifferError> {
    let Some(c) = cursor else { return Ok(0) };
    let (cs, cr, cq, o): (String, u64, String, usize) = serde_json::from_str(c)
        .map_err(|_| SnifferError::new("staleCursor", "query", true, "Invalid result cursor."))?;
    if cs != scan || cr != revision || cq != signature {
        return Err(SnifferError::new(
            "staleCursor",
            "query",
            true,
            "Results changed. Refresh this page.",
        ));
    }
    Ok(o)
}
fn validate_name(name: &str) -> Result<&str, SnifferError> {
    let n = name.trim();
    if n != name
        || n.is_empty()
        || n == "."
        || n == ".."
        || n.contains(['\\', '/', '\0'])
        || n.ends_with(['.', ' '])
    {
        return Err(SnifferError::new(
            "unsupported",
            "rename",
            false,
            "Enter a valid filename without path separators, trailing spaces, or trailing dots.",
        ));
    }
    #[cfg(windows)]
    {
        if n.chars().any(|character| character < ' ' || "<>:\"|?*".contains(character)) {
            return Err(SnifferError::new(
                "unsupported",
                "rename",
                false,
                "That filename contains a character forbidden by Windows.",
            ));
        }
        let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err(SnifferError::new(
                "unsupported",
                "rename",
                false,
                "That name is reserved by Windows.",
            ));
        }
    }
    Ok(n)
}
#[cfg(windows)]
fn safe_rename(
    root: &Path,
    source: &Path,
    dest: &Path,
    expected: &str,
) -> Result<(), SnifferError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO,
    };
    if source.parent() != dest.parent() {
        return Err(SnifferError::new(
            "unsupported",
            "rename",
            false,
            "Rename must remain in the indexed parent.",
        ));
    }
    // Deny deletion/renaming of every ancestor while resolving the target. Opening
    // each component no-follow also prevents a replacement junction redirect.
    let mut ancestors = source.ancestors().skip(1).collect::<Vec<_>>();
    ancestors.reverse();
    let mut locks = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors {
        let handle = open_identity(ancestor, true, false)
            .map_err(|e| SnifferError::new("staleTarget", "rename", true, e.to_string()))?;
        let meta = handle.metadata().map_err(|e| sql_error("rename", e))?;
        if is_link(ancestor, &meta) {
            return Err(SnifferError::new(
                "staleTarget",
                "rename",
                false,
                "An ancestor is a link or reparse point.",
            ));
        }
        locks.push(handle);
    }
    let target = open_identity(source, true, true)
        .map_err(|e| SnifferError::new("staleTarget", "rename", true, e.to_string()))?;
    validate_target(root, source, expected)?;
    // Win32 path conversion requires a trailing NUL even though FileNameLength
    // excludes it. Keep the buffer terminator allocated, including aligned paths.
    let d: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
    let length = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + d.len() * 2;
    let mut storage = vec![0usize; length.div_ceil(std::mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((d.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(d.as_ptr(), (*info).FileName.as_mut_ptr(), d.len());
    }
    if unsafe {
        SetFileInformationByHandle(
            target.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            length as u32,
        )
    } == 0
    {
        let e = std::io::Error::last_os_error();
        return Err(SnifferError::new(
            if e.kind() == std::io::ErrorKind::AlreadyExists { "nameCollision" } else { "failed" },
            "rename",
            true,
            e.to_string(),
        ));
    }
    Ok(())
}
#[cfg(not(windows))]
fn safe_rename(
    _root: &Path,
    _source: &Path,
    _dest: &Path,
    _expected: &str,
) -> Result<(), SnifferError> {
    Err(SnifferError::new(
        "unsupported",
        "rename",
        false,
        "Identity-bound no-replace rename is unavailable on this platform.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, SnifferService, ScanSnapshot) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        let service = SnifferService::open(&dir.path().join("cache")).unwrap();
        let snapshot = ScanSnapshot {
            id: "scan".into(),
            generation_id: "generation".into(),
            root: display(&root),
            root_node_id: Some("1".into()),
            status: ScanStatus::Completed,
            revision: 7,
            files_visited: "250".into(),
            folders_visited: "1".into(),
            logical_bytes: "31375".into(),
            issue_count: "0".into(),
            coverage_complete: true,
            stale: false,
            current_directory: None,
            started_at: now_ms(),
            finished_at: Some(now_ms()),
            error: None,
            refreshed_directory_id: None,
        };
        let db = service.db.lock().unwrap();
        db.execute("INSERT INTO scans(id,generation_id,root_display,root_native,root_node_id,status,revision,files,folders,bytes,issues,coverage,stale,started_at,last_accessed) VALUES('scan','generation',?1,?2,1,'completed',7,250,1,31375,0,1,0,?3,?3)", params![display(&root), path_bytes(&root), now_ms()]).unwrap();
        db.execute("INSERT INTO nodes(id,scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files,folders,state) VALUES(1,'scan',NULL,'root',?1,?2,'/',0,'directory',31375,250,1,'complete')", params![display(&root), path_bytes(&root)]).unwrap();
        db.execute("INSERT INTO nodes(id,scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files,folders,state) VALUES(2,'scan',1,'nested',?1,?2,'/2/',1,'directory',31375,250,0,'complete')", params![display(&root.join("nested")), path_bytes(&root.join("nested"))]).unwrap();
        for index in 0..250_i64 {
            let name =
                if index == 42 { "100%real.txt".into() } else { format!("file-{index}.txt") };
            let path = root.join("nested").join(&name);
            db.execute("INSERT INTO nodes(scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files,folders,state,extension) VALUES('scan',2,?1,?2,?3,?4,2,'file',?5,1,0,'complete','txt')", params![name,display(&path),path_bytes(&path),format!("/2/{}/",index+3),index+1]).unwrap();
        }
        drop(db);
        service.jobs.lock().unwrap().insert(
            "scan".into(),
            JobControl {
                snapshot: snapshot.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
                last_emitted: Instant::now(),
                refresh: None,
            },
        );
        (dir, service, snapshot)
    }

    #[test]
    fn indexed_queries_include_deep_files_and_cap_pages() {
        let (_dir, service, snapshot) = fixture();
        assert_eq!(validate_name("CON").is_err(), cfg!(windows));
        assert!(validate_name("bad/").is_err());
        let request = QueryRequest {
            scan_id: snapshot.id,
            generation_id: snapshot.generation_id,
            directory_id: "1".into(),
            scope: "subtreeFiles".into(),
            limit: 500,
            ..Default::default()
        };
        let first = service.query(request.clone()).unwrap();
        assert_eq!(first.rows.len(), 200);
        assert_eq!(first.rows[0].logical_size, "250");
        assert!(first.next_cursor.is_some());
        let second =
            service.query(QueryRequest { cursor: first.next_cursor, ..request.clone() }).unwrap();
        assert_eq!(second.rows.len(), 50);
        let literal =
            service.query(QueryRequest { search: "%".into(), cursor: None, ..request }).unwrap();
        assert_eq!(literal.rows.len(), 1);
        assert_eq!(literal.rows[0].name, "100%real.txt");
    }

    #[test]
    fn filtered_summary_matches_query_and_bounds_tiles() {
        let (_dir, service, snapshot) = fixture();
        let query = QueryRequest {
            scan_id: snapshot.id.clone(),
            generation_id: snapshot.generation_id.clone(),
            directory_id: "1".into(),
            scope: "subtreeFiles".into(),
            min_size: Some("101".into()),
            ..Default::default()
        };
        let page = service.query(query.clone()).unwrap();
        let summary =
            service.summary(&snapshot.id, &snapshot.generation_id, "1", Some(query)).unwrap();
        assert_eq!(summary.tiles.len(), 41);
        assert_eq!(summary.tiles.last().unwrap().kind, "other");
        let bytes: u64 =
            summary.tiles.iter().map(|tile| tile.logical_size.parse::<u64>().unwrap()).sum();
        assert_eq!(bytes.to_string(), page.matched_bytes);
        assert_eq!(summary.zero_size_count, "0");
    }

    #[test]
    fn category_issue_cursor_terminates_at_filtered_count() {
        let (_dir, service, _) = fixture();
        {
            let db = service.db.lock().unwrap();
            for category in ["permissionDenied", "skippedLink", "skippedLink"] {
                db.execute("INSERT INTO issues(scan_id,category,path_display,message) VALUES('scan',?1,'path','message')",[category]).unwrap();
            }
            db.execute("UPDATE scans SET issues=3 WHERE id='scan'", []).unwrap();
        }
        let page = service.issues("scan", Some("permissionDenied"), None, 1).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert!(page.next_cursor.is_none());
        let missing = service.issues("scan", Some("vanished"), None, 1).unwrap();
        assert!(missing.rows.is_empty());
        assert!(missing.next_cursor.is_none());
    }

    #[test]
    fn retention_protects_displayed_and_reviewed_scans() {
        let (_dir, service, _) = fixture();
        {
            let db = service.db.lock().unwrap();
            for index in 0..12 {
                db.execute("INSERT INTO scans(id,generation_id,root_display,root_native,status,revision,started_at,last_accessed) VALUES(?1,'generation','root',X'','completed',1,?2,?2)",params![format!("old-{index}"),now_ms()-100+index]).unwrap();
            }
        }
        service.pin(Some("old-0".into())).unwrap();
        assert!(service.get(Some("old-0")).unwrap().is_some());
        let count: i64 = service
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM scans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 8);
    }

    #[test]
    fn aggregate_partial_children_without_claiming_empty_complete_directory() {
        let (_dir, service, _) = fixture();
        {
            let db = service.db.lock().unwrap();
            db.execute("UPDATE nodes SET state='scanning' WHERE id=2", []).unwrap();
            db.execute("INSERT INTO pending_directories VALUES('scan',2,X'',1)", []).unwrap();
        }
        service.aggregate_ancestors("scan", 2).unwrap();
        let root = service.node("scan", "generation", "1").unwrap();
        assert_eq!(root.logical_size, "31375");
        assert_eq!(root.status, "scanning");
        service.db.lock().unwrap().execute("DELETE FROM pending_directories", []).unwrap();
        service.aggregate_ancestors("scan", 2).unwrap();
        assert_eq!(service.node("scan", "generation", "1").unwrap().status, "complete");
    }

    #[test]
    fn overlapping_writes_invalidate_scan_and_old_cursors() {
        let (_dir, service, snapshot) = fixture();
        let query = QueryRequest {
            scan_id: "scan".into(),
            generation_id: "generation".into(),
            directory_id: "2".into(),
            ..Default::default()
        };
        let first = service.query(query.clone()).unwrap();
        service.mark_writes_stale(&[PathBuf::from(snapshot.root).join("nested")]);
        assert!(service.get(Some("scan")).unwrap().unwrap().stale);
        let error = service.query(QueryRequest { cursor: first.next_cursor, ..query }).unwrap_err();
        assert_eq!(error.code, "staleCursor");
    }

    #[test]
    fn action_rejects_replaced_identity_and_root() {
        let (dir, service, _) = fixture();
        let path = dir.path().join("root").join("target.txt");
        fs::write(&path, b"original").unwrap();
        let original = fingerprint(&path, &fs::symlink_metadata(&path).unwrap());
        service.db.lock().unwrap().execute("INSERT INTO nodes(scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,fingerprint) VALUES('scan',1,'target.txt',?1,?2,'/999/',1,'file',?3)",params![display(&path),path_bytes(&path),original]).unwrap();
        let node = service.db.lock().unwrap().last_insert_rowid().to_string();
        fs::rename(&path, path.with_extension("old")).unwrap();
        fs::write(&path, b"replaced").unwrap();
        let request = PrepareActionRequest {
            scan_id: "scan".into(),
            generation_id: "generation".into(),
            node_id: node,
            action: "rename".into(),
            new_name: Some("renamed.txt".into()),
        };
        let error = service.prepare_action(request).unwrap_err();
        assert_eq!(error.code, if cfg!(windows) { "staleTarget" } else { "unsupported" });
        let error = service
            .prepare_action(PrepareActionRequest {
                scan_id: "scan".into(),
                generation_id: "generation".into(),
                node_id: "1".into(),
                action: "rename".into(),
                new_name: Some("renamed".into()),
            })
            .unwrap_err();
        assert_eq!(error.code, "unsupported");
        assert_eq!(fs::read(&path).unwrap(), b"replaced");
    }

    #[cfg(windows)]
    #[test]
    fn identity_bound_rename_preserves_collision_and_supports_case_only() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let collision = dir.path().join("collision.txt");
        fs::write(&source, b"source").unwrap();
        fs::write(&collision, b"collision").unwrap();
        let expected = fingerprint(&source, &fs::symlink_metadata(&source).unwrap());
        let result = safe_rename(dir.path(), &source, &collision, &expected);
        assert!(
            result.is_err(),
            "unexpected rename success: source={:?}, destination={:?}",
            fs::read(&source),
            fs::read(&collision)
        );
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&collision).unwrap(), b"collision");
        let renamed = dir.path().join("SOURCE.txt");
        safe_rename(dir.path(), &source, &renamed, &expected).unwrap();
        assert_eq!(fs::read(&renamed).unwrap(), b"source");
    }

    #[test]
    fn invalid_names_are_not_silently_normalized() {
        for name in ["trailing ", " leading", "nul\0suffix", "dot.", "bad/name"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
        #[cfg(windows)]
        for name in ["file:stream", "CON.txt", "bad?name"] {
            assert!(validate_name(name).is_err());
        }
    }

    fn staged_fixture(service: &SnifferService, old: &ScanSnapshot) -> ScanSnapshot {
        let mut snapshot = old.clone();
        snapshot.id = "replacement".into();
        snapshot.generation_id = "replacement-generation".into();
        snapshot.root = display(&PathBuf::from(&old.root).join("nested"));
        snapshot.root_node_id = Some("300".into());
        snapshot.status = ScanStatus::Scanning;
        snapshot.logical_bytes = "500".into();
        snapshot.files_visited = "1".into();
        snapshot.folders_visited = "0".into();
        {
            let db = service.db.lock().unwrap();
            db.execute("INSERT INTO nodes(id,scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files) VALUES(299,'scan',1,'sibling','sibling',X'','/299/',1,'file',10,1)",[]).unwrap();
            db.execute("UPDATE nodes SET size=size+10,files=files+1 WHERE id=1", []).unwrap();
            db.execute("INSERT INTO scans(id,generation_id,root_display,root_native,root_node_id,status,revision,started_at,last_accessed) VALUES('replacement','replacement-generation',?1,?2,300,'scanning',7,?3,?3)",params![snapshot.root,path_bytes(Path::new(&snapshot.root)),now_ms()]).unwrap();
            db.execute("INSERT INTO nodes(id,scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files) VALUES(300,'replacement',NULL,'nested',?1,?2,'/',0,'directory',500,1)",params![snapshot.root,path_bytes(Path::new(&snapshot.root))]).unwrap();
            db.execute("INSERT INTO nodes(id,scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files) VALUES(301,'replacement',300,'new.txt','new.txt',X'','/301/',1,'file',500,1)",[]).unwrap();
        }
        service.jobs.lock().unwrap().insert(
            snapshot.id.clone(),
            JobControl {
                snapshot: snapshot.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
                last_emitted: Instant::now(),
                refresh: Some(("scan".into(), 2)),
            },
        );
        snapshot
    }

    #[test]
    fn subtree_refresh_preserves_siblings_and_publishes_ancestor_totals() {
        let (_dir, service, old) = fixture();
        let staged = staged_fixture(&service, &old);
        {
            let db = service.db.lock().unwrap();
            db.execute("INSERT INTO issues(scan_id,node_id,category,path_display,message) VALUES('scan',299,'old','sibling','old issue')", []).unwrap();
            db.execute("INSERT INTO issues(scan_id,node_id,category,path_display,message) VALUES('replacement',301,'new','new.txt','new issue')", []).unwrap();
        }
        service.update_job_quiet("scan", |snapshot| snapshot.issue_count = "1".into());
        service.update_job_quiet("replacement", |snapshot| snapshot.issue_count = "10005".into());
        service.publish_refresh(&staged.id, &AtomicBool::new(false)).unwrap();
        let published = service.get(Some(&staged.id)).unwrap().unwrap();
        assert_eq!(published.root, old.root);
        assert_eq!(published.logical_bytes, "510");
        assert_eq!(published.issue_count, "10006");
        assert_eq!(published.refreshed_directory_id.as_deref(), Some("300"));
        let page = service
            .query(QueryRequest {
                scan_id: published.id.clone(),
                generation_id: published.generation_id.clone(),
                directory_id: published.root_node_id.clone().unwrap(),
                scope: "subtreeFiles".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.matched_bytes, "510");
        assert_eq!(service.node("scan", "generation", "1").unwrap().logical_size, "31385");
    }

    #[test]
    fn cancelled_refresh_does_not_publish_replacement() {
        let (_dir, service, old) = fixture();
        let staged = staged_fixture(&service, &old);
        let error = service.publish_refresh(&staged.id, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert_eq!(service.get(Some(&staged.id)).unwrap().unwrap().root, staged.root);
        assert_eq!(service.node("scan", "generation", "1").unwrap().logical_size, "31385");
        assert_eq!(service.node(&staged.id, &staged.generation_id, "300").unwrap().parent_id, None);
    }

    #[test]
    fn cancelled_batch_rolls_back_rows_and_counters() {
        let (dir, service, snapshot) = fixture();
        let path = dir.path().join("root/new.txt");
        fs::write(&path, b"new").unwrap();
        let mut batch = vec![Ok(path.clone())];
        let error = service
            .publish_batch(
                "scan",
                1,
                1,
                path.parent().unwrap(),
                &mut batch,
                &AtomicBool::new(true),
                &|_| {},
            )
            .unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert_eq!(
            service.get(Some("scan")).unwrap().unwrap().files_visited,
            snapshot.files_visited
        );
        let count: i64 = service
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM nodes WHERE path_display=?1", [display(&path)], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        assert!(service.db.lock().unwrap().is_autocommit());
    }

    #[test]
    fn wide_traversal_commits_exact_bounded_results_without_rescanning() {
        let (dir, service, _) = fixture();
        let root = dir.path().join("root");
        for index in 0..1100 {
            fs::write(root.join(format!("wide-{index}.txt")), b"abc").unwrap();
        }
        fs::create_dir(root.join("empty")).unwrap();
        service.db.lock().unwrap().execute("DELETE FROM nodes", []).unwrap();
        service.update_job_quiet("scan", |snapshot| {
            snapshot.status = ScanStatus::Scanning;
            snapshot.root_node_id = None;
            snapshot.files_visited = "0".into();
            snapshot.folders_visited = "0".into();
            snapshot.logical_bytes = "0".into();
        });
        let updates = Mutex::new(Vec::new());
        service
            .traverse("scan", &root, &AtomicBool::new(false), &|snapshot| {
                updates.lock().unwrap().push(snapshot.clone())
            })
            .unwrap();
        service.finish("scan", ScanStatus::Completed, None, &|snapshot| {
            updates.lock().unwrap().push(snapshot.clone())
        });
        let completed = service.get(Some("scan")).unwrap().unwrap();
        assert_eq!(completed.files_visited, "1100");
        assert_eq!(completed.folders_visited, "1");
        assert_eq!(completed.logical_bytes, "3300");
        let root_id = completed.root_node_id.unwrap();
        let summary = service.summary("scan", "generation", &root_id, None).unwrap();
        assert_eq!(summary.logical_bytes, "3300");
        assert_eq!(summary.directory.status, "complete");
        assert_eq!(summary.tiles.len(), 41);
        assert_eq!(summary.zero_size_count, "1");
        let query = QueryRequest {
            scan_id: "scan".into(),
            generation_id: "generation".into(),
            directory_id: root_id,
            scope: "subtreeFiles".into(),
            ..Default::default()
        };
        let first = service.query(query.clone()).unwrap();
        let second = service.query(QueryRequest { cursor: first.next_cursor, ..query }).unwrap();
        assert_eq!(first.rows.len(), 100);
        assert_eq!(second.rows.len(), 100);
        assert!(!second
            .rows
            .iter()
            .any(|row| first.rows.iter().any(|previous| previous.node_id == row.node_id)));
        assert!(updates.lock().unwrap().windows(2).all(|pair| pair[0].revision < pair[1].revision));
    }
}
