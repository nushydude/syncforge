use std::path::PathBuf;
use std::sync::Mutex;

use crate::persistence::{Database, PersistenceError};

pub struct AppState {
    pub db: Mutex<Database>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Self, PersistenceError> {
        let db_path = data_dir.join("syncforge.db");
        let db = Database::open(&db_path)?;
        Ok(Self {
            db: Mutex::new(db),
        })
    }
}
