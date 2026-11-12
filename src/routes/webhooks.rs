use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use super::wallet::{body_json, integer};
use crate::{
    error::{ApiError, ApiResult},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/webhooks/bank", post(bank_webhook))
}

fn verify_signature(secret: &[u8], raw: &[u8], signature: Option<&str>) -> bool {
    let Some(signature) = signature.and_then(|value| hex::decode(value).ok()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(raw);
    // HMAC verification compares the full digest in constant time.
    mac.verify_slice(&signature).is_ok()
}

async fn bank_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if !verify_signature(
        state.config.bank_secret.as_bytes(),
        &body,
        headers
            .get("X-Signature")
            .and_then(|value| value.to_str().ok()),
    ) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "bad signature"));
    }
    let data = body_json(&body);
    if data["status"] == "settled" {
        let id = integer(&data["order_id"]);
        let bank_ref = data["bank_ref"].as_str();
        let mut tx = state.pool.begin().await?;
        // Lock orders before payment rows, matching the pay/cancel lock order.
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        sqlx::query("UPDATE payments SET status='settled' WHERE order_id=$1 AND bank_ref=$2")
            .bind(id)
            .bind(bank_ref)
            .execute(&mut *tx)
            .await?;
        // A valid callback can settle a pending order even when no matching
        // payment exists. Preserve the existing paid_amount (including zero).
        sqlx::query("UPDATE orders SET status='paid' WHERE id=$1 AND status='pending'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_cover_exact_body_bytes() {
        let raw = br#"{ "order_id": 7, "status": "settled" }"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(raw);
        let signature = hex::encode(mac.finalize().into_bytes());
        assert!(verify_signature(b"secret", raw, Some(&signature)));
        assert!(!verify_signature(
            b"secret",
            br#"{"order_id":7,"status":"settled"}"#,
            Some(&signature)
        ));
        assert!(!verify_signature(b"wrong", raw, Some(&signature)));
        assert!(!verify_signature(b"secret", raw, None));
        assert!(!verify_signature(b"secret", raw, Some("")));
        assert!(!verify_signature(b"secret", raw, Some("deadbeef")));
        assert!(!verify_signature(b"secret", raw, Some("not hex")));
    }
}
