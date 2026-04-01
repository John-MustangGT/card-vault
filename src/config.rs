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
            host: resolve_host(
                &std::env::var("HOST").unwrap_or_else(|_| "localhost".into())
            ),
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()?,
        })
    }
}

/// Resolve friendly HOST aliases to bind addresses.
///
/// | Value       | Binds to    | Accessible from              |
/// |-------------|-------------|------------------------------|
/// | `localhost` | 127.0.0.1   | This machine only (default)  |
/// | `localnet`  | 0.0.0.0     | Any machine on your LAN      |
/// | `any`       | 0.0.0.0     | Any network (open to WAN)    |
/// | `<ip>`      | that IP     | Specific interface           |
fn resolve_host(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "localhost" => "127.0.0.1".into(),
        "localnet" | "lan" => "0.0.0.0".into(),
        "any" | "0.0.0.0" => "0.0.0.0".into(),
        other => other.to_string(),
    }
}
