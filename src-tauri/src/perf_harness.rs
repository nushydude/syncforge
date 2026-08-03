use std::env;
use std::fs;
use std::path::Path;
use std::sync::{atomic::AtomicBool, Mutex};

use crate::engine::{self, RunOptions};
use crate::models::{ConflictPolicy, Filters, FolderPair, SyncMode};
use crate::persistence::{self, Database};
use crate::progress::ProgressCoalescer;
use crate::scanner;

#[test]
fn run_generated_fixture_and_write_counters() {
    let Ok(root) = env::var("PERF_FIXTURE_ROOT") else { return };
    let Ok(result_file) = env::var("PERF_RESULT_FILE") else { return };
    let pair = FolderPair {
        id: "perf-harness-pair".into(),
        name: "PERF harness".into(),
        left_path: Path::new(&root).join("left").display().to_string(),
        right_path: Path::new(&root).join("right").display().to_string(),
        mode: SyncMode::Echo,
        filters: Filters::default(),
        conflict_policy: ConflictPolicy::NewerWins,
        enabled: true,
        watch_enabled: false,
        schedule_enabled: false,
        schedule_cron: None,
        created_at: 0,
        updated_at: 0,
    };
    let db = Mutex::new(Database::open(&Path::new(&root).join("perf.db")).expect("open perf db"));
    db.lock().expect("lock perf db").save_pair(&pair).expect("save perf pair");
    engine::reset_progress_emit_counter();
    crate::run_coordinator::reset_tauri_event_counter();
    persistence::reset_run_item_batch_counter();
    crate::hashing::reset_hash_invocations();
    let cancel = AtomicBool::new(false);
    let mut progress_sink = ProgressCoalescer::system(|progress| {
        crate::run_coordinator::emit_test_event("sync://progress", &progress)
    });
    let (run_result, scan_count) = scanner::with_scan_counting(|| {
        engine::run_pair_impl(
            &db,
            &pair,
            RunOptions { use_recycle_bin: false, ..RunOptions::default() },
            &cancel,
            |progress| progress_sink.push_event(progress),
        )
    });
    progress_sink.flush();
    progress_sink.flush();
    let report = run_result.expect("run generated fixture");
    let payload = serde_json::json!({
        "scanDirectory": scan_count,
        "hashFile": crate::hashing::hash_invocation_count(),
        "engineProgressCallbacks": engine::progress_emit_count(),
        "progressEvents": crate::run_coordinator::tauri_event_count(),
        "tauriEmits": crate::run_coordinator::tauri_event_count(),
        "dbItemFlushes": persistence::run_item_batch_insert_count(),
        "maxRunItemBuffer": engine::max_run_item_buffer(),
        "filesCopied": report.files_copied,
        "filesDeleted": report.files_deleted,
    });
    fs::write(result_file, serde_json::to_vec(&payload).expect("serialize counters"))
        .expect("write counters");
}
