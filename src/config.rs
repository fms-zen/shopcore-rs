use std::{env, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bank_url: String,
    pub bank_secret: String,
    pub secret_key: String,
    pub export_dir: PathBuf,
    pub admin_user: String,
    pub admin_password: String,
    pub bind_addr: String,
}

impl Config {
    pub fn from_env() -> Self {
        fn env_or(key: &str, default: &str) -> String {
            env::var(key).unwrap_or_else(|_| default.to_owned())
        }
        Self {
            database_url: env_or("DATABASE_URL", "postgresql://shop:shop@localhost:5432/shop"),
            bank_url: env_or("BANK_URL", "http://localhost:5001"),
            bank_secret: env_or("BANK_SECRET", "bank-shared-secret-2019"),
            secret_key: env_or("SECRET_KEY", "shopcore-dev-secret"),
            export_dir: PathBuf::from(env_or("EXPORT_DIR", "/tmp/exports")),
            admin_user: env_or("ADMIN_USER", "admin"),
            admin_password: env_or("ADMIN_PASSWORD", "admin123"),
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8000"),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::from_env()
    }
}
