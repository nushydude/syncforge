use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::models::{RunItem, RunReport, RunStatus};
use crate::persistence::DatabaseManager;

fn csv(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn pretty_indented<T: serde::Serialize>(value: &T, spaces: usize) -> Result<String, String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    Ok(json.replace('\n', &format!("\n{}", " ".repeat(spaces))))
}

fn write_report<W, F>(
    writer: &mut W,
    report: &RunReport,
    format: &str,
    visit_items: F,
) -> Result<(), String>
where
    W: Write,
    F: FnOnce(&mut dyn FnMut(RunItem) -> Result<(), String>) -> Result<(), String>,
{
    let write = |e: std::io::Error| e.to_string();
    if format == "json" {
        write!(writer, "{{\n  \"report\": {},\n  \"items\": [", pretty_indented(report, 2)?)
            .map_err(write)?;
    } else if format == "csv" {
        write!(writer, "section,key,value\nrun,runId,{}\nrun,pairId,{}\nrun,startedAt,{}\nrun,finishedAt,{}\nrun,status,{}\nrun,filesCopied,{}\nrun,filesDeleted,{}\nrun,bytesTransferred,{}\nrun,errors,{}\nitems,path,action,status,message,bytes", csv(&report.run_id), csv(&report.pair_id), report.started_at, report.finished_at.map(|v| v.to_string()).unwrap_or_default(), csv(serde_json::to_value(report.status).map_err(|e| e.to_string())?.as_str().ok_or("invalid status")?), report.files_copied, report.files_deleted, report.bytes_transferred, csv(&report.errors.join("; "))).map_err(write)?;
    } else {
        return Err("Unsupported history export format".into());
    }
    let mut first = true;
    visit_items(&mut |item| {
        if format == "json" {
            write!(writer, "{}\n    {}", if first { "" } else { "," }, pretty_indented(&item, 4)?)
                .map_err(write)?;
        } else {
            write!(
                writer,
                "\nitem,{},{},{},{},{}",
                csv(&item.path),
                csv(&item.action),
                csv(&item.status),
                csv(item.message.as_deref().unwrap_or("")),
                item.bytes.map(|v| v.to_string()).unwrap_or_default()
            )
            .map_err(write)?;
        }
        first = false;
        Ok(())
    })?;
    if format == "json" {
        if !first {
            write!(writer, "\n  ").map_err(write)?;
        }
        write!(writer, "],\n  \"nextCursor\": null\n}}").map_err(write)?;
    }
    Ok(())
}

#[cfg(windows)]
fn publish_new_file(stage: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let source: Vec<u16> = stage.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    // No replacement or cross-volume copy flags. The stage is beside the destination.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn publish_new_file(stage: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(stage, destination)
}

pub fn export_to_path(
    db: &DatabaseManager,
    run_id: &str,
    format: &str,
    destination: &Path,
) -> Result<(), String> {
    let report = db.get_run(run_id).map_err(|e| e.to_string())?.ok_or("Run not found")?;
    if report.status == RunStatus::Running {
        return Err("Wait for the run to finish before exporting".into());
    }
    let parent = destination.parent().ok_or("Invalid destination")?;
    let stage = parent.join(format!(".syncforge-export-{}.tmp", uuid::Uuid::new_v4()));
    let file =
        OpenOptions::new().write(true).create_new(true).open(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        write_report(&mut writer, &report, format, |visit| db.visit_run_items(&report, visit))?;
        writer.flush().map_err(|e| e.to_string())?;
        writer.get_ref().sync_all().map_err(|e| e.to_string())?;
        drop(writer);
        // Publish only to a new name; never replace an existing file.
        publish_new_file(&stage, destination)
            .map_err(|e| format!("Could not create export (choose a new file name): {e}"))?;
        Ok(())
    })();
    // On Windows the successful move has already consumed the staged pathname.
    if result.is_err() || cfg!(not(windows)) {
        let _ = fs::remove_file(&stage);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::Database;

    fn report() -> RunReport {
        RunReport {
            run_id: "run".into(),
            pair_id: "pair".into(),
            started_at: 1,
            finished_at: Some(2),
            status: RunStatus::Completed,
            files_copied: 1,
            files_deleted: 0,
            bytes_transferred: 42,
            errors: vec!["bad, \"disk\"".into()],
        }
    }
    fn item(n: usize) -> RunItem {
        RunItem {
            id: format!("{n:06}"),
            run_id: "run".into(),
            path: format!("path,{n:06}"),
            action: "copyLeftToRight".into(),
            status: "completed".into(),
            message: Some("hello\nworld".into()),
            bytes: Some(42),
        }
    }
    #[test]
    fn format_matches_existing_json_and_csv_contract() {
        let mut json = Vec::new();
        write_report(&mut json, &report(), "json", |visit| visit(item(0))).unwrap();
        let detail = crate::commands::history::RunDetail {
            report: report(),
            items: vec![item(0)],
            next_cursor: None,
        };
        assert_eq!(
            String::from_utf8(json).unwrap(),
            serde_json::to_string_pretty(&detail).unwrap()
        );
        let mut out = Vec::new();
        write_report(&mut out, &report(), "csv", |visit| visit(item(0))).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "section,key,value\nrun,runId,run\nrun,pairId,pair\nrun,startedAt,1\nrun,finishedAt,2\nrun,status,completed\nrun,filesCopied,1\nrun,filesDeleted,0\nrun,bytesTransferred,42\nrun,errors,\"bad, \"\"disk\"\"\"\nitems,path,action,status,message,bytes\nitem,\"path,000000\",copyLeftToRight,completed,\"hello\nworld\",42");
    }
    fn fixture_db(dir: &Path) -> Database {
        let db_path = dir.join("history.db");
        let db = Database::open(&db_path).unwrap();
        db.save_pair(&crate::models::FolderPair {
            id: "pair".into(),
            name: "test".into(),
            left_path: "left".into(),
            right_path: "right".into(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::Ask,
            enabled: true,
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
        db.save_run(&report()).unwrap();
        db
    }
    #[test]
    fn staged_export_never_replaces_existing_file_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("history.db");
        let db = fixture_db(dir.path());
        db.insert_run_items(&[item(0)]).unwrap();
        drop(db);
        let db = DatabaseManager::open(&db_path).unwrap();
        let destination = dir.path().join("report.csv");
        export_to_path(&db, "run", "csv", &destination).unwrap();
        let content = fs::read(&destination).unwrap();
        assert!(export_to_path(&db, "run", "csv", &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), content);
        assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".syncforge-export-")));
    }
    #[test]
    fn sqlite_export_streams_one_hundred_thousand_ordered_rows() {
        use std::io::{BufRead, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let db = fixture_db(dir.path());
        for start in (0..100_000).step_by(1_000) {
            let rows: Vec<_> = (start..start + 1_000)
                .rev()
                .map(|n| {
                    let mut row = item(n);
                    row.message = None;
                    row
                })
                .collect();
            db.insert_run_items(&rows).unwrap();
        }
        drop(db);
        let db = DatabaseManager::open(&dir.path().join("history.db")).unwrap();
        let destination = dir.path().join("large.csv");
        export_to_path(&db, "run", "csv", &destination).unwrap();
        let reader = BufReader::new(fs::File::open(destination).unwrap());
        let mut count = 0usize;
        for line in reader.lines().skip(11) {
            let line = line.unwrap();
            assert_eq!(line, format!("item,\"path,{count:06}\",copyLeftToRight,completed,,42"));
            count += 1;
        }
        assert_eq!(count, 100_000);
    }
    #[test]
    fn sqlite_export_uses_case_insensitive_path_then_id_order_and_all_actions() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture_db(dir.path());
        let mut a = item(2);
        a.path = "alpha".into();
        a.action = "deleteLeft".into();
        let mut b = item(1);
        b.path = "Alpha".into();
        let mut c = item(3);
        c.path = "beta".into();
        db.insert_run_items(&[c, a, b]).unwrap();
        drop(db);
        let db = DatabaseManager::open(&dir.path().join("history.db")).unwrap();
        let destination = dir.path().join("ordered.json");
        export_to_path(&db, "run", "json", &destination).unwrap();
        let value: serde_json::Value =
            serde_json::from_reader(fs::File::open(destination).unwrap()).unwrap();
        let items = value["items"].as_array().unwrap();
        assert_eq!(
            items.iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(),
            ["000001", "000002", "000003"]
        );
        assert_eq!(items[1]["action"], "deleteLeft");
    }
    #[test]
    fn streaming_cursor_retains_snapshot_when_pair_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture_db(dir.path());
        db.insert_run_items(&[item(0), item(1)]).unwrap();
        drop(db);
        let db = DatabaseManager::open(&dir.path().join("history.db")).unwrap();
        let mut ids = Vec::new();
        db.visit_run_items(&report(), |row| {
            ids.push(row.id);
            if ids.len() == 1 {
                db.delete_pair("pair").map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(ids, ["000000", "000001"]);
        assert!(db.get_run("run").unwrap().is_none());
    }
}
