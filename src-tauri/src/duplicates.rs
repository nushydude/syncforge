use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use jwalk::WalkDir;
use serde::{Deserialize, Serialize};

use crate::hashing;

const MAX_WARNINGS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateMatchMode {
    Filename,
    Size,
    Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateScanStatus {
    Running,
    Completed,
    Cancelled,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateScanPhase {
    Collecting,
    Hashing,
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateScanJob {
    #[serde(default)]
    pub attempt: u64,
    #[serde(default)]
    pub sequence: u64,
    pub id: String,
    pub root: String,
    pub mode: DuplicateMatchMode,
    pub status: DuplicateScanStatus,
    pub phase: Option<DuplicateScanPhase>,
    pub files_found: u64,
    pub total_files: Option<u64>,
    pub hashed_files: u64,
    pub hash_total: Option<u64>,
    pub bytes_processed: u64,
    pub bytes_total: Option<u64>,
    pub current_path: Option<String>,
    pub cancel_requested: bool,
    #[serde(skip_serializing)]
    pub result: Option<DuplicateScanResult>,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
}

impl DuplicateScanJob {
    pub fn compact_copy(&self, sequence: u64) -> Self {
        Self {
            id: self.id.clone(),
            attempt: self.attempt,
            sequence,
            root: self.root.clone(),
            mode: self.mode,
            status: self.status,
            phase: self.phase,
            files_found: self.files_found,
            total_files: self.total_files,
            hashed_files: self.hashed_files,
            hash_total: self.hash_total,
            bytes_processed: self.bytes_processed,
            bytes_total: self.bytes_total,
            current_path: self.current_path.clone(),
            cancel_requested: self.cancel_requested,
            result: None,
            error: self.error.clone(),
            started_at: self.started_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub relative_path: String,
    pub name: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub key: String,
    pub files: Vec<DuplicateFile>,
    pub potential_savings_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateScanResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_identity: Option<String>,
    pub root: String,
    pub mode: DuplicateMatchMode,
    pub scanned_files: u32,
    pub hashed_files: u32,
    pub skipped_files: u32,
    pub potential_savings_bytes: u64,
    pub groups: Vec<DuplicateGroup>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateResultSummary {
    pub revision: String,
    pub cleanup_available: bool,
    pub root_identity: Option<String>,
    pub root: String,
    pub mode: DuplicateMatchMode,
    pub scanned_files: u32,
    pub hashed_files: u32,
    pub skipped_files: u32,
    pub potential_savings_bytes: u64,
    pub group_count: usize,
    pub warnings: Vec<String>,
}
impl DuplicateResultSummary {
    pub fn from_result(job_id: &str, r: &DuplicateScanResult) -> Self {
        Self {
            revision: r.revision.clone().unwrap_or_else(|| format!("legacy:{job_id}")),
            cleanup_available: cfg!(windows) && r.revision.is_some() && r.root_identity.is_some(),
            root_identity: r.root_identity.clone(),
            root: r.root.clone(),
            mode: r.mode,
            scanned_files: r.scanned_files,
            hashed_files: r.hashed_files,
            skipped_files: r.skipped_files,
            potential_savings_bytes: r.potential_savings_bytes,
            group_count: r.groups.len(),
            warnings: r.warnings.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateJobSnapshot {
    #[serde(flatten)]
    pub job: DuplicateScanJob,
    pub summary: Option<DuplicateResultSummary>,
}
impl DuplicateJobSnapshot {
    pub fn from_job(mut job: DuplicateScanJob) -> Self {
        let summary = job.result.as_ref().map(|r| DuplicateResultSummary::from_result(&job.id, r));
        job.result = None;
        Self { job, summary }
    }
}

pub const DUPLICATE_PAGE_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupSummary {
    pub id: String,
    pub key: String,
    pub file_count: u64,
    pub potential_savings_bytes: u64,
    pub first_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupsPage {
    pub revision: String,
    pub groups: Vec<DuplicateGroupSummary>,
    pub next_cursor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMembersPage {
    pub revision: String,
    pub group_id: String,
    pub files: Vec<DuplicateFile>,
    pub next_cursor: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateFile {
    pub(crate) absolute_path: PathBuf,
    pub(crate) relative_path: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) fingerprint: Option<String>,
}

pub fn mode_to_str(mode: DuplicateMatchMode) -> &'static str {
    match mode {
        DuplicateMatchMode::Filename => "filename",
        DuplicateMatchMode::Size => "size",
        DuplicateMatchMode::Hash => "hash",
    }
}

pub fn mode_from_str(value: &str) -> Result<DuplicateMatchMode, String> {
    match value {
        "filename" => Ok(DuplicateMatchMode::Filename),
        "size" => Ok(DuplicateMatchMode::Size),
        "hash" => Ok(DuplicateMatchMode::Hash),
        other => Err(format!("invalid duplicate match mode: {other}")),
    }
}

pub fn status_to_str(status: DuplicateScanStatus) -> &'static str {
    match status {
        DuplicateScanStatus::Running => "running",
        DuplicateScanStatus::Completed => "completed",
        DuplicateScanStatus::Cancelled => "cancelled",
        DuplicateScanStatus::Interrupted => "interrupted",
        DuplicateScanStatus::Failed => "failed",
    }
}

pub fn status_from_str(value: &str) -> Result<DuplicateScanStatus, String> {
    match value {
        "running" => Ok(DuplicateScanStatus::Running),
        "completed" => Ok(DuplicateScanStatus::Completed),
        "cancelled" => Ok(DuplicateScanStatus::Cancelled),
        "interrupted" => Ok(DuplicateScanStatus::Interrupted),
        "failed" => Ok(DuplicateScanStatus::Failed),
        other => Err(format!("invalid duplicate scan status: {other}")),
    }
}

pub fn phase_to_str(phase: Option<DuplicateScanPhase>) -> Option<&'static str> {
    phase.map(|phase| match phase {
        DuplicateScanPhase::Collecting => "collecting",
        DuplicateScanPhase::Hashing => "hashing",
        DuplicateScanPhase::Finalizing => "finalizing",
    })
}

pub fn phase_from_str(value: Option<String>) -> Result<Option<DuplicateScanPhase>, String> {
    match value.as_deref() {
        None => Ok(None),
        Some("collecting") => Ok(Some(DuplicateScanPhase::Collecting)),
        Some("hashing") => Ok(Some(DuplicateScanPhase::Hashing)),
        Some("finalizing") => Ok(Some(DuplicateScanPhase::Finalizing)),
        Some(other) => Err(format!("invalid duplicate scan phase: {other}")),
    }
}

// Older results without native identity remain visible and require a fresh scan
// before a handoff.
#[cfg(windows)]
fn identity(path: &Path) -> std::io::Result<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    windows_file_id(&file)
}

#[cfg(windows)]
fn windows_file_id(file: &std::fs::File) -> std::io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO,
    };
    let mut info = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0
    {
        // Preserve the full identifier on volumes that provide 128-bit file IDs.
        return Ok(format!("{:x}:{:02x?}", info.VolumeSerialNumber, info.FileId.Identifier));
    }
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(format!("{}:{}:{}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}
#[cfg(unix)]
fn identity(path: &Path) -> std::io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path)?;
    Ok(format!("{}:{}", m.dev(), m.ino()))
}
#[cfg(windows)]
fn fingerprint(path: &Path) -> std::io::Result<String> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || is_link(&meta) {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut basic = FILE_BASIC_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(format!(
        "{}:{}:{}:{}",
        windows_file_id(&file)?,
        meta.len(),
        basic.LastWriteTime,
        basic.ChangeTime
    ))
}
#[cfg(unix)]
fn fingerprint(path: &Path) -> std::io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(format!(
        "{}:{}:{}:{}:{}:{}:{}",
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec()
    ))
}

fn warning(warnings: &mut Vec<String>, path: &Path, reason: impl std::fmt::Display) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(format!("{}: {reason}", path.display()));
    }
}

fn relative_path(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root).ok().map(|relative| {
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    })
}

fn collect_files(
    root: &Path,
    cancel: &AtomicBool,
    on_file: &mut dyn FnMut(&CandidateFile, u64),
) -> Result<(Vec<CandidateFile>, u32, Vec<String>), String> {
    let mut files = Vec::new();
    let mut skipped_files = 0;
    let mut warnings = Vec::new();

    for entry in WalkDir::new(root).follow_links(false).into_iter() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                skipped_files += 1;
                warning(&mut warnings, root, error);
                continue;
            }
        };

        let path = entry.path();
        if path == root {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                skipped_files += 1;
                warning(&mut warnings, &path, error);
                continue;
            }
        };

        if !metadata.is_file() {
            continue;
        }

        let Some(relative_path) = relative_path(&path, root) else {
            skipped_files += 1;
            warning(&mut warnings, &path, "could not determine relative path");
            continue;
        };
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            skipped_files += 1;
            warning(&mut warnings, &path, "could not determine filename");
            continue;
        };

        let observed_fingerprint = fingerprint(&path).ok();
        files.push(CandidateFile {
            absolute_path: path,
            relative_path,
            name,
            size: metadata.len(),
            fingerprint: observed_fingerprint,
        });
        let file = files.last().expect("just pushed file");
        on_file(file, files.len() as u64);
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok((files, skipped_files, warnings))
}

fn group_key(file: &CandidateFile, mode: DuplicateMatchMode) -> String {
    match mode {
        DuplicateMatchMode::Filename => file.name.to_lowercase(),
        DuplicateMatchMode::Size | DuplicateMatchMode::Hash => file.size.to_string(),
    }
}

fn to_duplicate_file(file: &CandidateFile, hash: Option<String>) -> DuplicateFile {
    DuplicateFile {
        id: Some(uuid::Uuid::new_v4().to_string()),
        fingerprint: file.fingerprint.clone().filter(|expected| {
            fingerprint(&file.absolute_path).ok().as_deref() == Some(expected.as_str())
        }),
        relative_path: file.relative_path.clone(),
        name: file.name.clone(),
        size: file.size,
        hash,
    }
}

fn make_group(key: String, files: Vec<&CandidateFile>, hash: Option<String>) -> DuplicateGroup {
    let files: Vec<DuplicateFile> =
        files.iter().map(|file| to_duplicate_file(file, hash.clone())).collect();
    let potential_savings_bytes = files.iter().skip(1).map(|file| file.size).sum();

    DuplicateGroup {
        id: Some(uuid::Uuid::new_v4().to_string()),
        key,
        files,
        potential_savings_bytes,
    }
}

fn build_groups(
    files: &[CandidateFile],
    mode: DuplicateMatchMode,
    warnings: &mut Vec<String>,
    cancel: &AtomicBool,
    on_hash_progress: &mut dyn FnMut(&CandidateFile, u64, u64, bool),
) -> (Vec<DuplicateGroup>, u32) {
    let mut candidates: HashMap<String, Vec<&CandidateFile>> = HashMap::new();
    for file in files {
        candidates.entry(group_key(file, mode)).or_default().push(file);
    }

    let mut hashed_files = 0;
    let mut groups = Vec::new();

    for (key, candidates) in candidates {
        if candidates.len() < 2 {
            continue;
        }

        if mode == DuplicateMatchMode::Hash {
            let mut by_hash: HashMap<String, Vec<&CandidateFile>> = HashMap::new();
            for file in candidates {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let mut file_bytes = 0;
                match hashing::hash_file_with_progress(&file.absolute_path, |processed, total| {
                    file_bytes = processed;
                    on_hash_progress(file, processed, total, false);
                    !cancel.load(Ordering::Relaxed)
                }) {
                    Ok(hash) => {
                        hashed_files += 1;
                        on_hash_progress(file, file_bytes, file.size, true);
                        by_hash.entry(hash).or_default().push(file);
                    }
                    Err(error) => warning(warnings, &file.absolute_path, error),
                }
            }

            for (hash, hash_files) in by_hash {
                if hash_files.len() >= 2 {
                    groups.push(make_group(hash.clone(), hash_files, Some(hash)));
                }
            }
        } else {
            groups.push(make_group(key, candidates, None));
        }
    }

    groups.sort_by(|a, b| a.files[0].relative_path.cmp(&b.files[0].relative_path));
    (groups, hashed_files)
}

#[cfg(test)]
pub fn find_duplicates(
    root: &Path,
    mode: DuplicateMatchMode,
) -> Result<DuplicateScanResult, String> {
    let cancel = AtomicBool::new(false);
    find_duplicates_with_progress(
        root,
        mode,
        &cancel,
        &mut |_, _| {},
        &mut |_, _, _| {},
        &mut |_, _, _, _| {},
    )
}

pub(crate) fn find_duplicates_with_progress(
    root: &Path,
    mode: DuplicateMatchMode,
    cancel: &AtomicBool,
    on_file: &mut dyn FnMut(&CandidateFile, u64),
    on_phase: &mut dyn FnMut(DuplicateScanPhase, Option<u64>, Option<u64>),
    on_hash_progress: &mut dyn FnMut(&CandidateFile, u64, u64, bool),
) -> Result<DuplicateScanResult, String> {
    let root = root.canonicalize().map_err(|error| format!("could not open folder: {error}"))?;
    if !root.is_dir() {
        return Err(format!("not a folder: {}", root.display()));
    }

    let root_identity = identity(&root).map_err(|e| format!("could not identify folder: {e}"))?;
    let (files, skipped_files, mut warnings) = collect_files(&root, cancel, on_file)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let scanned_files = files.len() as u32;
    let hash_total = if mode == DuplicateMatchMode::Hash {
        let mut sizes: HashMap<u64, (u64, u64)> = HashMap::new();
        for file in &files {
            let entry = sizes.entry(file.size).or_default();
            entry.0 += 1;
            entry.1 += file.size;
        }
        Some(sizes.into_values().filter_map(|(count, bytes)| (count >= 2).then_some(bytes)).sum())
    } else {
        None
    };
    if mode == DuplicateMatchMode::Hash {
        on_phase(DuplicateScanPhase::Hashing, Some(files.len() as u64), hash_total);
    } else {
        on_phase(DuplicateScanPhase::Finalizing, Some(files.len() as u64), None);
    }
    let (groups, hashed_files) =
        build_groups(&files, mode, &mut warnings, cancel, on_hash_progress);
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let potential_savings_bytes = groups.iter().map(|group| group.potential_savings_bytes).sum();

    if identity(&root).ok().as_deref() != Some(root_identity.as_str()) {
        return Err("Folder changed during scan. Scan again.".into());
    }
    Ok(DuplicateScanResult {
        revision: Some(uuid::Uuid::new_v4().to_string()),
        root_identity: Some(root_identity),
        root: root.display().to_string(),
        mode,
        scanned_files,
        hashed_files,
        skipped_files,
        potential_savings_bytes,
        groups,
        warnings,
    })
}

/// Validate the reviewed item and (for exact matches) a still-matching keeper,
/// holding no-follow handles until the non-mutating handoff has been requested.
pub(crate) fn handoff_reviewed_with<F, P>(
    root_path: &Path,
    root_identity: &str,
    mode: DuplicateMatchMode,
    file: &DuplicateFile,
    mut page_keepers: P,
    mut reveal: F,
) -> Result<(), String>
where
    F: FnMut(&Path) -> Result<(), String>,
    P: FnMut(usize) -> Result<DuplicateMembersPage, String>,
{
    let stale = |reason: String| {
        format!(
            "This file or its matching keeper changed. Scan again before opening it. ({reason})"
        )
    };
    let root = root_path.canonicalize().map_err(|e| stale(e.to_string()))?;
    let path = root.join(&file.relative_path);
    let _locks = lock_ancestors(&root, &path).map_err(&stale)?;
    if identity(&root).ok().as_deref() != Some(root_identity) {
        return Err(stale("folder identity changed".into()));
    }
    let _target = lock_file(&path).map_err(&stale)?;
    validate_file(&root, &path, file).map_err(&stale)?;
    if mode == DuplicateMatchMode::Hash {
        let hash =
            file.hash.as_deref().ok_or_else(|| stale("reviewed hash is unavailable".into()))?;
        if hashing::hash_file(&path).map_err(|e| stale(e.to_string()))? != hash {
            return Err(stale("file contents changed".into()));
        }
        let mut cursor = 0;
        loop {
            let page = page_keepers(cursor).map_err(&stale)?;
            for keeper in page.files {
                if keeper.id == file.id {
                    continue;
                }
                let candidate = root.join(&keeper.relative_path);
                if let (Ok(_keeper_locks), Ok(_keeper)) =
                    (lock_ancestors(&root, &candidate), lock_file(&candidate))
                {
                    if validate_file(&root, &candidate, &keeper).is_ok()
                        && keeper.hash.as_deref() == Some(hash)
                        && hashing::hash_file(&candidate).ok().as_deref() == Some(hash)
                    {
                        validate_file(&root, &path, file).map_err(&stale)?;
                        return reveal(&path)
                            .map_err(|e| format!("Could not send the Explorer request: {e}"));
                    }
                }
            }
            match page.next_cursor {
                Some(next) if next > cursor => cursor = next,
                _ => break,
            }
        }
        Err(stale("no reviewed matching keeper remains".into()))
    } else {
        reveal(&path).map_err(|e| format!("Could not send the Explorer request: {e}"))
    }
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    use std::path::Component;
    if Path::new(path).components().any(|c| !matches!(c, Component::Normal(_))) || path.is_empty() {
        return Err("invalid relative path".into());
    }
    Ok(())
}
fn validate_file(root: &Path, path: &Path, file: &DuplicateFile) -> Result<(), String> {
    validate_relative_path(&file.relative_path)?;
    let relative = Path::new(&file.relative_path);
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let meta = std::fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
        if is_link(&meta) {
            return Err("path contains a link or reparse point; scan again".into());
        }
    }
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(root) {
        return Err("path is outside the selected folder".into());
    }
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() != file.size {
        return Err("file size changed; scan again".into());
    }
    let expected = file.fingerprint.as_deref().ok_or("file identity unavailable; scan again")?;
    if fingerprint(path).ok().as_deref() != Some(expected) {
        return Err("file changed or disappeared; scan again".into());
    }
    Ok(())
}
#[cfg(windows)]
fn is_link(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}
#[cfg(not(windows))]
fn is_link(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

#[cfg(windows)]
fn lock_ancestors(root: &Path, path: &Path) -> Result<Vec<std::fs::File>, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut handles = Vec::new();
    let mut current = path.parent();
    let mut found_root = false;
    while let Some(parent) = current {
        let handle = std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(parent)
            .map_err(|e| e.to_string())?;
        if is_link(&handle.metadata().map_err(|e| e.to_string())?) {
            return Err("ancestor is a link or reparse point".into());
        }
        handles.push(handle);
        if parent == root {
            found_root = true;
        }
        current = parent.parent();
    }
    if !found_root {
        return Err("path is outside the selected folder".into());
    }
    Ok(handles)
}
#[cfg(not(windows))]
fn lock_ancestors(_root: &Path, _path: &Path) -> Result<Vec<std::fs::File>, String> {
    Err("Opening in Explorer is available on Windows.".into())
}

#[cfg(windows)]
fn lock_file(path: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
    let file = std::fs::OpenOptions::new()
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| e.to_string())?;
    if is_link(&file.metadata().map_err(|e| e.to_string())?) {
        return Err("file is a reparse point".into());
    }
    Ok(file)
}
#[cfg(not(windows))]
fn lock_file(_path: &Path) -> Result<std::fs::File, String> {
    Err("Opening in Explorer is available on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_folder() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir_all(dir.path().join("nested")).expect("mkdir");
        fs::write(dir.path().join("first.txt"), "same contents").expect("write");
        fs::write(dir.path().join("nested/second.txt"), "same contents").expect("write");
        fs::write(dir.path().join("different.txt"), "different").expect("write");
        dir
    }

    #[test]
    fn hash_mode_finds_exact_content_duplicates() {
        let dir = setup_folder();
        let result = find_duplicates(dir.path(), DuplicateMatchMode::Hash).expect("scan");
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 2);
        assert_eq!(result.hashed_files, 2);
    }

    #[test]
    fn filename_mode_groups_same_names_even_when_content_differs() {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir_all(dir.path().join("a")).expect("mkdir");
        fs::create_dir_all(dir.path().join("b")).expect("mkdir");
        fs::write(dir.path().join("a/report.txt"), "one").expect("write");
        fs::write(dir.path().join("b/report.txt"), "two").expect("write");

        let result = find_duplicates(dir.path(), DuplicateMatchMode::Filename).expect("scan");
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 2);
        assert!(result.groups[0].files.iter().all(|file| file.hash.is_none()));
    }

    #[test]
    fn size_mode_does_not_hash_files() {
        let dir = setup_folder();
        let result = find_duplicates(dir.path(), DuplicateMatchMode::Size).expect("scan");
        assert_eq!(result.hashed_files, 0);
        assert_eq!(result.scanned_files, 3);
    }

    #[test]
    fn legacy_results_deserialize_without_cleanup_identity() {
        let value = serde_json::json!({"root":"x", "mode":"size", "scannedFiles":0, "hashedFiles":0, "skippedFiles":0, "potentialSavingsBytes":0, "groups":[], "warnings":[]});
        let result: DuplicateScanResult = serde_json::from_value(value).unwrap();
        assert!(result.revision.is_none());
        assert!(result.root_identity.is_none());
    }
    #[cfg(windows)]
    #[test]
    fn handoff_validates_reviewed_file_and_keeper_without_mutation() {
        let dir = setup_folder();
        let result = find_duplicates(dir.path(), DuplicateMatchMode::Hash).unwrap();
        let files = &result.groups[0].files;
        let mut calls = Vec::new();
        let page = |_| {
            Ok(DuplicateMembersPage {
                revision: result.revision.clone().unwrap(),
                group_id: "fixture".into(),
                files: files.clone(),
                next_cursor: None,
            })
        };
        handoff_reviewed_with(
            Path::new(&result.root),
            result.root_identity.as_deref().unwrap(),
            result.mode,
            &files[1],
            page,
            |p| {
                // Both checked files must refuse a write-open until the handoff returns.
                for file in files {
                    let reviewed = dir.path().join(&file.relative_path);
                    assert_eq!(fs::read(&reviewed).unwrap(), b"same contents");
                    assert!(fs::OpenOptions::new().write(true).open(&reviewed).is_err());
                }
                calls.push(p.to_path_buf());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert!(dir.path().join(&files[1].relative_path).exists());
        fs::write(dir.path().join(&files[0].relative_path), b"different now").unwrap();
        let page = |_| {
            Ok(DuplicateMembersPage {
                revision: result.revision.clone().unwrap(),
                group_id: "fixture".into(),
                files: files.clone(),
                next_cursor: None,
            })
        };
        assert!(handoff_reviewed_with(
            Path::new(&result.root),
            result.root_identity.as_deref().unwrap(),
            result.mode,
            &files[1],
            page,
            |_| panic!("stale keeper must not invoke handoff")
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn handoff_refuses_a_replaced_file_and_reports_opener_errors_separately() {
        let dir = setup_folder();
        let result = find_duplicates(dir.path(), DuplicateMatchMode::Hash).unwrap();
        let files = &result.groups[0].files;
        let page = |_| {
            Ok(DuplicateMembersPage {
                revision: result.revision.clone().unwrap(),
                group_id: "fixture".into(),
                files: files.clone(),
                next_cursor: None,
            })
        };
        let error = handoff_reviewed_with(
            Path::new(&result.root),
            result.root_identity.as_deref().unwrap(),
            result.mode,
            &files[1],
            page,
            |_| Err("opener unavailable".into()),
        )
        .unwrap_err();
        assert!(error.contains("Could not send the Explorer request"));
        let path = dir.path().join(&files[1].relative_path);
        fs::write(&path, b"changed").unwrap();
        let page = |_| {
            Ok(DuplicateMembersPage {
                revision: result.revision.clone().unwrap(),
                group_id: "fixture".into(),
                files: files.clone(),
                next_cursor: None,
            })
        };
        let error = handoff_reviewed_with(
            Path::new(&result.root),
            result.root_identity.as_deref().unwrap(),
            result.mode,
            &files[1],
            page,
            |_| panic!("stale item must not invoke handoff"),
        )
        .unwrap_err();
        assert!(error.contains("Scan again"));
    }
}
