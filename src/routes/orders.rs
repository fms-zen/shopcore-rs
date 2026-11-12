use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    auth::require_user,
    error::{json_response, ApiError, ApiResult},
    services::orders,
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/orders", get(list).post(create))
        .route("/api/orders/{id}", get(detail))
        .route("/api/orders/{id}/invoice", get(invoice))
        .route("/api/orders/{id}/cancel", post(cancel))
        .route("/api/orders/{id}/refund", post(refund))
}

fn request_data(body: &[u8]) -> ApiResult<Value> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let value: Value =
        serde_json::from_slice(body).map_err(|_| ApiError::bad_request("invalid JSON"))?;
    if !value.is_object() {
        return Err(ApiError::bad_request("JSON object required"));
    }
    Ok(value)
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let user = require_user(&state, &headers).await?;
    let data = request_data(&body)?;
    Ok(json_response(
        StatusCode::CREATED,
        orders::create(&state, &user, &data).await?,
    ))
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let user = require_user(&state, &headers).await?;
    let page = query
        .get("page")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    let limit = query
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(20);
    let rows = if limit < 0 {
        sqlx::query("SELECT id FROM orders WHERE user_id=$1 ORDER BY id DESC")
            .bind(user.id)
            .fetch_all(&state.pool)
            .await?
    } else {
        sqlx::query("SELECT id FROM orders WHERE user_id=$1 ORDER BY id DESC LIMIT $2 OFFSET $3")
            .bind(user.id)
            .bind(limit)
            .bind(page.saturating_sub(1).max(0).saturating_mul(limit))
            .fetch_all(&state.pool)
            .await?
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(order) = orders::load_order(&state.pool, row.try_get("id")?).await? {
            out.push(order);
        }
    }
    Ok(json_response(StatusCode::OK, json!(out)))
}

async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
) -> ApiResult<Response> {
    require_user(&state, &headers).await?;
    let order = orders::load_order(&state.pool, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(json_response(StatusCode::OK, order))
}

async fn invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
) -> ApiResult<Response> {
    require_user(&state, &headers).await?;
    let order = orders::load_order(&state.pool, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let customer = sqlx::query("SELECT name,email FROM users WHERE id=$1")
        .bind(order["user_id"].as_i64().unwrap_or_default() as i32)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let lines:Vec<Value> = order["items"].as_array().unwrap().iter().map(|item|json!({
        "sku":item["sku"],"name":item["name"],"quantity":item["quantity"],"price":item["price"],
        "line_total":item["price"].as_f64().unwrap_or(0.0)*item["quantity"].as_f64().unwrap_or(0.0),
    })).collect();
    Ok(json_response(
        StatusCode::OK,
        json!({"order_no":order["order_no"],"customer":{
        "name":customer.try_get::<Option<String>,_>("name")?,"email":customer.try_get::<Option<String>,_>("email")?},
        "lines":lines,"total":order["total"],"status":order["status"]}),
    ))
}

async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
) -> ApiResult<Response> {
    require_user(&state, &headers).await?;
    Ok(json_response(
        StatusCode::OK,
        orders::cancel(&state, id).await?,
    ))
}

async fn refund(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
    body: Bytes,
) -> ApiResult<Response> {
    require_user(&state, &headers).await?;
    let data = request_data(&body)?;
    Ok(json_response(
        StatusCode::OK,
        orders::refund(&state, id, &data).await?,
    ))
}
