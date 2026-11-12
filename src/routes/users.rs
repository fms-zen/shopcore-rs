use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    auth::{hash_password, make_token, require_user, AuthUser},
    error::{json_response, ApiError, ApiResult},
    AppState,
};

// Also taken by profile email changes: the legacy schema has no unique indexes.
const ACCOUNT_LOCK: i64 = 0x5348_4f50_5553_4552;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/register", post(register))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/users/me", get(me).patch(update_me))
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

fn public_user(user: &AuthUser) -> Value {
    json!({
        "id": user.id, "username": user.username, "email": user.email,
        "name": user.name, "role": user.role, "wallet_balance": user.wallet_balance,
    })
}

async fn register(State(state): State<AppState>, payload: Bytes) -> ApiResult<Response> {
    let data = body(payload)?;
    let username = data
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let email = data
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = data.get("password").and_then(Value::as_str).unwrap_or("");
    if username.is_empty() || email.is_empty() || password.is_empty() {
        return Err(ApiError::bad_request(
            "username, email and password are required",
        ));
    }
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(username);
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ACCOUNT_LOCK)
        .execute(&mut *tx)
        .await?;
    if sqlx::query("SELECT id FROM users WHERE username=$1")
        .bind(username)
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("username taken"));
    }
    if sqlx::query("SELECT id FROM users WHERE email=$1")
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("email taken"));
    }
    let row = sqlx::query("INSERT INTO users (username,email,name,password_hash,role,wallet_balance) VALUES ($1,$2,$3,$4,'user',0) RETURNING id")
        .bind(username).bind(email).bind(name).bind(hash_password(password)).fetch_one(&mut *tx).await?;
    let id: i32 = row.try_get("id")?;
    tx.commit().await?;
    Ok(json_response(
        StatusCode::CREATED,
        json!({"id": id, "username": username}),
    ))
}

async fn login(State(state): State<AppState>, payload: Bytes) -> ApiResult<Response> {
    let data = body(payload)?;
    let username = data.get("username").and_then(Value::as_str);
    let password = data.get("password").and_then(Value::as_str).unwrap_or("");
    let row = sqlx::query(
        "SELECT id, username FROM users WHERE username=$1 AND password_hash=$2 ORDER BY id LIMIT 1",
    )
    .bind(username)
    .bind(hash_password(password))
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "invalid credentials"))?;
    let id: i32 = row.try_get("id")?;
    let username: String = row.try_get("username")?;
    let token = make_token(&username);
    state.sessions.write().await.insert(token.clone(), id);
    Ok(json_response(StatusCode::OK, json!({"token": token})))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    require_user(&state, &headers).await?;
    if let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        state.sessions.write().await.remove(token);
    }
    Ok(json_response(StatusCode::OK, json!({"ok": true})))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let user = require_user(&state, &headers).await?;
    Ok(json_response(StatusCode::OK, public_user(&user)))
}

async fn update_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Bytes,
) -> ApiResult<Response> {
    let user = require_user(&state, &headers).await?;
    let data = body(payload)?;
    // Extra keys are ignored. Only these two fixed SQL statements can update a profile.
    for key in ["name", "email"] {
        if let Some(value) = data.get(key) {
            if !value.is_string() && !value.is_null() {
                return Err(ApiError::bad_request(format!("{key} must be a string")));
            }
        }
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ACCOUNT_LOCK)
        .execute(&mut *tx)
        .await?;
    if let Some(email) = data.get("email") {
        let email = email.as_str();
        if sqlx::query("SELECT id FROM users WHERE email=$1 AND id<>$2")
            .bind(email)
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        {
            return Err(ApiError::conflict("email taken"));
        }
        sqlx::query("UPDATE users SET email=$1 WHERE id=$2")
            .bind(email)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(name) = data.get("name") {
        sqlx::query("UPDATE users SET name=$1 WHERE id=$2")
            .bind(name.as_str())
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    let row =
        sqlx::query("SELECT id,username,email,name,role,wallet_balance FROM users WHERE id=$1")
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
    let value = crate::db::row_to_json(&row);
    tx.commit().await?;
    Ok(json_response(StatusCode::OK, value))
}
