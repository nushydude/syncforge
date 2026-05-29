use std::collections::{BTreeSet, HashMap};

use crate::models::{
    ConflictPolicy, ConflictResolution, FileEntry, SyncAction, SyncMode, SyncPlan,
};

pub fn build_sync_plan(
    pair_id: &str,
    mode: SyncMode,
    conflict_policy: ConflictPolicy,
    left: &[FileEntry],
    right: &[FileEntry],
    snapshot: Option<&[FileEntry]>,
    scan_warnings: Vec<String>,
) -> SyncPlan {
    let left_map = entries_map(left);
    let right_map = entries_map(right);
    let snapshot_map = snapshot.map(entries_map).unwrap_or_default();

    let paths = collect_paths(&left_map, &right_map, &snapshot_map);
    let mut actions = Vec::new();

    for path in paths {
        let l = left_map.get(&path);
        let r = right_map.get(&path);
        let s = snapshot_map.get(&path);

        match mode {
            SyncMode::Echo => {
                // Echo mirrors left → right; snapshot is not used for planning.
                plan_echo(&mut actions, &path, l, r);
            }
            SyncMode::Contribute => {
                plan_contribute(&mut actions, &path, l, r);
            }
            SyncMode::Synchronize => {
                plan_synchronize(&mut actions, &path, l, r, s, conflict_policy);
            }
        }
    }

    ensure_parent_dirs(&mut actions, mode, &left_map, &right_map);
    sort_actions(&mut actions);

    SyncPlan {
        pair_id: pair_id.to_string(),
        actions,
        scanned_left: left.len() as u32,
        scanned_right: right.len() as u32,
        scan_warnings,
    }
}

fn plan_echo(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: Option<&FileEntry>,
    right: Option<&FileEntry>,
) {
    match (left, right) {
        (Some(l), None) if l.is_dir => {
            actions.push(SyncAction::CreateDirRight {
                path: path.to_string(),
            });
        }
        (Some(l), None) if !l.is_dir => {
            actions.push(SyncAction::CopyLeftToRight {
                path: path.to_string(),
            });
        }
        (Some(l), Some(r)) if entries_differ(l, r) && !l.is_dir => {
            actions.push(SyncAction::CopyLeftToRight {
                path: path.to_string(),
            });
        }
        (Some(l), Some(r)) if l.is_dir && !r.is_dir => {
            actions.push(SyncAction::CreateDirRight {
                path: path.to_string(),
            });
        }
        (None, Some(_)) => {
            actions.push(SyncAction::DeleteRight {
                path: path.to_string(),
            });
        }
        _ => {}
    }
}

fn plan_contribute(
    actions: &mut Vec<SyncAction>,
    path: &str,
    left: Option<&FileEntry>,
    right: Option<&FileEntry>,
) {
    match (left, right) {
        (Some(l), None) if l.is_dir => {
            actions.push(SyncAction::CreateDirRight {
                path: path.to_string(),
            });
        }
        (Some(l), None) if !l.is_dir => {
            actions.push(SyncAction::CopyLeftToRight {
                path: path.to_string(),
            });
        }
        (Some(l), Some(r)) if entries_differ(l, r) && !l.is_dir => {
            actions.push(SyncAction::CopyLeftToRight {
                path: path.to_string(),
            });
        }
        (Some(l), Some(r)) if l.is_dir && !r.is_dir => {
            actions.push(SyncAction::CreateDirRight {
                path: path.to_string(),
            });
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
) {
    match (left, right) {
        (Some(l), None) if l.is_dir => {
            actions.push(SyncAction::CreateDirRight {
                path: path.to_string(),
            });
        }
        (Some(l), None) if !l.is_dir => {
            plan_one_sided_file(
                actions,
                path,
                l,
                true,
                snapshot,
                conflict_policy,
            );
        }
        (None, Some(r)) if r.is_dir => {
            actions.push(SyncAction::CreateDirLeft {
                path: path.to_string(),
            });
        }
        (None, Some(r)) if !r.is_dir => {
            plan_one_sided_file(
                actions,
                path,
                r,
                false,
                snapshot,
                conflict_policy,
            );
        }
        (Some(l), Some(r)) if l.is_dir && r.is_dir => {}
        (Some(l), Some(r)) if !l.is_dir && !r.is_dir => {
            if !entries_differ(l, r) {
                return;
            }
            if snapshot_changed_both(l, r, snapshot) {
                apply_conflict_policy(actions, path, l, r, conflict_policy);
            } else if snapshot.is_none() {
                apply_conflict_policy(actions, path, l, r, conflict_policy);
            } else {
                push_newer_wins_copy(actions, path, l, r);
            }
        }
        (Some(l), Some(r)) if l.is_dir && !r.is_dir => {
            actions.push(SyncAction::CreateDirRight {
                path: path.to_string(),
            });
        }
        (Some(l), Some(r)) if !l.is_dir && r.is_dir => {
            actions.push(SyncAction::CreateDirLeft {
                path: path.to_string(),
            });
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
) {
    match snapshot {
        None => {
            if present_is_left {
                actions.push(SyncAction::CopyLeftToRight {
                    path: path.to_string(),
                });
            } else {
                actions.push(SyncAction::CopyRightToLeft {
                    path: path.to_string(),
                });
            }
        }
        Some(snap) if entries_match(snap, present) => {
            if present_is_left {
                actions.push(SyncAction::DeleteRight {
                    path: path.to_string(),
                });
            } else {
                actions.push(SyncAction::DeleteLeft {
                    path: path.to_string(),
                });
            }
        }
        Some(snap) => {
            let (left, right) = if present_is_left {
                (present.clone(), snap.clone())
            } else {
                (snap.clone(), present.clone())
            };
            apply_conflict_policy(actions, path, &left, &right, conflict_policy);
        }
    }
}

fn entries_match(a: &FileEntry, b: &FileEntry) -> bool {
    !entries_differ(a, b)
}

fn snapshot_changed_both(
    left: &FileEntry,
    right: &FileEntry,
    snapshot: Option<&FileEntry>,
) -> bool {
    let Some(snap) = snapshot else {
        return false;
    };
    entries_differ(left, snap) && entries_differ(right, snap)
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
            push_newer_wins_copy(actions, path, left, right);
        }
        ConflictPolicy::Left => {
            actions.push(SyncAction::CopyLeftToRight {
                path: path.to_string(),
            });
        }
        ConflictPolicy::Right => {
            actions.push(SyncAction::CopyRightToLeft {
                path: path.to_string(),
            });
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
    if left.modified_secs > right.modified_secs
        || (left.modified_secs == right.modified_secs && left.size != right.size)
    {
        actions.push(SyncAction::CopyLeftToRight {
            path: path.to_string(),
        });
    } else if right.modified_secs > left.modified_secs {
        actions.push(SyncAction::CopyRightToLeft {
            path: path.to_string(),
        });
    } else {
        actions.push(SyncAction::CopyLeftToRight {
            path: path.to_string(),
        });
    }
}

fn ensure_parent_dirs(
    actions: &mut Vec<SyncAction>,
    mode: SyncMode,
    left: &HashMap<String, FileEntry>,
    right: &HashMap<String, FileEntry>,
) {
    let mut needed_right: BTreeSet<String> = BTreeSet::new();
    let mut needed_left: BTreeSet<String> = BTreeSet::new();

    for action in actions.iter() {
        match action {
            SyncAction::CopyLeftToRight { path } | SyncAction::CreateDirRight { path } => {
                for parent in parent_paths(path) {
                    if !right.contains_key(&parent) && left.get(&parent).is_some_and(|e| e.is_dir)
                    {
                        needed_right.insert(parent);
                    }
                }
            }
            SyncAction::CopyRightToLeft { path } | SyncAction::CreateDirLeft { path } => {
                for parent in parent_paths(path) {
                    if !left.contains_key(&parent) && right.get(&parent).is_some_and(|e| e.is_dir)
                    {
                        needed_left.insert(parent);
                    }
                }
            }
            _ => {}
        }
    }

    for path in needed_right {
        if !has_action_for_path(actions, &path, true) {
            actions.push(SyncAction::CreateDirRight { path });
        }
    }

    for path in needed_left {
        if mode == SyncMode::Synchronize && !has_action_for_path(actions, &path, false) {
            actions.push(SyncAction::CreateDirLeft { path });
        }
    }
}

fn has_action_for_path(actions: &[SyncAction], path: &str, right: bool) -> bool {
    actions.iter().any(|a| match a {
        SyncAction::CreateDirRight { path: p } if right => p == path,
        SyncAction::CreateDirLeft { path: p } if !right => p == path,
        _ => false,
    })
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

fn sort_actions(actions: &mut Vec<SyncAction>) {
    actions.sort_by(|a, b| action_path(a).cmp(action_path(b)));
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

fn entries_map(entries: &[FileEntry]) -> HashMap<String, FileEntry> {
    entries
        .iter()
        .map(|e| (e.relative_path.clone(), e.clone()))
        .collect()
}

fn collect_paths(
    left: &HashMap<String, FileEntry>,
    right: &HashMap<String, FileEntry>,
    snapshot: &HashMap<String, FileEntry>,
) -> Vec<String> {
    let mut set = BTreeSet::new();
    for key in left.keys().chain(right.keys()).chain(snapshot.keys()) {
        set.insert(key.clone());
    }
    set.into_iter().collect()
}

fn entries_differ(a: &FileEntry, b: &FileEntry) -> bool {
    a.is_dir != b.is_dir || a.size != b.size || a.modified_secs != b.modified_secs
}

pub fn resolve_conflict_action(
    resolution: ConflictResolution,
    path: &str,
    _left: &FileEntry,
    _right: &FileEntry,
) -> SyncAction {
    match resolution {
        ConflictResolution::Left => SyncAction::CopyLeftToRight {
            path: path.to_string(),
        },
        ConflictResolution::Right => SyncAction::CopyRightToLeft {
            path: path.to_string(),
        },
        ConflictResolution::KeepBoth => SyncAction::Skip {
            path: path.to_string(),
            reason: "keep both (user choice)".into(),
        },
        ConflictResolution::Skip => SyncAction::Skip {
            path: path.to_string(),
            reason: "skipped by user".into(),
        },
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
        FileEntry {
            relative_path: path.into(),
            size,
            modified_secs: modified,
            is_dir: false,
            hash: None,
        }
    }

    fn dir(path: &str) -> FileEntry {
        FileEntry {
            relative_path: path.into(),
            size: 0,
            modified_secs: 0,
            is_dir: true,
            hash: None,
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
        plan.actions
            .iter()
            .any(|a| matches!(a, SyncAction::DeleteRight { path: p } if p == path))
    }

    fn has_conflict(plan: &SyncPlan, path: &str) -> bool {
        plan.actions
            .iter()
            .any(|a| matches!(a, SyncAction::Conflict { path: p, .. } if p == path))
    }

    fn has_delete_left(plan: &SyncPlan, path: &str) -> bool {
        plan.actions
            .iter()
            .any(|a| matches!(a, SyncAction::DeleteLeft { path: p } if p == path))
    }

    fn plan(
        mode: SyncMode,
        policy: ConflictPolicy,
        left: &[FileEntry],
        right: &[FileEntry],
        snapshot: Option<&[FileEntry]>,
    ) -> SyncPlan {
        build_sync_plan("p1", mode, policy, left, right, snapshot, vec![])
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
        assert!(plan.actions.iter().any(|a| {
            matches!(a, SyncAction::Skip { path, .. } if path == "extra.txt")
        }));
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
        let plan = plan(
            SyncMode::Echo,
            ConflictPolicy::NewerWins,
            &[entry.clone()],
            &[entry],
            None,
        );
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
            vec!["left: 2 paths skipped".into()],
        );
        assert_eq!(plan.pair_id, "pair-99");
        assert_eq!(plan.scanned_left, 1);
        assert_eq!(plan.scanned_right, 1);
        assert_eq!(plan.scan_warnings.len(), 1);
    }
}
