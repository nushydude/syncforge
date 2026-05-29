use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::persistence::{Database, PersistenceError};
use crate::watcher::WatchService;

pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
    pub watch_service: Mutex<Option<WatchService>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Self, PersistenceError> {
        let db_path = data_dir.join("syncforge.db");
        let db = Database::open(&db_path)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            cancel_flag: Mutex::new(None),
            watch_service: Mutex::new(None),
        })
    }
}
