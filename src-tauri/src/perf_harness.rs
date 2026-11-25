use std::env;
use std::fs;
use std::path::Path;
use std::sync::{atomic::AtomicBool, Mutex};
use std::time::Instant;

use crate::engine::{self, RunOptions};
use crate::models::{ConflictPolicy, Filters, FolderPair, RunStatus, SyncMode};
use crate::persistence::{self, Database};
use crate::progress::ProgressCoalescer;
use crate::scanner;

#[cfg(windows)]
fn peak_resident_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS { cb: size, ..Default::default() };
    // The pseudo-handle is owned by the system and the output points to this stack value.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    (ok != 0).then_some(counters.PeakWorkingSetSize as u64)
}

#[cfg(target_os = "linux")]
fn peak_resident_bytes() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_resident_bytes() -> Option<u64> {
    None
}

#[test]
fn run_generated_fixture_and_write_counters() {
    let Ok(root) = env::var("PERF_FIXTURE_ROOT") else { return };
    let Ok(result_file) = env::var("PERF_RESULT_FILE") else { return };
    let expected_copies: u32 = env::var("PERF_EXPECTED_COPIES").unwrap().parse().unwrap();
    let expected_bytes: u64 = env::var("PERF_EXPECTED_BYTES").unwrap().parse().unwrap();
    assert!(expected_copies > 0 && expected_bytes > 0);
    let root = Path::new(&root);
    let db = Mutex::new(Database::open(&root.join("perf.db")).expect("open perf db"));
    let run = |name: &str, left: &str, right: &str, copies: u32, bytes: u64| {
        let pair = FolderPair {
            id: format!("perf-{name}"),
            name: format!("PERF {name}"),
            left_path: root.join(left).display().to_string(),
            right_path: root.join(right).display().to_string(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 0,
            updated_at: 0,
        };
        db.lock().expect("lock perf db").save_pair(&pair).expect("save perf pair");
        engine::reset_progress_emit_counter();
        crate::run_coordinator::reset_tauri_event_counter();
        persistence::reset_run_item_batch_counter();
        crate::hashing::reset_hash_invocations();
        let cancel = AtomicBool::new(false);
        let mut progress_sink = ProgressCoalescer::system(|progress| {
            crate::run_coordinator::emit_test_event("sync://progress", &progress)
        });
        let started = Instant::now();
        let (result, scan_count) = scanner::with_scan_counting(|| {
            engine::run_pair_impl(
                &db,
                &pair,
                RunOptions { use_recycle_bin: false, ..RunOptions::default() },
                &cancel,
                |progress| progress_sink.push_event(progress),
            )
        });
        progress_sink.flush();
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let process_peak_resident_bytes = peak_resident_bytes();
        let report = result.expect("run generated fixture");
        assert_eq!(report.status, RunStatus::Completed, "{name}: {:?}", report.errors);
        assert_eq!(report.files_copied, copies, "{name} copies");
        assert_eq!(report.files_deleted, 0, "{name} deletes");
        assert_eq!(report.bytes_transferred, bytes, "{name} bytes");
        let flushes = persistence::run_item_batch_insert_count();
        let buffer = engine::max_run_item_buffer();
        if copies > 0 {
            assert!(flushes > 0 && buffer > 0, "changed run did not persist items");
        } else {
            assert_eq!(flushes, 0);
            assert_eq!(buffer, 0);
        }
        let verify_db = Database::open(&root.join("perf.db")).expect("reopen perf db");
        assert_eq!(verify_db.get_run(&report.run_id).unwrap().as_ref(), Some(&report));
        let persisted_items =
            verify_db.list_run_items(&report.run_id).expect("read persisted items");
        assert_eq!(persisted_items.len(), copies as usize, "{name} persisted item count");
        assert!(
            persisted_items.iter().all(|item| {
                item.action == "copyLeftToRight"
                    && item.status == "completed"
                    && item.bytes.is_some()
            }),
            "{name} unexpected persisted action"
        );
        assert_eq!(
            persisted_items.iter().filter_map(|item| item.bytes).sum::<u64>(),
            bytes,
            "{name} persisted bytes"
        );
        serde_json::json!({
            "elapsedMs": elapsed_ms, "scanDirectory": scan_count,
            "rustProcessPeakResidentBytesAtRunEnd": process_peak_resident_bytes,
            "hashFile": crate::hashing::hash_invocation_count(),
            "engineProgressCallbacks": engine::progress_emit_count(),
            "progressEvents": crate::run_coordinator::tauri_event_count(),
            "dbItemFlushes": flushes, "maxRunItemBuffer": buffer,
            "filesCopied": report.files_copied, "filesDeleted": report.files_deleted,
            "bytesTransferred": report.bytes_transferred, "status": report.status,
        })
    };
    let payload = serde_json::json!({
        "noOp": run("noop", "left", "right", 0, 0),
        "changed": run("changed", "changed-left", "changed-right", expected_copies, expected_bytes),
    });
    fs::write(result_file, serde_json::to_vec(&payload).expect("serialize counters"))
        .expect("write counters");
}
