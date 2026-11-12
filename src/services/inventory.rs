use sqlx::{Postgres, Row, Transaction};

use crate::error::{ApiError, ApiResult};

/// Acquire product locks in the same order used when reserving stock.
pub async fn restock(tx: &mut Transaction<'_, Postgres>, order_id: i32) -> ApiResult<()> {
    let items = sqlx::query(
        "SELECT product_id, SUM(quantity)::BIGINT AS quantity FROM order_items \
         WHERE order_id=$1 GROUP BY product_id ORDER BY product_id",
    )
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    for item in items {
        let product_id: i32 = item.try_get("product_id")?;
        let quantity: i64 = item.try_get("quantity")?;
        sqlx::query("UPDATE products SET stock=stock+$1 WHERE id=$2")
            .bind(i32::try_from(quantity).map_err(|_| ApiError::bad_request("invalid quantity"))?)
            .bind(product_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Atomic arithmetic also locks the user, avoiding lost concurrent credits.
pub async fn credit_wallet(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i32,
    amount: f64,
) -> ApiResult<f64> {
    let row = sqlx::query(
        "UPDATE users SET wallet_balance=COALESCE(wallet_balance,0)+$1 \
         WHERE id=$2 RETURNING wallet_balance",
    )
    .bind(amount)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    Ok(row.try_get("wallet_balance")?)
}
