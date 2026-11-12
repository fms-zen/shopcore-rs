use std::time::Duration;

use axum::http::StatusCode;
use serde_json::Value;

use crate::error::{ApiError, ApiResult};

pub async fn send(client: &reqwest::Client, url: &str, body: &Value) -> ApiResult<()> {
    client
        .post(url)
        .timeout(Duration::from_secs(3))
        .json(body)
        .send()
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "notification failed"))?;
    Ok(())
}
