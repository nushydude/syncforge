use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::persistence::{Database, PersistenceError};
use crate::watcher::WatchService;

pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
    /// Pair ids whose debounced watch sync could not start while a run was active.
    pub pending_watch_syncs: Mutex<HashSet<String>>,
    pub watch_service: Mutex<Option<WatchService>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Self, PersistenceError> {
        let db_path = data_dir.join("syncforge.db");
        let db = Database::open(&db_path)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            cancel_flag: Mutex::new(None),
            pending_watch_syncs: Mutex::new(HashSet::new()),
            watch_service: Mutex::new(None),
        })
    }
}
