use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::http::HeaderMap;
use sqlx::Row;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i32,
    pub username: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
    pub wallet_balance: f64,
}
impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}
pub fn hash_password(password: &str) -> String {
    format!("{:x}", md5::compute(password.as_bytes()))
}
pub fn make_token(_username: &str) -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub async fn require_user(state: &AppState, headers: &HeaderMap) -> ApiResult<AuthUser> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty())
        .ok_or_else(ApiError::unauthorized)?;
    let id = state
        .sessions
        .read()
        .await
        .get(token)
        .copied()
        .ok_or_else(ApiError::unauthorized)?;
    let row =
        sqlx::query("SELECT id,username,email,name,role,wallet_balance FROM users WHERE id=$1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
    Ok(AuthUser {
        id: row.get("id"),
        username: row.try_get("username").unwrap_or_default(),
        email: row.get("email"),
        name: row.get("name"),
        role: row.try_get("role").unwrap_or_else(|_| "user".into()),
        wallet_balance: row.try_get("wallet_balance").unwrap_or(0.0),
    })
}
pub async fn require_admin(state: &AppState, headers: &HeaderMap) -> ApiResult<AuthUser> {
    let user = require_user(state, headers).await?;
    if !user.is_admin() {
        return Err(ApiError::forbidden());
    }
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_legacy_password_hashes_and_generates_distinct_tokens() {
        assert_eq!(
            hash_password("admin123"),
            "0192023a7bbd73250516f069df18b500"
        );
        assert_ne!(make_token("same"), make_token("same"));
    }
}
