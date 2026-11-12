use std::{collections::HashSet, path::Path};

use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;

use crate::{
    db,
    error::{ApiError, ApiResult},
    services::exporter::{csv_error, csv_number},
    AppState,
};

pub const USAGE: &str =
    "usage: shopcore <init-db|seed|import-products|export-sales|purge-sessions> [args]";

pub async fn run(args: &[String], state: &AppState) -> ApiResult<()> {
    match args.first().map(String::as_str) {
        Some("init-db") if args.len() == 1 => {
            db::init(&state.pool).await?;
            println!("tables ready");
        }
        Some("seed") if args.len() == 1 => {
            let had_products: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products)")
                .fetch_one(&state.pool)
                .await?;
            db::seed(&state.pool, &state.config).await?;
            if !had_products {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM products")
                    .fetch_one(&state.pool)
                    .await?;
                println!("imported {count} products");
            }
            println!("seeded");
        }
        Some("import-products") if args.len() == 2 => {
            let count = import_products(Path::new(&args[1]), state).await?;
            println!("imported {count} products");
        }
        Some("export-sales") if args.len() == 2 => {
            let count = export_sales(Path::new(&args[1]), state).await?;
            println!("wrote {} ({count} rows)", args[1]);
        }
        Some("purge-sessions") if args.len() == 1 => {
            // Sessions are process-local, as in legacy manage.py. A standalone
            // command cannot invalidate the running HTTP server's sessions.
            state.sessions.write().await.clear();
            println!("purged sessions");
        }
        _ => return Err(ApiError::bad_request(USAGE)),
    }
    Ok(())
}

#[derive(Deserialize)]
struct ProductRecord {
    sku: String,
    name: String,
    price: String,
    stock: String,
}

#[derive(Debug, PartialEq)]
struct ImportedProduct {
    sku: String,
    name: String,
    price: f64,
    stock: i32,
}

fn parse_products(bytes: &[u8]) -> ApiResult<Vec<ImportedProduct>> {
    let mut reader = csv::Reader::from_reader(bytes);
    let mut seen = HashSet::new();
    let mut products = Vec::new();
    for record in reader.deserialize::<ProductRecord>() {
        let record = record
            .map_err(|error| ApiError::bad_request(format!("invalid product CSV: {error}")))?;
        if !seen.insert(record.sku.clone()) {
            continue;
        }
        let price: f64 = record
            .price
            .parse()
            .map_err(|_| ApiError::bad_request("invalid product price"))?;
        let stock: i32 = record
            .stock
            .parse()
            .map_err(|_| ApiError::bad_request("invalid product stock"))?;
        if !price.is_finite() || price < 0.0 || stock < 0 {
            return Err(ApiError::bad_request(
                "product price and stock must be nonnegative",
            ));
        }
        products.push(ImportedProduct {
            sku: record.sku,
            name: record.name,
            price,
            stock,
        });
    }
    Ok(products)
}

async fn import_products(path: &Path, state: &AppState) -> ApiResult<usize> {
    let products = parse_products(&tokio::fs::read(path).await?)?;
    let mut transaction = state.pool.begin().await?;
    for product in &products {
        sqlx::query("INSERT INTO products (sku,name,price,stock) VALUES ($1,$2,$3,$4)")
            .bind(&product.sku)
            .bind(&product.name)
            .bind(product.price)
            .bind(product.stock)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(products.len())
}

async fn export_sales(path: &Path, state: &AppState) -> ApiResult<usize> {
    let rows = sqlx::query("SELECT order_no,status,total,created_at FROM orders ORDER BY id")
        .fetch_all(&state.pool)
        .await?;
    let mut csv = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    csv.write_record(["order_no", "status", "total", "created_at"])
        .map_err(csv_error)?;
    for row in &rows {
        csv.write_record([
            row.try_get::<i32, _>("order_no")?.to_string(),
            row.try_get::<String, _>("status")?,
            csv_number(row.try_get("total")?),
            row.try_get::<NaiveDateTime, _>("created_at")?.to_string(),
        ])
        .map_err(csv_error)?;
    }
    tokio::fs::write(path, csv.into_inner().map_err(csv_error)?).await?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_import_handles_quotes_and_keeps_first_duplicate_sku() {
        let products = parse_products(b"sku,name,price,stock\nONE,\"Name, with \"\"quotes\"\"\",39.0,2\nONE,ignored,invalid,invalid\nTWO,Second,7.25,100\n").unwrap();
        assert_eq!(
            products,
            vec![
                ImportedProduct {
                    sku: "ONE".into(),
                    name: "Name, with \"quotes\"".into(),
                    price: 39.0,
                    stock: 2
                },
                ImportedProduct {
                    sku: "TWO".into(),
                    name: "Second".into(),
                    price: 7.25,
                    stock: 100
                },
            ]
        );
    }

    #[test]
    fn csv_import_rejects_invalid_numbers_before_writing() {
        for row in ["NaN,1", "inf,1", "-1,1", "1,-1", "abc,1", "1,1.5"] {
            assert!(
                parse_products(format!("sku,name,price,stock\nSKU,Name,{row}\n").as_bytes())
                    .is_err()
            );
        }
    }

    #[test]
    fn legacy_seed_values_are_read_without_changes() {
        let products = parse_products(include_bytes!("../seed/products.csv")).unwrap();
        assert_eq!(products.len(), 10);
        assert_eq!(products[0].sku, "SKU-1001");
        assert_eq!(products[0].price, 89.99);
        assert_eq!(products[9].stock, 100);
    }
}
