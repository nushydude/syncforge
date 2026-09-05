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
}

#[derive(Clone)]
struct JobControl {
    snapshot: ScanSnapshot,
    cancel: Arc<AtomicBool>,
}
struct PreparedAction {
    review: ActionReview,
    scan_id: String,
    generation_id: String,
    parent_id: i64,
    native_path: Vec<u8>,
    fingerprint: String,
}

pub struct SnifferService {
    db_path: PathBuf,
    db: Mutex<Connection>,
    jobs: Mutex<HashMap<String, JobControl>>,
    active: Mutex<Option<String>>,
    actions: Mutex<HashMap<String, PreparedAction>>,
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
fn fingerprint(metadata: &fs::Metadata) -> String {
    format!(
        "{}:{}:{}",
        metadata.len(),
        modified_ms(metadata).unwrap_or(-1),
        if metadata.is_dir() { "d" } else { "f" }
    )
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
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS scans(id TEXT PRIMARY KEY,generation_id TEXT NOT NULL,root_display TEXT NOT NULL,root_native BLOB NOT NULL,root_node_id INTEGER,status TEXT NOT NULL,revision INTEGER NOT NULL,files INTEGER NOT NULL DEFAULT 0,folders INTEGER NOT NULL DEFAULT 0,bytes INTEGER NOT NULL DEFAULT 0,issues INTEGER NOT NULL DEFAULT 0,coverage INTEGER NOT NULL DEFAULT 1,stale INTEGER NOT NULL DEFAULT 0,current_directory TEXT,started_at INTEGER NOT NULL,finished_at INTEGER,error TEXT,last_accessed INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS nodes(id INTEGER PRIMARY KEY,scan_id TEXT NOT NULL,parent_id INTEGER,name_display TEXT NOT NULL,path_display TEXT NOT NULL,path_native BLOB NOT NULL,path_key TEXT NOT NULL,depth INTEGER NOT NULL,kind TEXT NOT NULL,size INTEGER NOT NULL DEFAULT 0,files INTEGER NOT NULL DEFAULT 0,folders INTEGER NOT NULL DEFAULT 0,modified_at INTEGER,state TEXT NOT NULL DEFAULT 'complete',fingerprint TEXT,extension TEXT,FOREIGN KEY(scan_id) REFERENCES scans(id) ON DELETE CASCADE);
          CREATE INDEX IF NOT EXISTS nodes_parent ON nodes(scan_id,parent_id); CREATE INDEX IF NOT EXISTS nodes_subtree ON nodes(scan_id,path_key); CREATE INDEX IF NOT EXISTS nodes_size ON nodes(scan_id,size DESC,id); CREATE INDEX IF NOT EXISTS nodes_name ON nodes(scan_id,name_display COLLATE NOCASE,id);
          CREATE TABLE IF NOT EXISTS issues(id INTEGER PRIMARY KEY,scan_id TEXT NOT NULL,node_id INTEGER,category TEXT NOT NULL,path_display TEXT NOT NULL,code TEXT,message TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS issues_scan ON issues(scan_id,category,id);
          CREATE TABLE IF NOT EXISTS pending_directories(scan_id TEXT NOT NULL,node_id INTEGER NOT NULL,path_native BLOB NOT NULL,depth INTEGER NOT NULL,PRIMARY KEY(scan_id,node_id));")?;
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
        })
    }

    pub fn start(
        self: &Arc<Self>,
        root: &str,
        app: AppHandle,
        state: Arc<AppState>,
    ) -> Result<ScanSnapshot, SnifferError> {
        let root = Path::new(root).canonicalize().map_err(|e| {
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
        };
        self.db.lock().map_err(|e| sql_error("scan", e))?.execute("INSERT INTO scans(id,generation_id,root_display,root_native,status,revision,started_at,last_accessed) VALUES(?1,?2,?3,?4,'queued',1,?5,?5)", params![id,generation_id,display(&root),path_bytes(&root),now]).map_err(|e| sql_error("scan",e))?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs
            .lock()
            .map_err(|e| sql_error("scan", e))?
            .insert(id.clone(), JobControl { snapshot: snapshot.clone(), cancel: cancel.clone() });
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
                    &app,
                );
            }
            retry_pending_syncs(app, &state);
        });
        Ok(snapshot)
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
        let mut permit = None;
        while permit.is_none() && !cancel.load(Ordering::Acquire) {
            permit = match coordinator.try_acquire(WorkRequest::new(
                Vec::new(),
                false,
                HeavyJobKind::Sniffer,
            )) {
                Ok(permit) => permit,
                Err(error) => {
                    self.finish(&id, ScanStatus::Failed, Some(sql_error("scan", error)), &app);
                    return;
                }
            };
            if permit.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        if cancel.load(Ordering::Acquire) {
            self.finish(&id, ScanStatus::Cancelled, None, &app);
            return;
        }
        self.update_job(&id, &app, true, |s| {
            s.status = ScanStatus::Scanning;
        });
        let result = self.traverse(&id, &root, &cancel, &app);
        drop(permit);
        match result {
            Ok(()) if cancel.load(Ordering::Acquire) => {
                self.finish(&id, ScanStatus::Cancelled, None, &app)
            }
            Ok(()) => self.finish(&id, ScanStatus::Completed, None, &app),
            Err(_e) if cancel.load(Ordering::Acquire) => {
                self.finish(&id, ScanStatus::Cancelled, None, &app)
            }
            Err(e) => self.finish(&id, ScanStatus::Failed, Some(e), &app),
        }
    }

    fn traverse(
        &self,
        scan_id: &str,
        root: &Path,
        cancel: &AtomicBool,
        app: &AppHandle,
    ) -> Result<(), SnifferError> {
        let root_meta = fs::symlink_metadata(root)
            .map_err(|e| SnifferError::new("permissionDenied", "scan", true, e.to_string()))?;
        let root_id = {
            let db = self.db.lock().map_err(|e| sql_error("scan", e))?;
            db.execute("INSERT INTO nodes(scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,modified_at,fingerprint) VALUES(?1,NULL,?2,?3,?4,'/',0,'directory',?5,?6)",params![scan_id,root.file_name().unwrap_or(root.as_os_str()).to_string_lossy(),display(root),path_bytes(root),modified_ms(&root_meta),fingerprint(&root_meta)]).map_err(|e|sql_error("scan",e))?;
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
            match fs::read_dir(&dir) {
                Ok(entries) => {
                    for child in entries {
                        if cancel.load(Ordering::Acquire) {
                            break;
                        }
                        match child {
                            Ok(child) => {
                                self.index_child(scan_id, parent_id, depth + 1, &child.path())?
                            }
                            Err(e) => self.add_issue(
                                scan_id,
                                Some(parent_id),
                                "unreadableDirectory",
                                &dir,
                                &e,
                            )?,
                        }
                    }
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
            self.db
                .lock()
                .map_err(|e| sql_error("scan", e))?
                .execute(
                    "DELETE FROM pending_directories WHERE scan_id=?1 AND node_id=?2",
                    params![scan_id, parent_id],
                )
                .map_err(|e| sql_error("scan", e))?;
        }
        self.finalize_directories(scan_id)?;
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
        db.execute("INSERT INTO nodes(scan_id,parent_id,name_display,path_display,path_native,path_key,depth,kind,size,files,modified_at,state,fingerprint,extension) VALUES(?1,?2,?3,?4,?5,'',?6,?7,?8,?9,?10,?11,?12,?13)",params![scan_id,parent,name,display(path),path_bytes(path),depth,kind,indexed_size,if kind=="file"{1}else{0},modified_ms(meta),state,fingerprint(meta),ext]).map_err(|e|sql_error("scan",e))?;
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
        job.snapshot.coverage_complete = false;
        let count = job.snapshot.issue_count.parse::<i64>().unwrap_or(0);
        drop(jobs);
        if count <= ISSUE_DETAIL_LIMIT {
            self.db.lock().map_err(|e|sql_error("scan",e))?.execute("INSERT INTO issues(scan_id,node_id,category,path_display,code,message)VALUES(?1,?2,?3,?4,?5,?6)",params![scan_id,node,category,display(path),error.raw_os_error().map(|v|v.to_string()),error.to_string()]).map_err(|e|sql_error("scan",e))?;
        }
        Ok(())
    }
    fn finalize_directories(&self, scan_id: &str) -> Result<(), SnifferError> {
        let db = self.db.lock().map_err(|e| sql_error("scan", e))?;
        let max: i64 = db
            .query_row(
                "SELECT COALESCE(MAX(depth),0) FROM nodes WHERE scan_id=?1",
                [scan_id],
                |r| r.get(0),
            )
            .map_err(|e| sql_error("scan", e))?;
        for depth in (0..=max).rev() {
            db.execute("UPDATE nodes AS n SET size=COALESCE((SELECT SUM(c.size) FROM nodes c WHERE c.parent_id=n.id),0),files=COALESCE((SELECT SUM(c.files) FROM nodes c WHERE c.parent_id=n.id),0),folders=COALESCE((SELECT SUM(c.folders+CASE WHEN c.kind='directory' THEN 1 ELSE 0 END) FROM nodes c WHERE c.parent_id=n.id),0),state=CASE WHEN EXISTS(SELECT 1 FROM issues i WHERE i.scan_id=n.scan_id AND (i.node_id=n.id OR i.node_id IN(SELECT id FROM nodes d WHERE d.path_key LIKE n.path_key||'%'))) THEN 'unreadable' ELSE 'complete' END WHERE n.scan_id=?1 AND n.kind='directory' AND n.depth=?2",params![scan_id,depth]).map_err(|e|sql_error("scan",e))?;
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
    fn update_job(&self, id: &str, app: &AppHandle, emit: bool, f: impl FnOnce(&mut ScanSnapshot)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.get_mut(id) {
                f(&mut j.snapshot);
                if emit {
                    j.snapshot.revision += 1;
                    let _ = self.persist_snapshot(&j.snapshot);
                    let _ = app.emit(PROGRESS_EVENT, &j.snapshot);
                }
            }
        }
    }
    fn finish(&self, id: &str, status: ScanStatus, error: Option<SnifferError>, app: &AppHandle) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.get_mut(id) {
                if j.snapshot.status == ScanStatus::Cancelling && status == ScanStatus::Completed {
                    j.snapshot.status = ScanStatus::Cancelled
                } else {
                    j.snapshot.status = status
                }
                j.snapshot.finished_at = Some(now_ms());
                j.snapshot.current_directory = None;
                j.snapshot.error = error;
                j.snapshot.revision += 1;
                let _ = self.persist_snapshot(&j.snapshot);
                let _ = app.emit(PROGRESS_EVENT, &j.snapshot);
            }
        }
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
        if let Ok(db) = self.db.lock() {
            let _ = db.execute("DELETE FROM pending_directories WHERE scan_id=?1", [id]);
        }
    }
    fn persist_snapshot(&self, s: &ScanSnapshot) -> Result<(), SnifferError> {
        self.db.lock().map_err(|e|sql_error("scan",e))?.execute("UPDATE scans SET root_node_id=?2,status=?3,revision=?4,files=?5,folders=?6,bytes=?7,issues=?8,coverage=?9,stale=?10,current_directory=?11,finished_at=?12,error=?13,last_accessed=?14 WHERE id=?1",params![s.id,s.root_node_id.as_deref().and_then(|v|v.parse::<i64>().ok()),status_text(&s.status),s.revision as i64,s.files_visited.parse::<i64>().unwrap_or(i64::MAX),s.folders_visited.parse::<i64>().unwrap_or(i64::MAX),s.logical_bytes.parse::<i64>().unwrap_or(i64::MAX),s.issue_count.parse::<i64>().unwrap_or(i64::MAX),s.coverage_complete,s.stale,s.current_directory,s.finished_at,s.error.as_ref().map(|e|e.message.clone()),now_ms()]).map_err(|e|sql_error("scan",e))?;
        Ok(())
    }

    pub fn query(&self, q: QueryRequest) -> Result<EntryPage, SnifferError> {
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
    ) -> Result<Summary, SnifferError> {
        let snapshot = self.validate_generation(scan_id, generation)?;
        let dir = self.node(scan_id, generation, directory)?;
        if dir.kind != "directory" {
            return Err(SnifferError::new(
                "unsupported",
                "summary",
                false,
                "Summary requires a directory.",
            ));
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
        let offset = cursor.and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let limit = limit.clamp(1, PAGE_MAX) as usize;
        let db = self.db.lock().map_err(|e| sql_error("issues", e))?;
        let total: i64 = db
            .query_row("SELECT issues FROM scans WHERE id=?1", [scan_id], |r| r.get(0))
            .map_err(|_| SnifferError::new("expired", "issues", false, "Scan expired."))?;
        let mut stmt=db.prepare("SELECT id,node_id,category,path_display,code,message FROM issues WHERE scan_id=?1 ORDER BY id").map_err(|e|sql_error("issues",e))?;
        let rows = stmt
            .query_map([scan_id], |r| {
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
            .filter_map(Result::ok)
            .filter(|r| category.is_none_or(|c| c == r.category))
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let retained: i64 = db
            .query_row("SELECT COUNT(*) FROM issues WHERE scan_id=?1", [scan_id], |r| r.get(0))
            .unwrap_or(0);
        let next =
            (offset + rows.len() < retained as usize).then(|| (offset + rows.len()).to_string());
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
        let db = self.db.lock().map_err(|e| sql_error("action", e))?;
        let (parent, native, kind): (i64, Vec<u8>, String) = db
            .query_row(
                "SELECT parent_id,path_native,kind FROM nodes WHERE scan_id=?1 AND id=?2",
                params![r.scan_id, id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| SnifferError::new("notFound", "action", false, "Node was not found."))?;
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
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| SnifferError::new("staleTarget", "action", true, e.to_string()))?;
        let new_path = if r.action == "rename" {
            let name = validate_name(r.new_name.as_deref().unwrap_or(""))?;
            Some(display(
                &path
                    .parent()
                    .ok_or_else(|| {
                        SnifferError::new(
                            "unsupported",
                            "rename",
                            false,
                            "Cannot rename this item.",
                        )
                    })?
                    .join(name),
            ))
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
            new_path,
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
                fingerprint: fingerprint(&meta),
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
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| SnifferError::new("staleTarget", "action", true, e.to_string()))?;
        if fingerprint(&meta) != action.fingerprint {
            return Err(SnifferError::new(
                "staleTarget",
                "action",
                true,
                "The item changed after review. Review it again.",
            ));
        }
        let _permit = coordinator
            .acquire_manual(WorkRequest::new(vec![path.clone()], true, HeavyJobKind::Sniffer))
            .map_err(|e| sql_error("action", e))?;
        let new_path = if action.review.action == "recycle" {
            trash::delete(&path).map_err(|e| {
                SnifferError::new(
                    "unsupported",
                    "recycle",
                    true,
                    format!("Could not move item to the Recycle Bin: {e}"),
                )
            })?;
            None
        } else {
            let dest = PathBuf::from(action.review.new_path.as_ref().unwrap());
            rename_no_replace(&path, &dest)?;
            Some(display(&dest))
        };
        self.db
            .lock()
            .map_err(|e| sql_error("action", e))?
            .execute("UPDATE scans SET stale=1,revision=revision+1 WHERE id=?1", [&action.scan_id])
            .map_err(|e| sql_error("action", e))?;
        self.update_job_quiet(&action.scan_id, |s| {
            s.stale = true;
            s.revision += 1;
        });
        Ok(ActionResult {
            action: action.review.action,
            path: action.review.full_path,
            new_path,
            stale_directory_id: action.parent_id.to_string(),
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
    if n.is_empty() || n == "." || n == ".." || n.contains(['\\', '/']) || n.ends_with(['.', ' ']) {
        return Err(SnifferError::new(
            "unsupported",
            "rename",
            false,
            "Enter a valid filename without path separators, trailing spaces, or trailing dots.",
        ));
    }
    #[cfg(windows)]
    {
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
fn rename_no_replace(source: &Path, dest: &Path) -> Result<(), SnifferError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let s: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let d: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(s.as_ptr(), d.as_ptr(), 0) } == 0 {
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
fn rename_no_replace(source: &Path, dest: &Path) -> Result<(), SnifferError> {
    if dest.exists() {
        return Err(SnifferError::new(
            "nameCollision",
            "rename",
            false,
            "An item with that name already exists.",
        ));
    }
    fs::rename(source, dest).map_err(|e| SnifferError::new("failed", "rename", true, e.to_string()))
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
            JobControl { snapshot: snapshot.clone(), cancel: Arc::new(AtomicBool::new(false)) },
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
}
