use sqlx::SqlitePool;
use minijinja::Environment;
use std::sync::Arc;

pub struct AppState {
    pub db: SqlitePool,
    pub tmpl: Arc<Environment<'static>>,
    pub scan_storage_path: String,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            tmpl: Arc::clone(&self.tmpl),
            scan_storage_path: self.scan_storage_path.clone(),
        }
    }
}
