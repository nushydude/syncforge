use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use jwalk::WalkDir;
use serde::{Deserialize, Serialize};

use crate::hashing;
use crate::path_normalization;

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
    pub result: Option<DuplicateScanResult>,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    pub relative_path: String,
    pub name: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub key: String,
    pub files: Vec<DuplicateFile>,
    pub potential_savings_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateScanResult {
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
pub struct DuplicateCleanupResult {
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateFile {
    pub(crate) absolute_path: PathBuf,
    pub(crate) relative_path: String,
    pub(crate) name: String,
    pub(crate) size: u64,
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

        files.push(CandidateFile {
            absolute_path: path,
            relative_path,
            name,
            size: metadata.len(),
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

    DuplicateGroup { key, files, potential_savings_bytes }
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

    Ok(DuplicateScanResult {
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

pub fn remove_duplicates(root: &Path, paths: &[String]) -> Result<DuplicateCleanupResult, String> {
    let root = root.canonicalize().map_err(|error| format!("could not open folder: {error}"))?;
    if !root.is_dir() {
        return Err(format!("not a folder: {}", root.display()));
    }

    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for relative_path in paths {
        let candidate = root.join(relative_path);
        let canonical = match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                errors.push(format!("{relative_path}: {error}"));
                continue;
            }
        };
        if !path_normalization::path_is_within_root(
            &canonical.display().to_string(),
            &root.display().to_string(),
        ) {
            errors.push(format!("{relative_path}: path is outside the selected folder"));
            continue;
        }
        if !canonical.is_file() {
            errors.push(format!("{relative_path}: file no longer exists"));
            continue;
        }

        match trash::delete(&canonical) {
            Ok(()) => removed.push(relative_path.clone()),
            Err(error) => errors.push(format!("{relative_path}: {error}")),
        }
    }

    Ok(DuplicateCleanupResult { removed, errors })
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
    fn cleanup_rejects_paths_outside_root() {
        let dir = setup_folder();
        let result = remove_duplicates(dir.path(), &["../outside.txt".into()]).expect("cleanup");
        assert!(result.removed.is_empty());
        assert_eq!(result.errors.len(), 1);
    }
}
