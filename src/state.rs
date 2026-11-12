use crate::{config::Config, jobs::Job};
use serde_json::Value;
use sqlx::PgPool;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub sessions: Arc<RwLock<HashMap<String, i32>>>,
    pub http: reqwest::Client,
    pub config: Arc<Config>,
    pub jobs: mpsc::UnboundedSender<Job>,
    pub outbox: Arc<RwLock<Vec<Value>>>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let (jobs, receiver) = mpsc::unbounded_channel();
        let state = Self {
            pool,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("valid HTTP client configuration"),
            config: Arc::new(config),
            jobs,
            outbox: Arc::new(RwLock::new(Vec::new())),
        };
        crate::jobs::start(state.clone(), receiver);
        state
    }
}
