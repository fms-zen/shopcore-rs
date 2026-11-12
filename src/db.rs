use crate::{
    auth::hash_password,
    config::Config,
    error::{ApiError, ApiResult},
};
use chrono::NaiveDateTime;
use serde_json::{Map, Value};
use sqlx::{postgres::PgRow, Column, PgPool, Row, TypeInfo, ValueRef};

pub async fn init(pool: &PgPool) -> ApiResult<()> {
    sqlx::raw_sql(include_str!("../schema.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn seed(pool: &PgPool, config: &Config) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    // Serialize seed invocations without changing the legacy table schema.
    sqlx::query("SELECT pg_advisory_xact_lock(794612031)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO users (username,email,name,password_hash,role,wallet_balance) SELECT $1,'admin@shopcore.local','Administrator',$2,'admin',0 WHERE NOT EXISTS (SELECT 1 FROM users WHERE username=$1)")
        .bind(&config.admin_user).bind(hash_password(&config.admin_password)).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO coupons (code,percent,expires,single_use) SELECT v.* FROM (VALUES ('SAVE10',10,'2099-12-31',FALSE),('ONCE50',50,'2099-12-31',TRUE)) AS v(code,percent,expires,single_use) WHERE NOT EXISTS (SELECT 1 FROM coupons)")
        .execute(&mut *tx).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products)")
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        let mut seen = std::collections::HashSet::new();
        for record in
            csv::Reader::from_reader(include_bytes!("../seed/products.csv").as_slice()).records()
        {
            let r = record.map_err(|_| ApiError::bad_request("invalid seed CSV"))?;
            let sku = r.get(0).unwrap_or_default();
            if !seen.insert(sku.to_owned()) {
                continue;
            }
            let price: f64 = r
                .get(2)
                .unwrap_or_default()
                .parse()
                .map_err(|_| ApiError::bad_request("invalid seed price"))?;
            let stock: i32 = r
                .get(3)
                .unwrap_or_default()
                .parse()
                .map_err(|_| ApiError::bad_request("invalid seed stock"))?;
            sqlx::query("INSERT INTO products (sku,name,price,stock) VALUES ($1,$2,$3,$4)")
                .bind(sku)
                .bind(r.get(1).unwrap_or_default())
                .bind(price)
                .bind(stock)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub fn format_timestamp(value: NaiveDateTime) -> String {
    value.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Serialize the existing schema's row types without leaking Rust/SQLx details.
pub fn row_to_json(row: &PgRow) -> Value {
    let mut result = Map::new();
    for column in row.columns() {
        let i = column.ordinal();
        let null = row.try_get_raw(i).map(|v| v.is_null()).unwrap_or(true);
        let value = if null {
            Value::Null
        } else {
            match column.type_info().name() {
                "INT2" => Value::from(row.get::<i16, _>(i)),
                "INT4" => Value::from(row.get::<i32, _>(i)),
                "INT8" => Value::from(row.get::<i64, _>(i)),
                "FLOAT4" => Value::from(row.get::<f32, _>(i)),
                "FLOAT8" => Value::from(row.get::<f64, _>(i)),
                "BOOL" => Value::from(row.get::<bool, _>(i)),
                "TIMESTAMP" => Value::from(format_timestamp(row.get::<NaiveDateTime, _>(i))),
                "TIMESTAMPTZ" => Value::from(format_timestamp(
                    row.get::<chrono::DateTime<chrono::Utc>, _>(i).naive_utc(),
                )),
                "DATE" => Value::from(row.get::<chrono::NaiveDate, _>(i).to_string()),
                "JSON" | "JSONB" => row.get::<Value, _>(i),
                _ => Value::from(row.try_get::<String, _>(i).unwrap_or_default()),
            }
        };
        result.insert(column.name().to_owned(), value);
    }
    Value::Object(result)
}
