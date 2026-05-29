use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderPair {
    pub id: String,
    pub name: String,
    pub left_path: String,
    pub right_path: String,
    pub mode: SyncMode,
    pub filters: Filters,
    pub conflict_policy: ConflictPolicy,
    pub enabled: bool,
    #[serde(default)]
    pub watch_enabled: bool,
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_cron: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncMode {
    Synchronize,
    Echo,
    Contribute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub relative_path: String,
    pub size: u64,
    pub modified_secs: i64,
    /// Subsecond fraction of [`modified_secs`] (0–999_999_999). Older snapshots omit this field (defaults to 0).
    #[serde(default)]
    pub modified_nanos: u32,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SyncAction {
    CopyLeftToRight { path: String },
    CopyRightToLeft { path: String },
    DeleteLeft { path: String },
    DeleteRight { path: String },
    CreateDirLeft { path: String },
    CreateDirRight { path: String },
    Conflict { path: String, left: FileEntry, right: FileEntry },
    Skip { path: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlan {
    pub pair_id: String,
    pub actions: Vec<SyncAction>,
    pub scanned_left: u32,
    pub scanned_right: u32,
    #[serde(default)]
    pub scan_skipped_left: u32,
    #[serde(default)]
    pub scan_skipped_right: u32,
    #[serde(default)]
    pub scan_warnings: Vec<String>,
    /// When true, destructive sync modes must not run until the user resolves scan issues.
    #[serde(default)]
    pub requires_attention: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub run_id: String,
    pub pair_id: String,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    pub status: RunStatus,
    pub files_copied: u32,
    pub files_deleted: u32,
    pub bytes_transferred: u64,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    NewerWins,
    Left,
    Right,
    KeepBoth,
    Ask,
}

/// Per-path resolution when the user is prompted for a conflict (`ask` policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictResolution {
    Left,
    Right,
    KeepBoth,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub pair_id: String,
    pub captured_at: i64,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunItem {
    pub id: String,
    pub run_id: String,
    pub path: String,
    pub action: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pair() -> FolderPair {
        FolderPair {
            id: "pair-1".into(),
            name: "Documents".into(),
            left_path: r"C:\Users\me\Documents".into(),
            right_path: r"D:\Backup\Documents".into(),
            mode: SyncMode::Echo,
            filters: Filters { include: vec!["*.txt".into()], exclude: vec!["*.tmp".into()] },
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_100_000,
        }
    }

    #[test]
    fn folder_pair_serde_round_trip() {
        let pair = sample_pair();
        let json = serde_json::to_string(&pair).expect("serialize");
        let back: FolderPair = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pair, back);
    }

    #[test]
    fn sync_plan_serde_round_trip() {
        let plan = SyncPlan {
            pair_id: "pair-1".into(),
            actions: vec![
                SyncAction::CopyLeftToRight { path: "notes.txt".into() },
                SyncAction::Conflict {
                    path: "report.doc".into(),
                    left: FileEntry {
                        relative_path: "report.doc".into(),
                        size: 1024,
                        modified_secs: 100,
                        modified_nanos: 0,
                        is_dir: false,
                        hash: None,
                    },
                    right: FileEntry {
                        relative_path: "report.doc".into(),
                        size: 2048,
                        modified_secs: 200,
                        modified_nanos: 0,
                        is_dir: false,
                        hash: Some("abc".into()),
                    },
                },
            ],
            scanned_left: 10,
            scanned_right: 12,
            scan_skipped_left: 0,
            scan_skipped_right: 0,
            scan_warnings: vec![],
            requires_attention: false,
        };
        let json = serde_json::to_string(&plan).expect("serialize");
        let back: SyncPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(plan, back);
    }

    #[test]
    fn run_report_serde_round_trip() {
        let report = RunReport {
            run_id: "run-1".into(),
            pair_id: "pair-1".into(),
            started_at: 1_700_000_000_000,
            finished_at: Some(1_700_000_060_000),
            status: RunStatus::Completed,
            files_copied: 3,
            files_deleted: 1,
            bytes_transferred: 4096,
            errors: vec![],
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let back: RunReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(report, back);
    }

    #[test]
    fn file_entry_deserialize_without_modified_nanos_defaults_zero() {
        let json = r#"{"relativePath":"f.txt","size":5,"modifiedSecs":9,"isDir":false}"#;
        let entry: FileEntry = serde_json::from_str(json).expect("deserialize");
        assert_eq!(entry.modified_nanos, 0);
        assert_eq!(entry.modified_secs, 9);
    }
}
