use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{rejection::QueryRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde_json::Value;
use sqlx::{postgres::PgRow, Row};

use crate::{
    auth::require_admin,
    db::row_to_json,
    error::{json_response, ApiError, ApiResult},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/products", get(list_products).post(create_product))
        .route("/api/products/{id}", get(get_product).put(update_product))
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

fn integer(value: Option<&Value>, default: i64) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|n| n.is_finite()).map(|n| n as i64))
            .unwrap_or(default),
        Some(Value::Bool(value)) => i64::from(*value),
        Some(Value::String(value)) => parse_integer(value, default),
        _ => default,
    }
}

fn parse_integer(value: &str, default: i64) -> i64 {
    let value = value.trim();
    if let Ok(number) = value.parse() {
        return number;
    }
    // Preserve the legacy to_int numeric-prefix fallback for query/form strings.
    let prefix_len = value
        .char_indices()
        .take_while(|(index, c)| c.is_ascii_digit() || (*index == 0 && *c == '-'))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    value[..prefix_len].parse().unwrap_or(default)
}

fn price(value: Option<&Value>, default: f64) -> ApiResult<f64> {
    let number = match value {
        None => Some(default),
        Some(Value::Number(value)) => value.as_f64(),
        Some(Value::String(value)) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite() && *value >= 0.0);
    number.ok_or_else(|| ApiError::bad_request("price must be a non-negative finite number"))
}

fn text(value: Option<&Value>) -> ApiResult<Option<&str>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        _ => Err(ApiError::bad_request("sku and name must be strings")),
    }
}

async fn list_products(
    State(state): State<AppState>,
    params: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> ApiResult<Response> {
    let Query(params) = params.map_err(|_| ApiError::bad_request("invalid query parameters"))?;
    let sort = params.get("sort").map(String::as_str).unwrap_or("id");
    if !matches!(sort, "id" | "name" | "price" | "stock" | "sku") {
        return Err(ApiError::bad_request("invalid sort"));
    }
    let page = params.get("page").map(|s| parse_integer(s, 1)).unwrap_or(1);
    let limit = params
        .get("limit")
        .map(|s| parse_integer(s, 20))
        .unwrap_or(20);
    if page < 1 {
        return Err(ApiError::bad_request("page must be at least 1"));
    }
    let offset = (page - 1)
        .checked_mul(limit.max(0))
        .ok_or_else(|| ApiError::bad_request("pagination is too large"))?;
    let search = params
        .get("q")
        .filter(|q| !q.is_empty())
        .map(|q| format!("%{q}%"));
    // sort has been checked against the documented identifier allowlist above.
    let sql = format!("SELECT * FROM products WHERE ($1::text IS NULL OR name ILIKE $1) ORDER BY {sort} LIMIT $2 OFFSET $3");
    let rows = sqlx::query(&sql)
        .bind(search)
        .bind(if limit < 0 { None } else { Some(limit) })
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;
    Ok(json_response(
        StatusCode::OK,
        Value::Array(rows.iter().map(row_to_json).collect()),
    ))
}

async fn find_product(state: &AppState, id: i32) -> ApiResult<PgRow> {
    sqlx::query("SELECT * FROM products WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_product(State(state): State<AppState>, Path(id): Path<i32>) -> ApiResult<Response> {
    let row = find_product(&state, id).await?;
    Ok(json_response(StatusCode::OK, row_to_json(&row)))
}

async fn create_product(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Bytes,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let data = body(payload)?;
    let price = price(data.get("price"), 0.0)?;
    let stock = i32::try_from(integer(data.get("stock"), 0))
        .map_err(|_| ApiError::bad_request("stock is out of range"))?;
    let row =
        sqlx::query("INSERT INTO products (sku,name,price,stock) VALUES ($1,$2,$3,$4) RETURNING *")
            .bind(text(data.get("sku"))?)
            .bind(text(data.get("name"))?)
            .bind(price)
            .bind(stock)
            .fetch_one(&state.pool)
            .await?;
    Ok(json_response(StatusCode::CREATED, row_to_json(&row)))
}

async fn update_product(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
    payload: Bytes,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let data = body(payload)?;
    let mut tx = state.pool.begin().await?;
    let old = sqlx::query("SELECT * FROM products WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let old_price: Option<f64> = old.try_get("price")?;
    let old_stock: Option<i32> = old.try_get("stock")?;
    let old_sku: Option<String> = old.try_get("sku")?;
    let old_name: Option<String> = old.try_get("name")?;
    let price = price(data.get("price"), old_price.unwrap_or(0.0))?;
    let stock = i32::try_from(integer(
        data.get("stock"),
        i64::from(old_stock.unwrap_or(0)),
    ))
    .map_err(|_| ApiError::bad_request("stock is out of range"))?;
    let sku = if data.get("sku").is_some() {
        text(data.get("sku"))?
    } else {
        old_sku.as_deref()
    };
    let name = if data.get("name").is_some() {
        text(data.get("name"))?
    } else {
        old_name.as_deref()
    };
    let row =
        sqlx::query("UPDATE products SET sku=$1,name=$2,price=$3,stock=$4 WHERE id=$5 RETURNING *")
            .bind(sku)
            .bind(name)
            .bind(price)
            .bind(stock)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let result = row_to_json(&row);
    tx.commit().await?;
    Ok(json_response(StatusCode::OK, result))
}
