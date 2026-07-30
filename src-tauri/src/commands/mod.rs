pub mod dialog;
pub mod duplicates;
pub mod history;
pub mod pairs;
pub mod preview;
pub mod run;
pub mod schedule;

#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg(test)]
mod tests {
    use super::greet;

    #[test]
    fn greet_includes_name() {
        assert!(greet("SyncForge").contains("SyncForge"));
    }
}
