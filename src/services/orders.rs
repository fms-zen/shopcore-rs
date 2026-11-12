use std::collections::BTreeMap;

use chrono::{Local, NaiveDateTime};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};

use crate::{
    auth::AuthUser,
    db::format_timestamp,
    error::{ApiError, ApiResult},
    jobs,
    services::{inventory, pricing},
    state::AppState,
};

pub async fn load_items(pool: &PgPool, order_id: i32) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT i.product_id,p.sku,p.name,i.quantity,i.price FROM order_items i \
         LEFT JOIN products p ON p.id=i.product_id WHERE i.order_id=$1 ORDER BY i.id",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(json!({
                "product_id": r.try_get::<i32,_>("product_id")?,
                "sku": r.try_get::<Option<String>,_>("sku")?,
                "name": r.try_get::<Option<String>,_>("name")?,
                "quantity": r.try_get::<i32,_>("quantity")?,
                "price": r.try_get::<f64,_>("price")?,
            }))
        })
        .collect()
}

pub async fn load_order(pool: &PgPool, order_id: i32) -> ApiResult<Option<Value>> {
    let row = sqlx::query("SELECT * FROM orders WHERE id=$1")
        .bind(order_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let timestamp: Option<NaiveDateTime> = row.try_get("created_at")?;
    Ok(Some(json!({
        "id": row.try_get::<i32,_>("id")?,
        "order_no": row.try_get::<i32,_>("order_no")?,
        "user_id": row.try_get::<i32,_>("user_id")?,
        "status": row.try_get::<String,_>("status")?,
        "total": row.try_get::<f64,_>("total")?,
        "paid_amount": row.try_get::<Option<f64>,_>("paid_amount")?,
        "refunded": row.try_get::<Option<f64>,_>("refunded")?,
        "coupon": row.try_get::<Option<String>,_>("coupon")?,
        "created_at": timestamp.map(format_timestamp),
        "items": load_items(pool,order_id).await?,
    })))
}

fn positive_quantity(value: Option<&Value>) -> ApiResult<i32> {
    let Some(value) = value else { return Ok(1) };
    let quantity = match value {
        Value::Number(n) => n.as_i64().and_then(|n| i32::try_from(n).ok()),
        Value::String(s) => s.trim().parse::<i32>().ok(),
        _ => None,
    };
    quantity
        .filter(|n| *n > 0)
        .ok_or_else(|| ApiError::bad_request("invalid quantity"))
}

fn product_id(value: Option<&Value>) -> i32 {
    match value {
        Some(Value::Number(n)) => n.as_i64().and_then(|n| i32::try_from(n).ok()).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

pub async fn create(state: &AppState, user: &AuthUser, data: &Value) -> ApiResult<Value> {
    let items = data
        .get("items")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| ApiError::bad_request("items required"))?;
    let mut requested = Vec::with_capacity(items.len());
    let mut quantities = BTreeMap::<i32, i32>::new();
    for item in items {
        let pid = product_id(item.get("product_id"));
        let quantity = positive_quantity(item.get("quantity"))?;
        let count = quantities.entry(pid).or_default();
        *count = count
            .checked_add(quantity)
            .ok_or_else(|| ApiError::bad_request("invalid quantity"))?;
        requested.push((pid, quantity));
    }
    let coupon = data.get("coupon").and_then(Value::as_str);
    let notify_url = data.get("notify_url").and_then(Value::as_str);
    let mut tx = state.pool.begin().await?;
    let mut percent = 0;
    let mut coupon_id = None;
    if let Some(code) = coupon.filter(|c| !c.is_empty()) {
        let row = sqlx::query("SELECT * FROM coupons WHERE code=$1 ORDER BY id LIMIT 1 FOR UPDATE")
            .bind(code)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::bad_request("invalid coupon"))?;
        let expires: String = row.try_get("expires")?;
        if expires < Local::now().date_naive().to_string() {
            return Err(ApiError::bad_request("coupon expired"));
        }
        if row.try_get::<bool, _>("single_use")? && row.try_get::<i32, _>("used_count")? > 0 {
            return Err(ApiError::conflict("coupon already used"));
        }
        percent = row.try_get("percent")?;
        coupon_id = Some(row.try_get::<i32, _>("id")?);
    }
    let mut products = BTreeMap::<i32, f64>::new();
    for (pid, quantity) in &quantities {
        let row = sqlx::query("SELECT sku,price,stock FROM products WHERE id=$1 FOR UPDATE")
            .bind(pid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    format!("product {pid} not found"),
                )
            })?;
        let price: f64 = row.try_get("price")?;
        if !price.is_finite() || price < 0.0 {
            return Err(ApiError::bad_request("invalid product price"));
        }
        if row.try_get::<i32, _>("stock")? < *quantity {
            return Err(ApiError::conflict(format!(
                "insufficient stock for {}",
                row.try_get::<Option<String>, _>("sku")?
                    .unwrap_or_else(|| "None".to_owned())
            )));
        }
        products.insert(*pid, price);
    }
    let subtotal: f64 = requested
        .iter()
        .map(|(pid, qty)| pricing::line_total(products[pid], *qty))
        .sum();
    let total = pricing::calc_total(subtotal, percent);
    if !total.is_finite() || total < 0.0 {
        return Err(ApiError::bad_request("invalid total"));
    }
    // A transaction-scoped advisory lock serializes MAX+1 without changing the schema.
    sqlx::query("SELECT pg_advisory_xact_lock(1936224114, 1)")
        .execute(&mut *tx)
        .await?;
    let order_no: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(order_no),1000)+1 FROM orders")
        .fetch_one(&mut *tx)
        .await?;
    let order_id: i32 = sqlx::query_scalar(
        "INSERT INTO orders (order_no,user_id,status,total,coupon,notify_url,created_at) \
         VALUES ($1,$2,'pending',$3,$4,$5,$6) RETURNING id",
    )
    .bind(order_no)
    .bind(user.id)
    .bind(total)
    .bind(coupon)
    .bind(notify_url)
    .bind(Local::now().naive_local())
    .fetch_one(&mut *tx)
    .await?;
    for (pid, quantity) in requested {
        sqlx::query(
            "INSERT INTO order_items (order_id,product_id,quantity,price) VALUES ($1,$2,$3,$4)",
        )
        .bind(order_id)
        .bind(pid)
        .bind(quantity)
        .bind(products[&pid])
        .execute(&mut *tx)
        .await?;
    }
    for (pid, quantity) in quantities {
        sqlx::query("UPDATE products SET stock=stock-$1 WHERE id=$2")
            .bind(quantity)
            .bind(pid)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(id) = coupon_id {
        sqlx::query("UPDATE coupons SET used_count=used_count+1 WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    jobs::enqueue(
        state,
        "email",
        json!({"to":user.email,"name":user.name,
        "subject":format!("Order {order_no} received"),
        "message":format!("We received your order totalling {total}.")}),
    );
    if let Some(url) = notify_url.filter(|url| !url.is_empty()) {
        jobs::enqueue(
            state,
            "notify",
            json!({"url":url,"body":{"order_id":order_id,"order_no":order_no,"status":"pending"}}),
        );
    }
    load_order(&state.pool, order_id)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn cancel(state: &AppState, order_id: i32) -> ApiResult<Value> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT user_id,status,total,refunded,order_no FROM orders WHERE id=$1 FOR UPDATE",
    )
    .bind(order_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let status: String = row.try_get("status")?;
    if status == "cancelled" {
        return Err(ApiError::conflict("already cancelled"));
    }
    inventory::restock(&mut tx, order_id).await?;
    if status == "paid" || status == "shipped" {
        let total: f64 = row.try_get("total")?;
        let refunded = row.try_get::<Option<f64>, _>("refunded")?.unwrap_or(0.0);
        inventory::credit_wallet(
            &mut tx,
            row.try_get("user_id")?,
            (total - refunded).max(0.0),
        )
        .await?;
    }
    sqlx::query("UPDATE orders SET status='cancelled' WHERE id=$1")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    let order_no: i32 = row.try_get("order_no")?;
    tx.commit().await?;
    jobs::enqueue(
        state,
        "email",
        json!({"to":null,"subject":format!("Order {order_no} cancelled"),"message":"Your order was cancelled."}),
    );
    load_order(&state.pool, order_id)
        .await?
        .ok_or_else(ApiError::not_found)
}

fn refund_amount(value: Option<&Value>, total: f64) -> ApiResult<f64> {
    let amount = match value {
        None => Some(total),
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    };
    amount
        .filter(|a| a.is_finite() && *a > 0.0)
        .ok_or_else(|| ApiError::bad_request("invalid amount"))
}

pub async fn refund(state: &AppState, order_id: i32, data: &Value) -> ApiResult<Value> {
    let mut tx = state.pool.begin().await?;
    let row =
        sqlx::query("SELECT user_id,status,total,refunded FROM orders WHERE id=$1 FOR UPDATE")
            .bind(order_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let status: String = row.try_get("status")?;
    if status == "pending" {
        return Err(ApiError::conflict("order not paid"));
    }
    if status == "cancelled" {
        return Err(ApiError::conflict("order cancelled"));
    }
    let total: f64 = row.try_get("total")?;
    let refunded: f64 = row.try_get::<Option<f64>, _>("refunded")?.unwrap_or(0.0);
    let amount = refund_amount(data.get("amount"), total)?;
    let cumulative = refunded + amount;
    // Permit harmless binary representation noise, but never credit it above the total.
    let tolerance = f64::EPSILON * total.abs().max(1.0) * 4.0;
    if !cumulative.is_finite() || cumulative > total + tolerance {
        return Err(ApiError::conflict("refund exceeds order total"));
    }
    let credited = amount.min((total - refunded).max(0.0));
    if credited <= 0.0 {
        return Err(ApiError::conflict("refund exceeds order total"));
    }
    inventory::credit_wallet(&mut tx, row.try_get("user_id")?, credited).await?;
    sqlx::query("UPDATE orders SET status='refunded',refunded=$1 WHERE id=$2")
        .bind((refunded + credited).min(total))
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    load_order(&state.pool, order_id)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_defaults_and_validation() {
        assert_eq!(positive_quantity(None).unwrap(), 1);
        assert_eq!(positive_quantity(Some(&json!(2))).unwrap(), 2);
        for value in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!(null),
            json!("x"),
            json!(true),
        ] {
            assert!(positive_quantity(Some(&value)).is_err());
        }
    }

    #[test]
    fn refund_defaults_and_validation() {
        assert_eq!(refund_amount(None, 12.5).unwrap(), 12.5);
        for value in [
            json!(-1),
            json!(0),
            json!(null),
            json!("NaN"),
            json!("inf"),
            json!("bad"),
        ] {
            assert!(refund_amount(Some(&value), 12.5).is_err());
        }
    }
}
