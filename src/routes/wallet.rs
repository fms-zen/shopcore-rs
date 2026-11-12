use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    auth::require_user,
    error::{ApiError, ApiResult},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/wallet", get(get_wallet))
        .route("/api/wallet/topup", post(topup))
        .route("/api/wallet/transfer", post(transfer))
}

pub(crate) fn body_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|_| json!({}))
}

pub(crate) fn money(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
}

pub(crate) fn integer(value: &Value) -> i32 {
    value
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .or_else(|| value.as_f64().filter(|n| n.is_finite()).map(|n| n as i32))
        .or_else(|| {
            value.as_str().and_then(|v| {
                let trimmed = v.trim_start();
                let end = trimmed
                    .char_indices()
                    .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && *c == '-'))
                    .map(|(i, c)| i + c.len_utf8())
                    .last()?;
                trimmed[..end].parse().ok()
            })
        })
        .unwrap_or(0)
}

async fn get_wallet(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(json!({"balance": user.wallet_balance})))
}

async fn topup(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let user = require_user(&state, &headers).await?;
    let amount = money(&body_json(&body)["amount"]);
    if amount <= 0.0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "amount must be positive",
        ));
    }
    let balance: f64 = sqlx::query_scalar("UPDATE users SET wallet_balance=COALESCE(wallet_balance,0)+$1 WHERE id=$2 RETURNING wallet_balance")
        .bind(amount).bind(user.id).fetch_one(&state.pool).await?;
    Ok(Json(json!({"balance": balance})))
}

async fn transfer(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let user = require_user(&state, &headers).await?;
    let data = body_json(&body);
    let to_id = integer(&data["to_user_id"]);
    let amount = money(&data["amount"]);
    if amount <= 0.0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "amount must be positive",
        ));
    }
    let mut tx = state.pool.begin().await?;
    // Every transfer takes user locks in the same order, including opposing transfers.
    let rows = sqlx::query("SELECT id,COALESCE(wallet_balance,0)::float8 AS balance FROM users WHERE id=$1 OR id=$2 ORDER BY id FOR UPDATE")
        .bind(user.id).bind(to_id).fetch_all(&mut *tx).await?;
    if !rows.iter().any(|row| row.get::<i32, _>("id") == to_id) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "recipient not found"));
    }
    let balance = rows
        .iter()
        .find(|row| row.get::<i32, _>("id") == user.id)
        .map(|row| row.get::<f64, _>("balance"))
        .unwrap_or(0.0);
    if balance < amount {
        return Err(ApiError::new(
            StatusCode::PAYMENT_REQUIRED,
            "insufficient balance",
        ));
    }
    // A transfer to oneself is a no-op, rather than two stale balance writes.
    let new_balance = if user.id == to_id {
        balance
    } else {
        sqlx::query("UPDATE users SET wallet_balance=COALESCE(wallet_balance,0)-$1 WHERE id=$2")
            .bind(amount)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE users SET wallet_balance=COALESCE(wallet_balance,0)+$1 WHERE id=$2")
            .bind(amount)
            .bind(to_id)
            .execute(&mut *tx)
            .await?;
        balance - amount
    };
    tx.commit().await?;
    Ok(Json(json!({"balance": new_balance})))
}
