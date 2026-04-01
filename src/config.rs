use anyhow::Result;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub scan_storage_path: String,
    pub data_dir: String,
    pub host: String,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:./card-vault.db".into()),
            scan_storage_path: std::env::var("SCAN_STORAGE_PATH")
                .unwrap_or_else(|_| "./scans".into()),
            data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".into()),
            host: std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()?,
        })
    }
}
