use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::persistence::{Database, PersistenceError};

pub struct AppState {
    pub db: Mutex<Database>,
    pub cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Self, PersistenceError> {
        let db_path = data_dir.join("syncforge.db");
        let db = Database::open(&db_path)?;
        Ok(Self {
            db: Mutex::new(db),
            cancel_flag: Mutex::new(None),
        })
    }
}
