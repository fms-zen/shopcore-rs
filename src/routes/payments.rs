use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::Row;

use super::wallet::body_json;
use crate::{
    auth::require_user,
    error::{ApiError, ApiResult},
    jobs,
    services::bank,
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/orders/{id}/pay", post(pay_order))
}

async fn pay_order(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let user = require_user(&state, &headers).await?;
    let data = body_json(&body);
    let method = data.get("method").unwrap_or(&Value::Null);
    let method = if data.get("method").is_none() {
        "wallet"
    } else {
        method.as_str().unwrap_or("")
    };
    let mut tx = state.pool.begin().await?;
    // Serialize payment, cancellation, and settlement for the same order. This is
    // a per-order database lock, and no application mutex spans the bank request.
    let order = sqlx::query(
        "SELECT status,total,order_no,notify_url FROM orders WHERE id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))?;
    if order.get::<Option<String>, _>("status").as_deref() != Some("pending") {
        return Err(ApiError::new(StatusCode::CONFLICT, "order not payable"));
    }
    let total: f64 = order.get::<Option<f64>, _>("total").unwrap_or(0.0);
    let bank_ref = match method {
        "wallet" => {
            let balance = sqlx::query_scalar::<_, f64>("UPDATE users SET wallet_balance=COALESCE(wallet_balance,0)-$1 WHERE id=$2 AND COALESCE(wallet_balance,0)>=$1 RETURNING wallet_balance")
                .bind(total).bind(user.id).fetch_optional(&mut *tx).await?;
            if balance.is_none() {
                return Err(ApiError::new(
                    StatusCode::PAYMENT_REQUIRED,
                    "insufficient wallet balance",
                ));
            }
            None
        }
        "bank" => Some(
            bank::charge(&state, id, total)
                .await
                .ok_or_else(|| ApiError::new(StatusCode::BAD_GATEWAY, "payment failed"))?,
        ),
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "unknown method")),
    };
    sqlx::query("INSERT INTO payments (order_id,amount,method,bank_ref,status) VALUES ($1,$2,$3,$4,'captured')")
        .bind(id).bind(total).bind(method).bind(&bank_ref).execute(&mut *tx).await?;
    sqlx::query("UPDATE orders SET status='paid',paid_amount=$1 WHERE id=$2")
        .bind(total)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let order_no: Option<i32> = order.get("order_no");
    let notify_url: Option<String> = order.get("notify_url");
    tx.commit().await?;
    jobs::enqueue(
        &state,
        "email",
        json!({
            "to":user.email,"name":user.name,
            "subject":format!("Payment received for order {}", order_no.map(|n| n.to_string()).unwrap_or_else(|| "None".to_owned())),
            "message":format!("Thanks, we received {total}.")
        }),
    );
    if let Some(url) = notify_url.filter(|url| !url.is_empty()) {
        jobs::enqueue(
            &state,
            "notify",
            json!({"url":url,"body":{"order_id":id,"order_no":order_no,"status":"paid"}}),
        );
    }
    Ok(Json(
        json!({"order_id":id,"status":"paid","method":method,"bank_ref":bank_ref}),
    ))
}
