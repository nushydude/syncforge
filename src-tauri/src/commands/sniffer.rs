use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Emitter;

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

fn measure(path: &Path) -> FolderTotals {
    let Ok(metadata) = fs::metadata(path) else {
        return FolderTotals { size: 0, files: 0, folders: 0, skipped: 1 };
    };
    if metadata.is_file() {
        return FolderTotals { size: metadata.len(), files: 1, folders: 0, skipped: 0 };
    }

    let mut totals = FolderTotals { size: 0, files: 0, folders: 1, skipped: 0 };
    let Ok(children) = fs::read_dir(path) else {
        totals.skipped += 1;
        return totals;
    };
    for child in children {
        let Ok(child) = child else {
            totals.skipped += 1;
            continue;
        };
        let child_totals = measure(&child.path());
        totals.size = totals.size.saturating_add(child_totals.size);
        totals.files = totals.files.saturating_add(child_totals.files);
        totals.folders = totals.folders.saturating_add(child_totals.folders);
        totals.skipped = totals.skipped.saturating_add(child_totals.skipped);
    }
    totals
}

#[tauri::command]
pub async fn scan_folder_sizes(
    path: String,
    app: tauri::AppHandle,
) -> Result<FolderSizeResult, String> {
    tauri::async_runtime::spawn_blocking(move || scan_folder_sizes_blocking(path, app))
        .await
        .map_err(|error| format!("folder scan failed: {error}"))?
}

#[tauri::command]
pub fn rename_sniffer_item(path: String, new_name: String) -> Result<String, String> {
    let source = PathBuf::from(path.trim());
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
pub fn delete_sniffer_item(path: String) -> Result<(), String> {
    let target = PathBuf::from(path.trim());
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
        let metadata = match fs::metadata(&child_path) {
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
