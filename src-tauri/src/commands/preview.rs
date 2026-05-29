use std::path::Path;

use std::sync::Arc;

use tauri::State;

use crate::diff::build_sync_plan;
use crate::models::{FolderPair, SyncPlan};
use crate::path_normalization;
use crate::persistence::Database;
use crate::scanner::scan_directory;
use crate::state::AppState;

fn scan_warnings_for_side(label: &str, skipped: u32) -> Option<String> {
    if skipped == 0 {
        None
    } else {
        Some(format!(
            "{label}: skipped {skipped} path(s) (permission denied or unreadable)"
        ))
    }
}

/// Core preview logic shared by the Tauri command and integration tests.
///
/// Uses `pair` for scan paths, filters, mode, and conflict policy (editor draft).
/// Snapshot history is loaded by `pair.id` only.
pub(crate) fn preview_pair_impl(db: &Database, pair: &FolderPair) -> Result<SyncPlan, String> {
    if pair.id.is_empty() {
        return Err("pair id required for preview".into());
    }

    let left_path = path_normalization::to_long_path(&pair.left_path);
    let right_path = path_normalization::to_long_path(&pair.right_path);

    let left_scan = scan_directory(Path::new(&left_path), &pair.filters)
        .map_err(|e| format!("scan left failed: {e}"))?;
    let right_scan = scan_directory(Path::new(&right_path), &pair.filters)
        .map_err(|e| format!("scan right failed: {e}"))?;

    let mut scan_warnings = Vec::new();
    if let Some(w) = scan_warnings_for_side("left", left_scan.skipped_entries) {
        scan_warnings.push(w);
    }
    if let Some(w) = scan_warnings_for_side("right", right_scan.skipped_entries) {
        scan_warnings.push(w);
    }

    let snapshot = db.latest_snapshot(&pair.id).map_err(|e| e.to_string())?;
    let snapshot_entries = snapshot.as_ref().map(|s| s.entries.as_slice());

    Ok(build_sync_plan(
        &pair.id,
        pair.mode,
        pair.conflict_policy,
        &left_scan.entries,
        &right_scan.entries,
        snapshot_entries,
        scan_warnings,
    ))
}

#[tauri::command]
pub fn preview_pair(pair: FolderPair, state: State<'_, Arc<AppState>>) -> Result<SyncPlan, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    preview_pair_impl(&db, &pair)
}

#[cfg(test)]
mod tests {
    use crate::models::{
        ConflictPolicy, FileEntry, Filters, FolderPair, Snapshot, SyncMode,
    };
    use crate::persistence::{new_pair_id, Database};
    use std::fs;
    use tempfile::TempDir;

    fn save_echo_pair(
        db: &Database,
        id: &str,
        left_dir: &std::path::Path,
        right_dir: &std::path::Path,
    ) {
        db.save_pair(&FolderPair {
            id: id.to_string(),
            name: "Test".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
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
        .expect("save");
    }

    #[test]
    fn preview_pair_impl_echo_plan_from_temp_folders() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Database::open(&data_dir.path().join("test.db")).expect("db");

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir left");
        fs::create_dir_all(&right_dir).expect("mkdir right");
        fs::write(left_dir.join("keep.txt"), "a").expect("write");
        fs::write(left_dir.join("sync.txt"), "b").expect("write");
        fs::write(right_dir.join("keep.txt"), "a").expect("write");
        fs::write(right_dir.join("remove.txt"), "gone").expect("write");

        let id = new_pair_id();
        save_echo_pair(&db, &id, &left_dir, &right_dir);
        let pair = db.get_pair(&id).expect("get").expect("pair");

        let plan = super::preview_pair_impl(&db, &pair).expect("preview");
        assert!(plan.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::CopyLeftToRight { path }
                    if path == "sync.txt"
            )
        }));
        assert!(plan.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::DeleteRight { path }
                    if path == "remove.txt"
            )
        }));
    }

    #[test]
    fn preview_pair_command_delegates_to_impl() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Database::open(&data_dir.path().join("test.db")).expect("db");

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("only-left.txt"), "x").expect("write");

        let id = new_pair_id();
        save_echo_pair(&db, &id, &left_dir, &right_dir);
        let pair = db.get_pair(&id).expect("get").expect("pair");

        let plan = super::preview_pair_impl(&db, &pair).expect("preview");
        assert!(plan.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::CopyLeftToRight { path }
                    if path == "only-left.txt"
            )
        }));
    }

    #[test]
    fn preview_does_not_modify_files() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Database::open(&data_dir.path().join("test.db")).expect("db");

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("a.txt"), "left").expect("write");
        fs::write(right_dir.join("b.txt"), "right").expect("write");

        let id = new_pair_id();
        db.save_pair(&FolderPair {
            id: id.clone(),
            name: "T".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::Ask,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        })
        .expect("save");

        let pair = db.get_pair(&id).expect("get").expect("pair");
        let _plan = super::preview_pair_impl(&db, &pair).expect("preview");

        assert!(right_dir.join("b.txt").exists());
        assert!(!right_dir.join("a.txt").exists());
    }

    #[test]
    fn preview_uses_snapshot_when_present() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Database::open(&data_dir.path().join("test.db")).expect("db");

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("both.txt"), "new-left").expect("write");
        fs::write(right_dir.join("both.txt"), "new-right").expect("write");

        let id = new_pair_id();
        db.save_pair(&FolderPair {
            id: id.clone(),
            name: "Sync".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: SyncMode::Synchronize,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::Ask,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        })
        .expect("save");

        db.save_snapshot(&Snapshot {
            id: uuid::Uuid::new_v4().to_string(),
            pair_id: id.clone(),
            captured_at: 1,
            entries: vec![FileEntry {
                relative_path: "both.txt".into(),
                size: 1,
                modified_secs: 1,
                is_dir: false,
                hash: None,
            }],
        })
        .expect("snapshot");

        let pair = db.get_pair(&id).expect("get").expect("pair");
        let plan = super::preview_pair_impl(&db, &pair).expect("preview");
        assert!(plan.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::Conflict { path, .. }
                    if path == "both.txt"
            )
        }));
    }

    #[test]
    fn preview_uses_passed_config_not_persisted_pair() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Database::open(&data_dir.path().join("test.db")).expect("db");

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("keep.txt"), "a").expect("write");
        fs::write(left_dir.join("skip.tmp"), "tmp").expect("write");

        let id = new_pair_id();
        save_echo_pair(&db, &id, &left_dir, &right_dir);

        let mut draft = db.get_pair(&id).expect("get").expect("pair");
        draft.filters = Filters {
            include: vec![],
            exclude: vec!["*.tmp".into()],
        };

        let plan = super::preview_pair_impl(&db, &draft).expect("preview");
        assert!(plan.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::CopyLeftToRight { path }
                    if path == "keep.txt"
            )
        }));
        assert!(!plan.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::CopyLeftToRight { path }
                    if path == "skip.tmp"
            )
        }));

        let persisted = super::preview_pair_impl(
            &db,
            &db.get_pair(&id).expect("get").expect("pair"),
        )
        .expect("preview");
        assert!(persisted.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::CopyLeftToRight { path }
                    if path == "skip.tmp"
            )
        }));
    }

}
