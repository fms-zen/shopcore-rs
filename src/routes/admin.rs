use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde_json::Value;
use sqlx::Row;

use crate::{
    auth::require_admin,
    db::row_to_json,
    error::{json_response, ApiError, ApiResult},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/users", get(list_users))
        .route("/api/admin/users/{id}/role", post(set_role))
        .route("/api/admin/orders/{id}/status", post(set_order_status))
}

fn body(payload: Bytes) -> ApiResult<Value> {
    let value: Value = if payload.iter().all(u8::is_ascii_whitespace) {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&payload).map_err(|_| ApiError::bad_request("invalid JSON body"))?
    };
    if !value.is_object() {
        return Err(ApiError::bad_request("JSON body must be an object"));
    }
    Ok(value)
}

async fn list_users(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let rows = sqlx::query(
        "SELECT id,username,email,name,role,wallet_balance,created_at FROM users ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(json_response(
        StatusCode::OK,
        Value::Array(rows.iter().map(row_to_json).collect()),
    ))
}

async fn set_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
    payload: Bytes,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let data = body(payload)?;
    let role = match data.get("role") {
        None => "user",
        Some(Value::String(role)) if role == "user" || role == "admin" => role,
        _ => return Err(ApiError::bad_request("role must be user or admin")),
    };
    let row = sqlx::query("UPDATE users SET role=$1 WHERE id=$2 RETURNING id,username,role")
        .bind(role)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(json_response(StatusCode::OK, row_to_json(&row)))
}

async fn set_order_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
    payload: Bytes,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let data = body(payload)?;
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("status is required"))?;
    let mut tx = state.pool.begin().await?;
    let old = sqlx::query("SELECT status FROM orders WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let old_status: Option<String> = old.try_get("status")?;
    // Cancellation already returned stock; reopening would allow another restock.
    if old_status.as_deref() == Some("cancelled") && status != "cancelled" {
        return Err(ApiError::conflict("cancelled orders cannot be reopened"));
    }
    let row = sqlx::query("UPDATE orders SET status=$1 WHERE id=$2 RETURNING id,order_no,status")
        .bind(status)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let value = row_to_json(&row);
    tx.commit().await?;
    Ok(json_response(StatusCode::OK, value))
}
