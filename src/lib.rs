pub mod auth;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
pub mod jobs;
pub mod routes;
pub mod services;
pub mod state;

pub use error::{json_response, ApiError, ApiResult};
pub use state::AppState;

use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .merge(routes::users::routes())
        .merge(routes::products::routes())
        .merge(routes::orders::routes())
        .merge(routes::payments::routes())
        .merge(routes::wallet::routes())
        .merge(routes::reports::routes())
        .merge(routes::admin::routes())
        .merge(routes::webhooks::routes())
        .fallback(|| async {
            (StatusCode::NOT_FOUND, Json(json!({"error":"not found"}))).into_response()
        })
        .layer(middleware::from_fn(json_errors))
        .with_state(state)
}

// Axum extractor and method errors also follow the public JSON error contract.
async fn json_errors(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
    {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    let error = parts
        .status
        .canonical_reason()
        .unwrap_or("request failed")
        .to_lowercase();
    let bytes = serde_json::to_vec(&json!({"error":error})).expect("serializable error");
    parts
        .headers
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_and_framework_failures_use_json() {
        let config = config::Config::from_env();
        let pool = PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .unwrap();
        let app = app(AppState::new(pool, config));
        for (method, path, status) in [
            ("GET", "/health", StatusCode::OK),
            ("GET", "/missing", StatusCode::NOT_FOUND),
            ("POST", "/health", StatusCode::METHOD_NOT_ALLOWED),
            (
                "GET",
                "/api/products/not-an-integer",
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), status, "{method} {path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if status.is_client_error() {
                assert!(value["error"].is_string());
            }
        }
    }
}
