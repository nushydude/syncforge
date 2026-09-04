use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;
use tauri::State;

use crate::run_coordinator::retry_pending_syncs;
use crate::state::{AppState, HeavyJobKind, HeavyJobPermit, WorkCoordinator, WorkRequest};

pub(crate) fn admit_sniffer(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<PathBuf>,
    writer: bool,
) -> Result<HeavyJobPermit, String> {
    coordinator.acquire_manual(WorkRequest::new(roots, writer, HeavyJobKind::Sniffer))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderSizeEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_folder: bool,
    pub child_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderSizeResult {
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub folders: u64,
    pub skipped: u64,
    pub entries: Vec<FolderSizeEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnifferProgress {
    pub completed: u64,
    pub total: u64,
    pub current: String,
}

struct FolderTotals {
    size: u64,
    files: u64,
    folders: u64,
    skipped: u64,
}

#[cfg(windows)]
fn is_name_surrogate_reparse_tag(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

#[cfg(windows)]
fn windows_reparse_tag(path: &Path) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};

    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe { FindFirstFileW(wide_path.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe { FindClose(handle) };
    Some(data.dwReserved0)
}

fn is_scan_link(path: &Path, metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            return false;
        }
        // Name-surrogate tags redirect traversal. Ordinary reparse files such as cloud
        // placeholders remain visible. Unknown directories are skipped conservatively.
        windows_reparse_tag(path).map(is_name_surrogate_reparse_tag).unwrap_or(metadata.is_dir())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

fn measure(path: &Path) -> FolderTotals {
    let mut totals = FolderTotals { size: 0, files: 0, folders: 0, skipped: 0 };
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            totals.skipped += 1;
            continue;
        };
        if is_scan_link(&path, &metadata) {
            totals.skipped += 1;
        } else if metadata.is_file() {
            totals.size = totals.size.saturating_add(metadata.len());
            totals.files += 1;
        } else if metadata.is_dir() {
            totals.folders += 1;
            match fs::read_dir(&path) {
                Ok(children) => {
                    for child in children {
                        match child {
                            Ok(child) => pending.push(child.path()),
                            Err(_) => totals.skipped += 1,
                        }
                    }
                }
                Err(_) => totals.skipped += 1,
            }
        } else {
            totals.skipped += 1;
        }
    }
    totals
}

#[tauri::command]
pub async fn scan_folder_sizes(
    path: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<FolderSizeResult, String> {
    let work_coordinator = Arc::clone(&state.work_coordinator);
    let job_root = PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|e| format!("could not open folder: {e}"))?;
    {
        let mut jobs = state.active_sniffer_jobs.lock().map_err(|e| e.to_string())?;
        if !jobs.is_empty() {
            return Err("a sniffer scan is already queued".into());
        }
        jobs.insert(job_root.clone());
    }
    let active_jobs = Arc::clone(&*state);
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = admit_sniffer(&work_coordinator, Vec::new(), false)?;
        let app_for_retry = app.clone();
        let result = scan_folder_sizes_blocking(path, app);
        if let Ok(mut jobs) = active_jobs.active_sniffer_jobs.lock() {
            jobs.remove(&job_root);
        }
        drop(_permit);
        retry_pending_syncs(app_for_retry, &active_jobs);
        result
    })
    .await
    .map_err(|error| format!("folder scan failed: {error}"))?
}

#[tauri::command]
pub fn rename_sniffer_item(
    path: String,
    new_name: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let source = PathBuf::from(path.trim());
    let root = source.canonicalize().unwrap_or_else(|_| source.clone());
    let _permit = admit_sniffer(&state.work_coordinator, vec![root], true)?;
    let name = new_name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("invalid new name".to_string());
    }
    if !source.exists() {
        return Err("item no longer exists".to_string());
    }
    let destination =
        source.parent().ok_or_else(|| "could not determine item folder".to_string())?.join(name);
    fs::rename(&source, &destination).map_err(|error| format!("could not rename item: {error}"))?;
    Ok(destination.display().to_string())
}

#[tauri::command]
pub fn delete_sniffer_item(path: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let target = PathBuf::from(path.trim());
    let root = target.canonicalize().unwrap_or_else(|_| target.clone());
    let _permit = admit_sniffer(&state.work_coordinator, vec![root], true)?;
    let metadata =
        fs::metadata(&target).map_err(|error| format!("could not access item: {error}"))?;
    if metadata.is_dir() {
        fs::remove_dir_all(&target).map_err(|error| format!("could not delete folder: {error}"))?;
    } else {
        fs::remove_file(&target).map_err(|error| format!("could not delete file: {error}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn show_sniffer_item_properties(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{SHObjectProperties, SHOP_FILEPATH};

        let trimmed = path.trim();
        let shell_path = if let Some(stripped) = trimmed.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{}", stripped)
        } else if let Some(stripped) = trimmed.strip_prefix("\\\\?\\") {
            stripped.to_string()
        } else {
            trimmed.to_string()
        };
        let wide_path: Vec<u16> =
            std::ffi::OsStr::new(&shell_path).encode_wide().chain(std::iter::once(0)).collect();
        let result = unsafe {
            SHObjectProperties(
                std::ptr::null_mut(),
                SHOP_FILEPATH as u32,
                wide_path.as_ptr(),
                std::ptr::null(),
            )
        };
        if result == 0 {
            return Err(format!(
                "Windows could not open Properties: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("file properties are only supported on Windows".to_string())
    }
}

fn scan_folder_sizes_blocking(
    path: String,
    app: tauri::AppHandle,
) -> Result<FolderSizeResult, String> {
    let requested = PathBuf::from(path.trim());
    let root =
        requested.canonicalize().map_err(|error| format!("could not open folder: {error}"))?;
    if !root.is_dir() {
        return Err(format!("not a folder: {}", root.display()));
    }

    let children: Vec<_> =
        fs::read_dir(&root).map_err(|error| format!("could not read folder: {error}"))?.collect();
    let total_children = children.len() as u64;
    let _ = app.emit(
        "sniffer://progress",
        SnifferProgress {
            completed: 0,
            total: total_children,
            current: root.display().to_string(),
        },
    );
    let mut total = FolderTotals { size: 0, files: 0, folders: 1, skipped: 0 };
    let mut entries = Vec::new();
    for (index, child_result) in children.into_iter().enumerate() {
        let completed = index as u64 + 1;
        let child = match child_result {
            Ok(child) => child,
            Err(_) => {
                total.skipped += 1;
                let _ = app.emit(
                    "sniffer://progress",
                    SnifferProgress {
                        completed,
                        total: total_children,
                        current: "Unreadable item".to_string(),
                    },
                );
                continue;
            }
        };
        let child_path = child.path();
        let metadata = match fs::symlink_metadata(&child_path) {
            Ok(metadata) => metadata,
            Err(_) => {
                total.skipped += 1;
                let _ = app.emit(
                    "sniffer://progress",
                    SnifferProgress {
                        completed,
                        total: total_children,
                        current: child_path.display().to_string(),
                    },
                );
                continue;
            }
        };
        let child_totals = measure(&child_path);
        total.size = total.size.saturating_add(child_totals.size);
        total.files = total.files.saturating_add(child_totals.files);
        total.folders = total.folders.saturating_add(child_totals.folders);
        total.skipped = total.skipped.saturating_add(child_totals.skipped);
        if !is_scan_link(&child_path, &metadata) {
            entries.push(FolderSizeEntry {
                name: child.file_name().to_string_lossy().into_owned(),
                path: child_path.display().to_string(),
                size: child_totals.size,
                is_folder: metadata.is_dir(),
                child_count: if metadata.is_dir() {
                    child_totals.files + child_totals.folders.saturating_sub(1)
                } else {
                    0
                },
            });
        }
        let _ = app.emit(
            "sniffer://progress",
            SnifferProgress {
                completed,
                total: total_children,
                current: child_path.display().to_string(),
            },
        );
    }
    entries.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));

    Ok(FolderSizeResult {
        path: root.display().to_string(),
        size: total.size,
        files: total.files,
        folders: total.folders.saturating_sub(1),
        skipped: total.skipped,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_nested_files_and_reports_missing_paths() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("first"), b"abc").unwrap();
        fs::write(root.path().join("nested/second"), b"12345").unwrap();
        let totals = measure(root.path());
        assert_eq!((totals.size, totals.files, totals.folders, totals.skipped), (8, 2, 2, 0));
        assert_eq!(measure(&root.path().join("missing")).skipped, 1);
    }

    #[test]
    fn skips_directory_links_that_cycle_back_to_root() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file"), b"abc").unwrap();
        let link = root.path().join("cycle");
        #[cfg(windows)]
        {
            // Junction creation does not require the symlink privilege on Windows.
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(root.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path(), &link).unwrap();
        let totals = measure(root.path());
        assert_eq!((totals.size, totals.files, totals.folders, totals.skipped), (3, 1, 1, 1));
    }

    #[cfg(windows)]
    #[test]
    fn distinguishes_name_surrogate_tags_from_cloud_reparse_tags() {
        assert!(is_name_surrogate_reparse_tag(0xA000_0003)); // mount point
        assert!(is_name_surrogate_reparse_tag(0xA000_000C)); // symbolic link
        assert!(!is_name_surrogate_reparse_tag(0x9000_001A)); // cloud placeholder
    }
}
