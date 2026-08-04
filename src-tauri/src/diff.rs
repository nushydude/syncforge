use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::hashing;
use crate::models::{
    ConflictPolicy, ConflictResolution, FileEntry, SyncAction, SyncMode, SyncPlan,
};
use crate::scanner::ScanIntegrity;

/// Options for comparing file entries during sync planning.
#[derive(Debug, Clone)]
pub struct DiffOptions {
    pub left_root: Option<PathBuf>,
    pub right_root: Option<PathBuf>,
    /// When metadata matches, hash file contents to detect same-second edits.
    pub content_hash_compare: bool,
    /// Maximum file size (bytes) eligible for content hashing (exclusive upper bound).
    pub content_hash_max_bytes: u64,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            left_root: None,
            right_root: None,
            content_hash_compare: true,
            content_hash_max_bytes: 50 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum EntrySide {
    Left,
    Right,
    Snapshot,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct HashKey {
    side: EntrySide,
    relative_path: String,
    size: u64,
    modified_secs: i64,
    modified_nanos: u32,
}

struct DiffContext<'a> {
    options: &'a DiffOptions,
    hashes: RefCell<HashMap<HashKey, Result<String, String>>>,
    hash_warnings: RefCell<Vec<String>>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_sync_plan(
    pair_id: &str,
    mode: SyncMode,
    conflict_policy: ConflictPolicy,
    left: &[FileEntry],
    right: &[FileEntry],
    snapshot: Option<&[FileEntry]>,
    scan: ScanIntegrity,
    diff_options: DiffOptions,
) -> SyncPlan {
    let ctx = DiffContext {
        options: &diff_options,
        hashes: RefCell::new(HashMap::new()),
        hash_warnings: RefCell::new(Vec::new()),
    };
    let mut actions = Vec::new();
    let empty_snapshot = [];
    let snapshot = snapshot.unwrap_or(&empty_snapshot);

    for (path, l, r, s) in ThreeWayMerge::new(left, right, snapshot) {
        match mode {
            SyncMode::Echo => {
                // Echo mirrors left → right; snapshot is not used for planning.
                plan_echo(&mut actions, path, l, r, &ctx);
            }
            SyncMode::Contribute => {
                plan_contribute(&mut actions, path, l, r, &ctx);
            }
            SyncMode::Synchronize => {
                plan_synchronize(&mut actions, path, l, r, s, conflict_policy, &ctx);
            }
        }
    }

    ensure_parent_dirs(&mut actions, mode, left, right);
    collapse_redundant_directory_deletes(&mut actions, left, right);
    sort_actions(&mut actions);

    let hash_warnings = ctx.hash_warnings.borrow().clone();
    SyncPlan {
        pair_id: pair_id.to_string(),
        actions,
        scanned_left: left.len() as u32,
        scanned_right: right.len() as u32,
        scan_skipped_left: scan.skipped_left,
        scan_skipped_right: scan.skipped_right,
        scan_warnings: scan.warnings.into_iter().chain(hash_warnings.iter().cloned()).collect(),
        requires_attention: scan.requires_attention || !hash_warnings.is_empty(),
    }
}

fn plan_echo(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: Option<&FileEntry>,
    right: Option<&FileEntry>,
    ctx: &DiffContext<'_>,
) {
    match (left, right) {
        (Some(l), None) if l.is_dir => {
            actions.push(SyncAction::CreateDirRight { path: path.to_string() });
        }
        (Some(l), None) if !l.is_dir => {
            actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
        }
        (Some(l), Some(r))
            if entries_differ(l, EntrySide::Left, r, EntrySide::Right, ctx) && !l.is_dir =>
        {
            actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
        }
        (Some(l), Some(r)) if l.is_dir && !r.is_dir => {
            actions.push(SyncAction::CreateDirRight { path: path.to_string() });
        }
        (None, Some(_)) => {
            actions.push(SyncAction::DeleteRight { path: path.to_string() });
        }
        _ => {}
    }
}

fn plan_contribute(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: Option<&FileEntry>,
    right: Option<&FileEntry>,
    ctx: &DiffContext<'_>,
) {
    match (left, right) {
        (Some(l), None) if l.is_dir => {
            actions.push(SyncAction::CreateDirRight { path: path.to_string() });
        }
        (Some(l), None) if !l.is_dir => {
            actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
        }
        (Some(l), Some(r))
            if entries_differ(l, EntrySide::Left, r, EntrySide::Right, ctx) && !l.is_dir =>
        {
            actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
        }
        (Some(l), Some(r)) if l.is_dir && !r.is_dir => {
            actions.push(SyncAction::CreateDirRight { path: path.to_string() });
        }
        (None, Some(_)) => {
            actions.push(SyncAction::Skip {
                path: path.to_string(),
                reason: "contribute mode does not delete".into(),
            });
        }
        _ => {}
    }
}

fn plan_synchronize(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: Option<&FileEntry>,
    right: Option<&FileEntry>,
    snapshot: Option<&FileEntry>,
    conflict_policy: ConflictPolicy,
    ctx: &DiffContext<'_>,
) {
    match (left, right) {
        (Some(l), None) if l.is_dir => {
            actions.push(SyncAction::CreateDirRight { path: path.to_string() });
        }
        (Some(l), None) if !l.is_dir => {
            plan_one_sided_file(actions, path, l, true, snapshot, conflict_policy, ctx);
        }
        (None, Some(r)) if r.is_dir => {
            actions.push(SyncAction::CreateDirLeft { path: path.to_string() });
        }
        (None, Some(r)) if !r.is_dir => {
            plan_one_sided_file(actions, path, r, false, snapshot, conflict_policy, ctx);
        }
        (Some(l), Some(r)) if l.is_dir && r.is_dir => {}
        (Some(l), Some(r)) if !l.is_dir && !r.is_dir => {
            if !entries_differ(l, EntrySide::Left, r, EntrySide::Right, ctx) {
                return;
            }
            if snapshot_changed_both(l, r, snapshot, ctx) || snapshot.is_none() {
                apply_conflict_policy(actions, path, l, r, conflict_policy);
            } else {
                push_newer_wins_copy(actions, path, l, r);
            }
        }
        (Some(l), Some(r)) if l.is_dir && !r.is_dir => {
            actions.push(SyncAction::CreateDirRight { path: path.to_string() });
        }
        (Some(l), Some(r)) if !l.is_dir && r.is_dir => {
            actions.push(SyncAction::CreateDirLeft { path: path.to_string() });
        }
        _ => {}
    }
}

/// File exists on one side only. Uses snapshot to tell new file vs delete vs modify+delete conflict.
fn plan_one_sided_file(
    actions: &mut Vec<SyncAction>,
    path: &str,
    present: &FileEntry,
    present_is_left: bool,
    snapshot: Option<&FileEntry>,
    conflict_policy: ConflictPolicy,
    ctx: &DiffContext<'_>,
) {
    match snapshot {
        None => {
            if present_is_left {
                actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
            } else {
                actions.push(SyncAction::CopyRightToLeft { path: path.to_string() });
            }
        }
        Some(snap)
            if entries_match(
                present,
                if present_is_left { EntrySide::Left } else { EntrySide::Right },
                snap,
                EntrySide::Snapshot,
                ctx,
            ) =>
        {
            if present_is_left {
                actions.push(SyncAction::DeleteRight { path: path.to_string() });
            } else {
                actions.push(SyncAction::DeleteLeft { path: path.to_string() });
            }
        }
        Some(snap) => {
            let (left, right) = if present_is_left {
                let mut deleted = snap.clone();
                deleted.deleted = true;
                (present.clone(), deleted)
            } else {
                let mut deleted = snap.clone();
                deleted.deleted = true;
                (deleted, present.clone())
            };
            apply_conflict_policy(actions, path, &left, &right, conflict_policy);
        }
    }
}

fn entries_match(
    a: &FileEntry,
    a_side: EntrySide,
    b: &FileEntry,
    b_side: EntrySide,
    ctx: &DiffContext<'_>,
) -> bool {
    !entries_differ(a, a_side, b, b_side, ctx)
}

fn snapshot_changed_both(
    left: &FileEntry,
    right: &FileEntry,
    snapshot: Option<&FileEntry>,
    ctx: &DiffContext<'_>,
) -> bool {
    let Some(snap) = snapshot else {
        return false;
    };
    entries_differ(left, EntrySide::Left, snap, EntrySide::Snapshot, ctx)
        && entries_differ(right, EntrySide::Right, snap, EntrySide::Snapshot, ctx)
}

fn apply_conflict_policy(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: &FileEntry,
    right: &FileEntry,
    policy: ConflictPolicy,
) {
    match policy {
        ConflictPolicy::Ask => {
            actions.push(SyncAction::Conflict {
                path: path.to_string(),
                left: left.clone(),
                right: right.clone(),
            });
        }
        ConflictPolicy::NewerWins => {
            if left.deleted {
                // A tombstone has no trustworthy deletion timestamp. Preserve
                // the live file rather than allowing a historical snapshot
                // time to delete a newer surviving copy.
                actions.push(SyncAction::CopyRightToLeft { path: path.to_string() });
            } else if right.deleted {
                actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
            } else {
                push_newer_wins_copy(actions, path, left, right);
            }
        }
        ConflictPolicy::Left => {
            actions.push(resolve_conflict_action(ConflictResolution::Left, path, left, right));
        }
        ConflictPolicy::Right => {
            actions.push(resolve_conflict_action(ConflictResolution::Right, path, left, right));
        }
        ConflictPolicy::KeepBoth => {
            actions.push(SyncAction::Skip {
                path: path.to_string(),
                reason: "keep both (conflict policy)".into(),
            });
        }
    }
}

fn push_newer_wins_copy(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: &FileEntry,
    right: &FileEntry,
) {
    match compare_newer(left, right) {
        Some(true) => {
            actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
        }
        Some(false) => {
            actions.push(SyncAction::CopyRightToLeft { path: path.to_string() });
        }
        None => {
            // Equal mtime (sec + nanos) and hash tie: prefer left.
            actions.push(SyncAction::CopyLeftToRight { path: path.to_string() });
        }
    }
}

/// Returns `Some(true)` when left is newer, `Some(false)` when right is newer, `None` on tie.
fn compare_newer(left: &FileEntry, right: &FileEntry) -> Option<bool> {
    if left.modified_secs > right.modified_secs {
        return Some(true);
    }
    if right.modified_secs > left.modified_secs {
        return Some(false);
    }
    if left.modified_nanos > right.modified_nanos {
        return Some(true);
    }
    if right.modified_nanos > left.modified_nanos {
        return Some(false);
    }
    if left.size != right.size {
        // Same second but different size: legacy tie-break (prefer left).
        return Some(true);
    }
    if let (Some(lh), Some(rh)) = (&left.hash, &right.hash) {
        if lh != rh {
            return None;
        }
    }
    None
}

fn ensure_parent_dirs(
    actions: &mut Vec<SyncAction>,
    mode: SyncMode,
    left: &[FileEntry],
    right: &[FileEntry],
) {
    let mut planned_right = HashSet::new();
    let mut planned_left = HashSet::new();
    let mut additions = Vec::new();

    for action in actions.iter() {
        match action {
            SyncAction::CopyLeftToRight { path } | SyncAction::CreateDirRight { path } => {
                for parent in parent_paths(path) {
                    if !contains_entry(right, &parent)
                        && contains_directory(left, &parent)
                        && planned_right.insert(parent.clone())
                    {
                        additions.push(SyncAction::CreateDirRight { path: parent });
                    }
                }
            }
            SyncAction::CopyRightToLeft { path } | SyncAction::CreateDirLeft { path } => {
                for parent in parent_paths(path) {
                    if !contains_entry(left, &parent)
                        && contains_directory(right, &parent)
                        && planned_left.insert(parent.clone())
                    {
                        additions.push(SyncAction::CreateDirLeft { path: parent });
                    }
                }
            }
            _ => {}
        }
    }

    if mode == SyncMode::Synchronize {
        actions.extend(additions);
    } else {
        actions.extend(
            additions
                .into_iter()
                .filter(|action| matches!(action, SyncAction::CreateDirRight { .. })),
        );
    }
}

fn contains_entry(entries: &[FileEntry], path: &str) -> bool {
    entries.binary_search_by(|entry| entry.relative_path.as_str().cmp(path)).is_ok()
}

fn contains_directory(entries: &[FileEntry], path: &str) -> bool {
    entries
        .binary_search_by(|entry| entry.relative_path.as_str().cmp(path))
        .ok()
        .is_some_and(|index| entries[index].is_dir)
}

fn parent_paths(path: &str) -> Vec<String> {
    let mut parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 1 {
        return vec![];
    }
    parts.pop();
    let mut parents = Vec::new();
    for i in 1..=parts.len() {
        parents.push(parts[..i].join("/"));
    }
    parents
}

fn sort_actions(actions: &mut [SyncAction]) {
    actions.sort_by(|a, b| action_path(a).cmp(action_path(b)));
}

fn collapse_redundant_directory_deletes(
    actions: &mut Vec<SyncAction>,
    left: &[FileEntry],
    right: &[FileEntry],
) {
    let mut deleted_dirs = HashSet::new();
    actions.retain(|action| {
        let (path, side, is_dir) = match action {
            SyncAction::DeleteLeft { path } => {
                (path, EntrySide::Left, contains_directory(left, path))
            }
            SyncAction::DeleteRight { path } => {
                (path, EntrySide::Right, contains_directory(right, path))
            }
            _ => return true,
        };
        if parent_paths(path).into_iter().any(|parent| deleted_dirs.contains(&(side, parent))) {
            return false;
        }
        if is_dir {
            deleted_dirs.insert((side, path.clone()));
        }
        true
    });
}

fn action_path(action: &SyncAction) -> &str {
    match action {
        SyncAction::CopyLeftToRight { path }
        | SyncAction::CopyRightToLeft { path }
        | SyncAction::DeleteLeft { path }
        | SyncAction::DeleteRight { path }
        | SyncAction::CreateDirLeft { path }
        | SyncAction::CreateDirRight { path }
        | SyncAction::Conflict { path, .. }
        | SyncAction::Skip { path, .. } => path,
    }
}

struct ThreeWayMerge<'a> {
    left: &'a [FileEntry],
    right: &'a [FileEntry],
    snapshot: &'a [FileEntry],
    left_index: usize,
    right_index: usize,
    snapshot_index: usize,
}

impl<'a> ThreeWayMerge<'a> {
    fn new(left: &'a [FileEntry], right: &'a [FileEntry], snapshot: &'a [FileEntry]) -> Self {
        Self { left, right, snapshot, left_index: 0, right_index: 0, snapshot_index: 0 }
    }
}

impl<'a> Iterator for ThreeWayMerge<'a> {
    type Item = (&'a str, Option<&'a FileEntry>, Option<&'a FileEntry>, Option<&'a FileEntry>);

    fn next(&mut self) -> Option<Self::Item> {
        let path = [
            self.left.get(self.left_index),
            self.right.get(self.right_index),
            self.snapshot.get(self.snapshot_index),
        ]
        .into_iter()
        .flatten()
        .map(|entry| entry.relative_path.as_str())
        .min()?;

        let left = self.left.get(self.left_index).filter(|entry| entry.relative_path == path);
        let right = self.right.get(self.right_index).filter(|entry| entry.relative_path == path);
        let snapshot =
            self.snapshot.get(self.snapshot_index).filter(|entry| entry.relative_path == path);
        if left.is_some() {
            self.left_index += 1;
        }
        if right.is_some() {
            self.right_index += 1;
        }
        if snapshot.is_some() {
            self.snapshot_index += 1;
        }
        Some((path, left, right, snapshot))
    }
}

fn entries_differ(
    a: &FileEntry,
    a_side: EntrySide,
    b: &FileEntry,
    b_side: EntrySide,
    ctx: &DiffContext<'_>,
) -> bool {
    if a.is_dir != b.is_dir {
        return true;
    }
    if a.size != b.size {
        return true;
    }
    if a.modified_secs != b.modified_secs {
        return true;
    }
    if a.modified_nanos != b.modified_nanos {
        return true;
    }
    if a.is_dir {
        return false;
    }
    if a.size == 0 {
        return false;
    }
    content_differs_if_enabled(a, a_side, b, b_side, ctx)
}

fn content_differs_if_enabled(
    a: &FileEntry,
    a_side: EntrySide,
    b: &FileEntry,
    b_side: EntrySide,
    ctx: &DiffContext<'_>,
) -> bool {
    let opts = ctx.options;
    if !opts.content_hash_compare || a.size >= opts.content_hash_max_bytes {
        return false;
    }
    // Metadata-only snapshots have no physical path or content hash. Metadata is
    // the documented comparison semantics; never infer a snapshot path.
    if (a_side == EntrySide::Snapshot && a.hash.is_none())
        || (b_side == EntrySide::Snapshot && b.hash.is_none())
    {
        return false;
    }
    let left_hash = hash_entry(a, a_side, ctx);
    let right_hash = hash_entry(b, b_side, ctx);
    match (left_hash, right_hash) {
        (Some(Ok(lh)), Some(Ok(rh))) => lh != rh,
        (Some(Err(_)), _) | (_, Some(Err(_))) => true,
        (Some(Ok(_)), None) | (None, Some(Ok(_))) | (None, None) => false,
    }
}

fn hash_entry(
    entry: &FileEntry,
    side: EntrySide,
    ctx: &DiffContext<'_>,
) -> Option<Result<String, String>> {
    if let Some(hash) = &entry.hash {
        return Some(Ok(hash.clone()));
    }
    let root = match side {
        EntrySide::Left => ctx.options.left_root.as_ref(),
        EntrySide::Right => ctx.options.right_root.as_ref(),
        EntrySide::Snapshot => return None,
    }?;
    let key = HashKey {
        side,
        relative_path: entry.relative_path.clone(),
        size: entry.size,
        modified_secs: entry.modified_secs,
        modified_nanos: entry.modified_nanos,
    };
    if let Some(cached) = ctx.hashes.borrow().get(&key) {
        return Some(cached.clone());
    }
    let path = join_relative(root, &entry.relative_path);
    let result = hashing::hash_file(&path)
        .map_err(|error| format!("{}: content hash failed: {error}", entry.relative_path));
    if let Err(error) = &result {
        ctx.hash_warnings.borrow_mut().push(error.clone());
    }
    ctx.hashes.borrow_mut().insert(key, result.clone());
    Some(result)
}

fn join_relative(base: &std::path::Path, relative: &str) -> PathBuf {
    let rel = relative.replace('/', std::path::MAIN_SEPARATOR_STR);
    base.join(rel)
}

pub fn resolve_conflict_action(
    resolution: ConflictResolution,
    path: &str,
    left: &FileEntry,
    right: &FileEntry,
) -> SyncAction {
    match resolution {
        ConflictResolution::Left if left.deleted => {
            SyncAction::DeleteRight { path: path.to_string() }
        }
        ConflictResolution::Left if right.deleted => {
            SyncAction::CopyLeftToRight { path: path.to_string() }
        }
        ConflictResolution::Left => SyncAction::CopyLeftToRight { path: path.to_string() },
        ConflictResolution::Right if right.deleted => {
            SyncAction::DeleteLeft { path: path.to_string() }
        }
        ConflictResolution::Right if left.deleted => {
            SyncAction::CopyRightToLeft { path: path.to_string() }
        }
        ConflictResolution::Right => SyncAction::CopyRightToLeft { path: path.to_string() },
        ConflictResolution::KeepBoth => {
            SyncAction::Skip { path: path.to_string(), reason: "keep both (user choice)".into() }
        }
        ConflictResolution::Skip => {
            SyncAction::Skip { path: path.to_string(), reason: "skipped by user".into() }
        }
    }
}

pub fn apply_conflict_resolutions(
    actions: &mut [SyncAction],
    resolutions: &std::collections::HashMap<String, ConflictResolution>,
) {
    for action in actions.iter_mut() {
        if let SyncAction::Conflict { path, left, right } = action {
            if let Some(resolution) = resolutions.get(path) {
                *action = resolve_conflict_action(*resolution, path, left, right);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ConflictPolicy;

    fn file(path: &str, size: u64, modified: i64) -> FileEntry {
        file_with_nanos(path, size, modified, 0)
    }

    fn file_with_nanos(path: &str, size: u64, modified: i64, nanos: u32) -> FileEntry {
        FileEntry {
            relative_path: path.into(),
            size,
            modified_secs: modified,
            modified_nanos: nanos,
            is_dir: false,
            hash: None,
            deleted: false,
        }
    }

    fn dir(path: &str) -> FileEntry {
        FileEntry {
            relative_path: path.into(),
            size: 0,
            modified_secs: 0,
            modified_nanos: 0,
            is_dir: true,
            hash: None,
            deleted: false,
        }
    }

    fn paths(plan: &SyncPlan) -> Vec<&str> {
        plan.actions
            .iter()
            .map(|a| match a {
                SyncAction::CopyLeftToRight { path }
                | SyncAction::CopyRightToLeft { path }
                | SyncAction::DeleteRight { path }
                | SyncAction::DeleteLeft { path }
                | SyncAction::CreateDirRight { path }
                | SyncAction::CreateDirLeft { path }
                | SyncAction::Conflict { path, .. }
                | SyncAction::Skip { path, .. } => path.as_str(),
            })
            .collect()
    }

    fn has_copy_ltr(plan: &SyncPlan, path: &str) -> bool {
        plan.actions.iter().any(|a| {
            matches!(
                a,
                SyncAction::CopyLeftToRight { path: p } if p == path
            )
        })
    }

    fn has_copy_rtl(plan: &SyncPlan, path: &str) -> bool {
        plan.actions.iter().any(|a| {
            matches!(
                a,
                SyncAction::CopyRightToLeft { path: p } if p == path
            )
        })
    }

    fn has_delete_right(plan: &SyncPlan, path: &str) -> bool {
        plan.actions.iter().any(|a| matches!(a, SyncAction::DeleteRight { path: p } if p == path))
    }

    fn has_conflict(plan: &SyncPlan, path: &str) -> bool {
        plan.actions.iter().any(|a| matches!(a, SyncAction::Conflict { path: p, .. } if p == path))
    }

    fn has_delete_left(plan: &SyncPlan, path: &str) -> bool {
        plan.actions.iter().any(|a| matches!(a, SyncAction::DeleteLeft { path: p } if p == path))
    }

    fn plan(
        mode: SyncMode,
        policy: ConflictPolicy,
        left: &[FileEntry],
        right: &[FileEntry],
        snapshot: Option<&[FileEntry]>,
    ) -> SyncPlan {
        build_sync_plan(
            "p1",
            mode,
            policy,
            left,
            right,
            snapshot,
            ScanIntegrity::default(),
            DiffOptions::default(),
        )
    }

    #[test]
    fn echo_copies_new_left_files_and_deletes_extra_right() {
        let plan = plan(
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &[file("a.txt", 1, 10), file("b.txt", 2, 20)],
            &[file("a.txt", 1, 10), file("orphan.txt", 3, 30)],
            None,
        );
        assert!(has_copy_ltr(&plan, "b.txt"));
        assert!(has_delete_right(&plan, "orphan.txt"));
    }

    #[test]
    fn echo_overwrites_differing_files_from_left() {
        let plan = plan(
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &[file("doc.txt", 100, 50)],
            &[file("doc.txt", 50, 10)],
            None,
        );
        assert!(has_copy_ltr(&plan, "doc.txt"));
    }

    #[test]
    fn contribute_skips_right_only_files() {
        let plan = plan(
            SyncMode::Contribute,
            ConflictPolicy::NewerWins,
            &[file("a.txt", 1, 1)],
            &[file("a.txt", 1, 1), file("extra.txt", 2, 2)],
            None,
        );
        assert!(plan
            .actions
            .iter()
            .any(|a| { matches!(a, SyncAction::Skip { path, .. } if path == "extra.txt") }));
        assert!(!has_delete_right(&plan, "extra.txt"));
    }

    #[test]
    fn contribute_copies_new_left_file() {
        let plan = plan(
            SyncMode::Contribute,
            ConflictPolicy::NewerWins,
            &[file("new.txt", 1, 1)],
            &[],
            None,
        );
        assert!(has_copy_ltr(&plan, "new.txt"));
    }

    #[test]
    fn synchronize_copies_from_right_when_only_on_right() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[],
            &[file("only-right.txt", 1, 1)],
            None,
        );
        assert!(plan.actions.iter().any(|a| {
            matches!(a, SyncAction::CopyRightToLeft { path } if path == "only-right.txt")
        }));
    }

    #[test]
    fn synchronize_reports_conflict_when_both_changed_since_snapshot_and_ask() {
        let snap = file("both.txt", 1, 1);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::Ask,
            &[file("both.txt", 10, 100)],
            &[file("both.txt", 20, 200)],
            Some(&[snap]),
        );
        assert!(has_conflict(&plan, "both.txt"));
    }

    #[test]
    fn synchronize_newer_wins_when_both_changed_since_snapshot() {
        let snap = file("both.txt", 1, 1);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[file("both.txt", 10, 100)],
            &[file("both.txt", 20, 200)],
            Some(&[snap]),
        );
        assert!(has_copy_rtl(&plan, "both.txt"));
        assert!(!has_conflict(&plan, "both.txt"));
    }

    #[test]
    fn synchronize_copies_newer_when_one_side_changed() {
        let snap = file("doc.txt", 5, 50);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::Ask,
            &[file("doc.txt", 5, 100)],
            &[file("doc.txt", 5, 50)],
            Some(&[snap]),
        );
        assert!(has_copy_ltr(&plan, "doc.txt"));
        assert!(!has_conflict(&plan, "doc.txt"));
    }

    #[test]
    fn synchronize_without_snapshot_newer_wins_copies_newer_not_conflict() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[file("both.txt", 10, 100)],
            &[file("both.txt", 20, 200)],
            None,
        );
        assert!(has_copy_rtl(&plan, "both.txt"));
        assert!(!has_conflict(&plan, "both.txt"));
    }

    #[test]
    fn synchronize_without_snapshot_ask_reports_conflict() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::Ask,
            &[file("both.txt", 10, 100)],
            &[file("both.txt", 20, 200)],
            None,
        );
        assert!(has_conflict(&plan, "both.txt"));
    }

    #[test]
    fn synchronize_without_snapshot_left_policy_copies_left() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::Left,
            &[file("both.txt", 10, 100)],
            &[file("both.txt", 20, 200)],
            None,
        );
        assert!(has_copy_ltr(&plan, "both.txt"));
        assert!(!has_conflict(&plan, "both.txt"));
    }

    #[test]
    fn identical_files_produce_no_actions_in_echo() {
        let entry = file("same.txt", 1, 1);
        let plan =
            plan(SyncMode::Echo, ConflictPolicy::NewerWins, &[entry.clone()], &[entry], None);
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn echo_creates_subdirectory_for_nested_copy() {
        let plan = plan(
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &[dir("sub"), file("sub/nested.txt", 1, 1)],
            &[],
            None,
        );
        assert!(paths(&plan).contains(&"sub"));
        assert!(has_copy_ltr(&plan, "sub/nested.txt"));
    }

    #[test]
    fn synchronize_snapshot_delete_on_right_when_left_unchanged() {
        let snap = file("gone.txt", 1, 1);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[file("gone.txt", 1, 1)],
            &[],
            Some(&[snap]),
        );
        assert!(has_delete_right(&plan, "gone.txt"));
        assert!(!has_copy_ltr(&plan, "gone.txt"));
    }

    #[test]
    fn synchronize_snapshot_delete_on_left_when_right_unchanged() {
        let snap = file("gone.txt", 1, 1);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[],
            &[file("gone.txt", 1, 1)],
            Some(&[snap]),
        );
        assert!(has_delete_left(&plan, "gone.txt"));
    }

    #[test]
    fn synchronize_without_snapshot_treats_one_sided_as_new_file() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[file("new.txt", 1, 1)],
            &[],
            None,
        );
        assert!(has_copy_ltr(&plan, "new.txt"));
        assert!(!has_delete_right(&plan, "new.txt"));
    }

    #[test]
    fn synchronize_one_sided_modify_and_delete_reports_conflict_when_ask() {
        let snap = file("doc.txt", 1, 1);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::Ask,
            &[file("doc.txt", 9, 9)],
            &[],
            Some(&[snap]),
        );
        assert!(has_conflict(&plan, "doc.txt"));
    }

    #[test]
    fn synchronize_keep_both_policy_skips_true_conflict() {
        let snap = file("both.txt", 1, 1);
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::KeepBoth,
            &[file("both.txt", 10, 100)],
            &[file("both.txt", 20, 200)],
            Some(&[snap]),
        );
        assert!(plan.actions.iter().any(|a| {
            matches!(
                a,
                SyncAction::Skip { path, reason }
                    if path == "both.txt" && reason.contains("keep both")
            )
        }));
    }

    #[test]
    fn apply_conflict_resolutions_replaces_conflict_actions() {
        use super::apply_conflict_resolutions;
        use crate::models::ConflictResolution;
        use std::collections::HashMap;

        let left = file("x.txt", 1, 1);
        let right = file("x.txt", 2, 2);
        let mut actions = vec![SyncAction::Conflict {
            path: "x.txt".into(),
            left: left.clone(),
            right: right.clone(),
        }];
        let mut resolutions = HashMap::new();
        resolutions.insert("x.txt".into(), ConflictResolution::Right);
        apply_conflict_resolutions(&mut actions, &resolutions);
        assert!(matches!(
            actions[0],
            SyncAction::CopyRightToLeft { ref path } if path == "x.txt"
        ));
    }

    #[test]
    fn plan_includes_scan_counts() {
        let plan = build_sync_plan(
            "pair-99",
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &[file("a.txt", 1, 1)],
            &[file("b.txt", 1, 1)],
            None,
            ScanIntegrity {
                warnings: vec!["left: 2 paths skipped".into()],
                skipped_left: 2,
                ..ScanIntegrity::default()
            },
            DiffOptions::default(),
        );
        assert_eq!(plan.pair_id, "pair-99");
        assert_eq!(plan.scanned_left, 1);
        assert_eq!(plan.scanned_right, 1);
        assert_eq!(plan.scan_warnings.len(), 1);
        assert_eq!(plan.scan_skipped_left, 2);
    }

    #[test]
    fn nanos_differ_detects_change_in_echo() {
        let plan = plan(
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &[file_with_nanos("doc.txt", 10, 100, 500_000_000)],
            &[file_with_nanos("doc.txt", 10, 100, 100_000_000)],
            None,
        );
        assert!(has_copy_ltr(&plan, "doc.txt"));
    }

    #[test]
    fn same_sec_different_content_detected_with_hash_compare() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let left_root = dir.path().join("left");
        let right_root = dir.path().join("right");
        fs::create_dir_all(&left_root).expect("mkdir left");
        fs::create_dir_all(&right_root).expect("mkdir right");
        fs::write(left_root.join("doc.txt"), "aaaaaaaaaa").expect("write left");
        fs::write(right_root.join("doc.txt"), "bbbbbbbbbb").expect("write right");

        let left = vec![file_with_nanos("doc.txt", 10, 100, 0)];
        let right = vec![file_with_nanos("doc.txt", 10, 100, 0)];

        let without_hash = build_sync_plan(
            "p1",
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &left,
            &right,
            None,
            ScanIntegrity::default(),
            DiffOptions {
                left_root: Some(left_root.clone()),
                right_root: Some(right_root.clone()),
                content_hash_compare: false,
                content_hash_max_bytes: 50 * 1024 * 1024,
            },
        );
        assert!(without_hash.actions.is_empty());

        let with_hash = build_sync_plan(
            "p1",
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &left,
            &right,
            None,
            ScanIntegrity::default(),
            DiffOptions {
                left_root: Some(left_root),
                right_root: Some(right_root),
                content_hash_compare: true,
                content_hash_max_bytes: 50 * 1024 * 1024,
            },
        );
        assert!(has_copy_ltr(&with_hash, "doc.txt"));
    }

    #[test]
    fn equal_metadata_hashes_each_current_side_once_per_plan() {
        use crate::hashing::{hash_invocation_count, reset_hash_invocations};
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let left_root = dir.path().join("left");
        let right_root = dir.path().join("right");
        fs::create_dir_all(&left_root).expect("mkdir left");
        fs::create_dir_all(&right_root).expect("mkdir right");
        fs::write(left_root.join("doc.txt"), "left-content").expect("write left");
        fs::write(right_root.join("doc.txt"), "right-content").expect("write right");

        let left = vec![file_with_nanos("doc.txt", 12, 100, 0)];
        let right = vec![file_with_nanos("doc.txt", 12, 100, 0)];
        let snapshot = vec![file_with_nanos("doc.txt", 12, 100, 0)];
        reset_hash_invocations();

        let plan = build_sync_plan(
            "p1",
            SyncMode::Synchronize,
            ConflictPolicy::Ask,
            &left,
            &right,
            Some(&snapshot),
            ScanIntegrity::default(),
            DiffOptions {
                left_root: Some(left_root),
                right_root: Some(right_root),
                ..DiffOptions::default()
            },
        );

        assert_eq!(hash_invocation_count(), 2);
        assert!(!plan.requires_attention);
    }

    #[test]
    fn metadata_only_snapshot_does_not_trigger_physical_hashing() {
        use crate::hashing::{hash_invocation_count, reset_hash_invocations};
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let left_root = dir.path().join("left");
        let right_root = dir.path().join("right");
        fs::create_dir_all(&left_root).expect("mkdir left");
        fs::create_dir_all(&right_root).expect("mkdir right");
        fs::write(left_root.join("doc.txt"), "left-content").expect("write left");

        let left = vec![file_with_nanos("doc.txt", 12, 100, 0)];
        let snapshot = vec![file_with_nanos("doc.txt", 12, 100, 0)];
        reset_hash_invocations();

        let plan = build_sync_plan(
            "p1",
            SyncMode::Synchronize,
            ConflictPolicy::Ask,
            &left,
            &[],
            Some(&snapshot),
            ScanIntegrity::default(),
            DiffOptions {
                left_root: Some(left_root),
                right_root: Some(right_root),
                ..DiffOptions::default()
            },
        );

        assert_eq!(hash_invocation_count(), 0);
        assert!(plan.actions.iter().any(|action| matches!(
            action,
            SyncAction::DeleteRight { path } if path == "doc.txt"
        )));
    }

    #[test]
    fn hash_failure_marks_plan_as_needing_attention() {
        use crate::hashing::reset_hash_invocations;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let left = vec![file_with_nanos("missing.txt", 12, 100, 0)];
        let right = vec![file_with_nanos("missing.txt", 12, 100, 0)];
        reset_hash_invocations();

        let plan = build_sync_plan(
            "p1",
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &left,
            &right,
            None,
            ScanIntegrity::default(),
            DiffOptions {
                left_root: Some(dir.path().join("left")),
                right_root: Some(dir.path().join("right")),
                ..DiffOptions::default()
            },
        );

        assert!(plan.requires_attention);
        assert!(plan.scan_warnings.iter().any(|warning| warning.contains("hash failed")));
    }

    #[test]
    fn newer_wins_prefers_higher_nanos_on_tied_seconds() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[file_with_nanos("both.txt", 10, 100, 900_000_000)],
            &[file_with_nanos("both.txt", 10, 100, 100_000_000)],
            None,
        );
        assert!(has_copy_ltr(&plan, "both.txt"));
    }

    #[test]
    fn newer_wins_keeps_newer_modified_file_against_delete() {
        let plan = plan(
            SyncMode::Synchronize,
            ConflictPolicy::NewerWins,
            &[file_with_nanos("doc.txt", 20, 200, 0)],
            &[],
            Some(&[file_with_nanos("doc.txt", 10, 100, 0)]),
        );
        assert!(has_copy_ltr(&plan, "doc.txt"));
    }
}
