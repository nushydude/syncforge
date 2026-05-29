use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use jwalk::WalkDir;

use crate::models::{FileEntry, Filters};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutput {
    pub entries: Vec<FileEntry>,
    pub skipped_entries: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("invalid glob pattern: {0}")]
    InvalidPattern(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ScanResult = std::result::Result<ScanOutput, ScanError>;

/// Scans `root` in parallel and returns filtered entries keyed by relative path.
pub fn scan_directory(root: &Path, filters: &Filters) -> ScanResult {
    let root = root
        .canonicalize()
        .map_err(|_| ScanError::NotFound(root.display().to_string()))?;

    if !root.is_dir() {
        return Err(ScanError::NotFound(root.display().to_string()));
    }

    let include_set = build_glob_set(&filters.include)?;
    let exclude_set = build_glob_set(&filters.exclude)?;

    let mut entries = Vec::new();
    let mut skipped_entries = 0u32;

    for entry in WalkDir::new(&root).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                skipped_entries += 1;
                continue;
            }
        };

        let path = entry.path();
        if path == root {
            continue;
        }

        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let relative_path = to_relative_string(relative);
        if relative_path.is_empty() {
            continue;
        }

        if !matches_filters(
            &relative_path,
            include_set.as_ref(),
            exclude_set.as_ref(),
        ) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                skipped_entries += 1;
                continue;
            }
        };

        let modified_secs = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        entries.push(FileEntry {
            relative_path,
            size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            modified_secs,
            is_dir: metadata.is_dir(),
            hash: None,
        });
    }

    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(ScanOutput {
        entries,
        skipped_entries,
    })
}

/// Returns true when `relative_path` passes include/exclude glob rules.
pub fn matches_filters(
    relative_path: &str,
    include: Option<&GlobSet>,
    exclude: Option<&GlobSet>,
) -> bool {
    let path = relative_path.replace('\\', "/");

    if let Some(include) = include {
        if !include.is_match(&path) {
            return false;
        }
    }

    if let Some(exclude) = exclude {
        if exclude.is_match(&path) {
            return false;
        }
    }

    true
}

fn build_glob_set(patterns: &[String]) -> std::result::Result<Option<globset::GlobSet>, ScanError> {
    if patterns.is_empty() {
        return Ok(None);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let normalized = normalize_pattern(pattern);
        let glob = Glob::new(&normalized)
            .map_err(|e| ScanError::InvalidPattern(format!("{pattern}: {e}")))?;
        builder.add(glob);
    }
    Ok(Some(
        builder
            .build()
            .map_err(|e| ScanError::InvalidPattern(e.to_string()))?,
    ))
}

fn normalize_pattern(pattern: &str) -> String {
    let trimmed = pattern.trim();
    if trimmed.contains('/') || trimmed.contains('\\') {
        trimmed.replace('\\', "/")
    } else {
        format!("**/{}", trimmed)
    }
}

fn to_relative_string(relative: &Path) -> String {
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn matches_filter_rules(relative_path: &str, filters: &Filters) -> bool {
        let include = build_glob_set(&filters.include).ok().flatten();
        let exclude = build_glob_set(&filters.exclude).ok().flatten();
        matches_filters(relative_path, include.as_ref(), exclude.as_ref())
    }

    #[test]
    fn matches_include_and_exclude() {
        let filters = Filters {
            include: vec!["*.txt".into()],
            exclude: vec!["*.tmp".into()],
        };
        assert!(matches_filter_rules("notes.txt", &filters));
        assert!(matches_filter_rules("sub/notes.txt", &filters));
        assert!(!matches_filter_rules("notes.tmp", &filters));
        assert!(!matches_filter_rules("image.png", &filters));
    }

    #[test]
    fn empty_include_matches_all_non_excluded() {
        let filters = Filters {
            include: vec![],
            exclude: vec!["*.tmp".into()],
        };
        assert!(matches_filter_rules("any.bin", &filters));
        assert!(!matches_filter_rules("skip.tmp", &filters));
    }

    #[test]
    fn scan_directory_collects_files_and_dirs() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("sub")).expect("mkdir");
        fs::write(root.join("a.txt"), "hello").expect("write");
        fs::write(root.join("sub/b.txt"), "world").expect("write");
        fs::write(root.join("skip.tmp"), "x").expect("write");

        let filters = Filters {
            include: vec![],
            exclude: vec!["*.tmp".into()],
        };
        let output = scan_directory(root, &filters).expect("scan");
        let paths: Vec<_> = output
            .entries
            .iter()
            .map(|e| e.relative_path.as_str())
            .collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"sub"));
        assert!(paths.contains(&"sub/b.txt"));
        assert!(!paths.contains(&"skip.tmp"));
        assert_eq!(output.skipped_entries, 0);
    }
}
