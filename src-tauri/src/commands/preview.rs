use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::State;

use crate::diff::{build_sync_plan, DiffOptions};
use crate::models::{
    FileEntry, FolderPair, PreviewActionPage, PreviewSummary, SyncAction, SyncPlan,
};
use crate::path_normalization;
use crate::persistence::DatabaseHandle;
use crate::scanner::{assert_destructive_scan_allowed, scan_directory, ScanIntegrity};
use crate::state::{
    canonical_job_roots, AppState, HeavyJobKind, HeavyJobPermit, WorkCoordinator, WorkRequest,
    PREVIEW_PAGE_MAX,
};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn config_fingerprint(pair: &FolderPair) -> String {
    let encoded = serde_json::to_vec(pair).unwrap_or_default();
    blake3::hash(&encoded).to_hex().to_string()
}

fn preview_summary(
    plan: &SyncPlan,
    plan_id: String,
    fingerprint: String,
    created_at: i64,
) -> PreviewSummary {
    let mut action_counts = std::collections::HashMap::new();
    let mut conflict_count = 0;
    for action in &plan.actions {
        let kind = crate::engine::action_kind(action).to_string();
        *action_counts.entry(kind).or_insert(0) += 1;
        if matches!(action, SyncAction::Conflict { .. }) {
            conflict_count += 1;
        }
    }
    let first_page = plan.actions.iter().take(PREVIEW_PAGE_MAX).cloned().collect();
    PreviewSummary {
        plan_id,
        pair_id: plan.pair_id.clone(),
        config_fingerprint: fingerprint,
        created_at,
        action_counts,
        action_count: plan.actions.len() as u32,
        conflict_count,
        next_cursor: (plan.actions.len() > PREVIEW_PAGE_MAX).then_some(PREVIEW_PAGE_MAX),
        first_page,
        scanned_left: plan.scanned_left,
        scanned_right: plan.scanned_right,
        scan_skipped_left: plan.scan_skipped_left,
        scan_skipped_right: plan.scan_skipped_right,
        scan_warnings: plan.scan_warnings.clone(),
        requires_attention: plan.requires_attention,
    }
}

pub(crate) fn admit_preview(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<PathBuf>,
) -> Result<HeavyJobPermit, String> {
    coordinator.acquire_manual(WorkRequest::new(roots, false, HeavyJobKind::Preview))
}

/// Core preview logic shared by the Tauri command, watcher, scheduler, and tests.
///
/// Uses `pair` for scan paths, filters, mode, and conflict policy (editor draft).
/// `snapshot_entries` must be loaded under a short DB lock before calling this function.
pub(crate) fn preview_pair_impl(
    pair: &FolderPair,
    snapshot_entries: Option<&[FileEntry]>,
) -> Result<SyncPlan, String> {
    build_preview(pair, snapshot_entries).map(|(plan, _, _)| plan)
}

pub(crate) fn build_preview(
    pair: &FolderPair,
    snapshot_entries: Option<&[FileEntry]>,
) -> Result<(SyncPlan, Vec<FileEntry>, Vec<FileEntry>), String> {
    if pair.id.is_empty() {
        return Err("pair id required for preview".into());
    }

    let left_path = path_normalization::to_long_path(&pair.left_path);
    let right_path = path_normalization::to_long_path(&pair.right_path);

    let mut left_scan = scan_directory(Path::new(&left_path), &pair.filters)
        .map_err(|e| format!("scan left failed: {e}"))?;
    let mut right_scan = scan_directory(Path::new(&right_path), &pair.filters)
        .map_err(|e| format!("scan right failed: {e}"))?;

    assert_destructive_scan_allowed(pair.mode, &left_scan, &right_scan)?;

    let scan = ScanIntegrity::from_sides(&left_scan, &right_scan);

    let plan = build_sync_plan(
        &pair.id,
        pair.mode,
        pair.conflict_policy,
        &left_scan.entries,
        &right_scan.entries,
        snapshot_entries,
        scan,
        DiffOptions {
            left_root: Some(Path::new(&left_path).to_path_buf()),
            right_root: Some(Path::new(&right_path).to_path_buf()),
            ..DiffOptions::default()
        },
    );
    if plan.requires_attention
        && matches!(pair.mode, crate::models::SyncMode::Echo | crate::models::SyncMode::Synchronize)
    {
        return Err("Preview requires attention: a content hash could not be read safely. Fix access and preview again.".into());
    }
    let mut preview_hash_bytes = 0;
    let mut preview_hash_files = 0;
    populate_preview_hashes_for_actions(
        &mut left_scan.entries,
        Path::new(&left_path),
        &plan.actions,
        &mut preview_hash_bytes,
        &mut preview_hash_files,
    )?;
    populate_preview_hashes_for_actions(
        &mut right_scan.entries,
        Path::new(&right_path),
        &plan.actions,
        &mut preview_hash_bytes,
        &mut preview_hash_files,
    )?;
    Ok((plan, left_scan.entries, right_scan.entries))
}

fn populate_preview_hashes_for_actions(
    entries: &mut [FileEntry],
    root: &Path,
    actions: &[SyncAction],
    hashed_bytes: &mut u64,
    hashed_files: &mut usize,
) -> Result<(), String> {
    const PREVIEW_HASH_MAX_BYTES: u64 = 512 * 1024 * 1024;
    const PREVIEW_SUBTREE_HASH_MAX_BYTES: u64 = 512 * 1024 * 1024;
    const PREVIEW_SUBTREE_HASH_MAX_FILES: usize = 10_000;
    let paths: HashSet<&str> = actions
        .iter()
        .map(|action| match action {
            SyncAction::CopyLeftToRight { path }
            | SyncAction::CopyRightToLeft { path }
            | SyncAction::DeleteLeft { path }
            | SyncAction::DeleteRight { path }
            | SyncAction::CreateDirLeft { path }
            | SyncAction::CreateDirRight { path }
            | SyncAction::Conflict { path, .. }
            | SyncAction::Skip { path, .. } => path.as_str(),
        })
        .collect();
    let deleted_directories: HashSet<&str> = actions
        .iter()
        .filter_map(|action| match action {
            SyncAction::DeleteLeft { path } | SyncAction::DeleteRight { path } => {
                Some(path.as_str())
            }
            _ => None,
        })
        .collect();
    for entry in entries.iter_mut() {
        let under_deleted_directory = entry
            .relative_path
            .split('/')
            .scan(String::new(), |prefix, part| {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                Some(deleted_directories.contains(prefix.as_str()))
            })
            .any(|is_match| is_match);
        let is_selected = paths.contains(entry.relative_path.as_str()) || under_deleted_directory;
        if !is_selected || entry.is_dir || entry.size == 0 {
            continue;
        }
        if entry.size > PREVIEW_HASH_MAX_BYTES {
            return Err(format!(
                "preview cannot safely validate large file at {}; reduce the workload or preview again",
                entry.relative_path
            ));
        }
        *hashed_bytes = (*hashed_bytes).saturating_add(entry.size);
        *hashed_files += 1;
        if *hashed_bytes > PREVIEW_SUBTREE_HASH_MAX_BYTES
            || *hashed_files > PREVIEW_SUBTREE_HASH_MAX_FILES
        {
            return Err("preview content exceeds the bounded validation budget".into());
        }
        let path = root.join(entry.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        entry.hash =
            Some(crate::hashing::hash_file(&path).map_err(|error| {
                format!("content hash failed at {}: {error}", entry.relative_path)
            })?);
    }
    Ok(())
}

fn load_preview_snapshot(
    db: &dyn DatabaseHandle,
    pair_id: &str,
) -> Result<Option<Vec<FileEntry>>, String> {
    db.latest_snapshot(pair_id)
        .map_err(|e| e.to_string())
        .map(|snapshot| snapshot.map(|s| s.entries))
}

#[tauri::command]
pub async fn preview_pair(
    pair: FolderPair,
    state: State<'_, Arc<AppState>>,
) -> Result<PreviewSummary, String> {
    if pair.id.is_empty() {
        return Err("pair id required for preview".into());
    }

    let work_coordinator = Arc::clone(&state.work_coordinator);
    let app_state = Arc::clone(&state);
    let db = Arc::clone(&state.db);
    let roots = canonical_job_roots(&[&pair.left_path, &pair.right_path]);
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = admit_preview(&work_coordinator, roots)?;
        let snapshot_entries = load_preview_snapshot(db.as_ref(), &pair.id)?;
        let (plan, left_preconditions, right_preconditions) =
            build_preview(&pair, snapshot_entries.as_deref())?;
        let created_at = now_millis();
        let fingerprint = config_fingerprint(&pair);
        let mut summary = preview_summary(&plan, String::new(), fingerprint.clone(), created_at);
        let mut previews = app_state.preview_plans.lock().map_err(|e| e.to_string())?;
        let plan_id = previews.insert(
            pair.id.clone(),
            fingerprint.clone(),
            plan,
            left_preconditions,
            right_preconditions,
            created_at,
        )?;
        summary.plan_id = plan_id;
        Ok(summary)
    })
    .await
    .map_err(|e| format!("preview task failed: {e}"))?
}

#[tauri::command]
pub fn get_preview_actions(
    plan_id: String,
    pair: FolderPair,
    cursor: Option<usize>,
    limit: Option<usize>,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<PreviewActionPage, String> {
    let limit = limit.unwrap_or(PREVIEW_PAGE_MAX).min(PREVIEW_PAGE_MAX);
    let fingerprint = config_fingerprint(&pair);
    state.preview_plans.lock().map_err(|e| e.to_string())?.get_page(
        &plan_id,
        &pair.id,
        &fingerprint,
        cursor.unwrap_or(0),
        limit,
        now_millis(),
    )
}

#[tauri::command]
pub fn get_preview_conflicts(
    plan_id: String,
    pair: FolderPair,
    cursor: Option<usize>,
    limit: Option<usize>,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<PreviewActionPage, String> {
    let fingerprint = config_fingerprint(&pair);
    state.preview_plans.lock().map_err(|e| e.to_string())?.get_conflicts_page(
        &plan_id,
        &pair.id,
        &fingerprint,
        cursor.unwrap_or(0),
        limit.unwrap_or(PREVIEW_PAGE_MAX),
        now_millis(),
    )
}

#[cfg(test)]
mod tests {
    use crate::models::{ConflictPolicy, FileEntry, Filters, FolderPair, Snapshot, SyncMode};
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

    fn preview_with_db(
        db: &Database,
        pair: &FolderPair,
    ) -> Result<crate::models::SyncPlan, String> {
        let snapshot_entries = super::load_preview_snapshot(db, &pair.id)?;
        super::preview_pair_impl(pair, snapshot_entries.as_deref())
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

        let plan = preview_with_db(&db, &pair).expect("preview");
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

        let plan = preview_with_db(&db, &pair).expect("preview");
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
        let _plan = preview_with_db(&db, &pair).expect("preview");

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
                modified_nanos: 0,
                is_dir: false,
                hash: None,
                deleted: false,
            }],
        })
        .expect("snapshot");

        let pair = db.get_pair(&id).expect("get").expect("pair");
        let plan = preview_with_db(&db, &pair).expect("preview");
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
        draft.filters = Filters { include: vec![], exclude: vec!["*.tmp".into()] };

        let plan = preview_with_db(&db, &draft).expect("preview");
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

        let persisted =
            preview_with_db(&db, &db.get_pair(&id).expect("get").expect("pair")).expect("preview");
        assert!(persisted.actions.iter().any(|a| {
            matches!(
                a,
                crate::models::SyncAction::CopyLeftToRight { path }
                    if path == "skip.tmp"
            )
        }));
    }

    #[test]
    #[cfg(all(unix, not(target_os = "linux")))]
    fn preview_echo_refuses_when_scan_skips_paths() {
        use std::os::unix::fs::PermissionsExt;

        let data_dir = TempDir::new().expect("tempdir");
        let db = Database::open(&data_dir.path().join("test.db")).expect("db");

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir left");
        fs::create_dir_all(&right_dir).expect("mkdir right");
        fs::write(left_dir.join("visible.txt"), "ok").expect("write");
        let secret = left_dir.join("secret.txt");
        fs::write(&secret, "hidden").expect("write");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).expect("chmod");
        fs::write(right_dir.join("orphan.txt"), "gone").expect("write");

        let id = new_pair_id();
        save_echo_pair(&db, &id, &left_dir, &right_dir);
        let pair = db.get_pair(&id).expect("get").expect("pair");

        let err = preview_with_db(&db, &pair).expect_err("preview must fail");
        assert!(err.contains("Cannot run Echo sync"));
        assert!(err.contains("skipped"));

        fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).expect("restore");
    }
}
