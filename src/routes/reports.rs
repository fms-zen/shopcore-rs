use axum::{
    body::{Body, Bytes},
    extract::{rejection::QueryRejection, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    auth::require_admin,
    error::{json_response, ApiError, ApiResult},
    services::exporter::{self, DEFAULT_FROM, DEFAULT_TO},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/reports/sales", get(sales))
        .route("/api/reports/export", post(export))
        .route("/api/reports/download", get(download))
}

#[derive(Default, Deserialize)]
struct SalesQuery {
    from: Option<String>,
    to: Option<String>,
    group_by: Option<String>,
}

fn bucket_sql(group: &str) -> ApiResult<&'static str> {
    match group {
        "day" => Ok("SELECT date(created_at)::text AS bucket, COUNT(*) AS orders, SUM(total) AS revenue FROM orders WHERE created_at >= $1 AND created_at < $2 GROUP BY date(created_at) ORDER BY date(created_at)"),
        "status" => Ok("SELECT status::text AS bucket, COUNT(*) AS orders, SUM(total) AS revenue FROM orders WHERE created_at >= $1 AND created_at < $2 GROUP BY status ORDER BY status"),
        "user" => Ok("SELECT user_id::text AS bucket, COUNT(*) AS orders, SUM(total) AS revenue FROM orders WHERE created_at >= $1 AND created_at < $2 GROUP BY user_id ORDER BY user_id"),
        _ => Err(ApiError::bad_request("invalid group_by")),
    }
}

async fn sales(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<SalesQuery>, QueryRejection>,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::bad_request("invalid report query"))?;
    let from = query.from.as_deref().unwrap_or(DEFAULT_FROM);
    let to = query.to.as_deref().unwrap_or(DEFAULT_TO);
    let group = query.group_by.as_deref().unwrap_or("day");
    let sql = bucket_sql(group)?;
    let start = exporter::date_bound(from)?;
    let end = exporter::date_bound(to)?;
    // One read-only snapshot keeps the two aggregates consistent during writes.
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let rows = sqlx::query(sql)
        .bind(start)
        .bind(end)
        .fetch_all(&mut *transaction)
        .await?;
    let mut buckets = Vec::with_capacity(rows.len());
    for row in rows {
        buckets.push(json!({
            "bucket": row.try_get::<Option<String>, _>("bucket")?.unwrap_or_else(|| "None".to_owned()),
            "orders": row.try_get::<i64, _>("orders")?,
            "revenue": row.try_get::<f64, _>("revenue")?,
        }));
    }
    let rows = sqlx::query(
        "SELECT p.name, SUM(i.price * i.quantity) AS revenue \
         FROM orders o JOIN order_items i ON i.order_id=o.id \
         JOIN products p ON p.id=i.product_id \
         WHERE o.created_at >= $1 AND o.created_at < $2 AND o.status IN ('paid','shipped') \
         GROUP BY p.name ORDER BY revenue DESC, p.name ASC LIMIT 10",
    )
    .bind(start)
    .bind(end)
    .fetch_all(&mut *transaction)
    .await?;
    let mut top = Vec::with_capacity(rows.len());
    for row in rows {
        top.push(json!({"name": row.try_get::<Option<String>, _>("name")?, "revenue": row.try_get::<f64, _>("revenue")?}));
    }
    transaction.commit().await?;
    Ok(json_response(
        StatusCode::OK,
        json!({
            "from": from, "to": to, "group_by": group, "buckets": buckets, "top_products": top,
        }),
    ))
}

async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let data: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("invalid JSON"))?
    };
    if !data.is_object() && !data.is_null() {
        return Err(ApiError::bad_request("expected a JSON object"));
    }
    let text = |key: &str, fallback: &'static str| -> ApiResult<String> {
        match data.get(key) {
            None | Some(Value::Null) => Ok(fallback.to_owned()),
            Some(Value::String(value)) => Ok(value.clone()),
            _ => Err(ApiError::bad_request(format!("invalid {key}"))),
        }
    };
    let filename = text("filename", "sales.csv")?;
    let filename = if filename.is_empty() {
        "sales.csv"
    } else {
        &filename
    };
    let file = exporter::export_sales(
        &state.pool,
        &state.config.export_dir,
        filename,
        &text("from", DEFAULT_FROM)?,
        &text("to", DEFAULT_TO)?,
    )
    .await?;
    Ok(json_response(StatusCode::OK, json!({"file": file})))
}

#[derive(Default, Deserialize)]
struct DownloadQuery {
    file: Option<String>,
}

async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<DownloadQuery>, QueryRejection>,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::bad_request("invalid report query"))?;
    let filename = query.file.as_deref().unwrap_or("");
    if filename.is_empty() {
        return Err(ApiError::not_found());
    }
    let path = exporter::download_path(&state.config.export_dir, filename).await?;
    let bytes = tokio::fs::read(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found()
        } else {
            error.into()
        }
    })?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/gzip")
        .body(Body::from(bytes))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_documented_bucket_columns_are_allowed() {
        for group in ["day", "status", "user"] {
            assert!(bucket_sql(group).is_ok());
        }
        for group in ["id", "created_at", "status; DROP TABLE orders", ""] {
            assert!(bucket_sql(group).is_err());
        }
    }
}
