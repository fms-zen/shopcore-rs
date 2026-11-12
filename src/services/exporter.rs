use std::{
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{NaiveDate, NaiveDateTime};
use flate2::{write::GzEncoder, Compression};
use sqlx::{PgPool, Row};

use crate::error::{ApiError, ApiResult};

pub const DEFAULT_FROM: &str = "1970-01-01";
pub const DEFAULT_TO: &str = "2999-12-31";
static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);

/// Parse a report date or timestamp without a time zone.
pub fn date_bound(value: &str) -> ApiResult<NaiveDateTime> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).unwrap());
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(timestamp) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(timestamp);
        }
    }
    Err(ApiError::bad_request("invalid report date"))
}

/// A report name is a single file component, never a path supplied by a client.
pub fn validate_filename(name: &str) -> ApiResult<()> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(ApiError::bad_request("invalid report filename"));
    }
    Ok(())
}

pub fn csv_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("CSV export failed: {error}"),
    )
}

pub fn csv_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

pub fn gzip(bytes: &[u8]) -> ApiResult<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

pub async fn export_sales(
    pool: &PgPool,
    export_dir: &Path,
    filename: &str,
    from: &str,
    to: &str,
) -> ApiResult<String> {
    validate_filename(filename)?;
    let from = date_bound(from)?;
    let to = date_bound(to)?;
    let rows = sqlx::query(
        "SELECT o.order_no, o.created_at, o.status, u.username, p.name AS product, \
         i.quantity, i.price FROM orders o \
         JOIN users u ON u.id=o.user_id JOIN order_items i ON i.order_id=o.id \
         JOIN products p ON p.id=i.product_id \
         WHERE o.created_at >= $1 AND o.created_at < $2 ORDER BY o.id, i.id",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    let mut csv = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    csv.write_record([
        "order_no", "date", "status", "user", "product", "qty", "price",
    ])
    .map_err(csv_error)?;
    for row in rows {
        let date: NaiveDateTime = row.try_get("created_at")?;
        csv.write_record([
            row.try_get::<i32, _>("order_no")?.to_string(),
            date.to_string(),
            row.try_get::<String, _>("status")?,
            row.try_get::<String, _>("username")?,
            row.try_get::<Option<String>, _>("product")?
                .unwrap_or_else(|| "None".to_owned()),
            row.try_get::<i32, _>("quantity")?.to_string(),
            csv_number(row.try_get("price")?),
        ])
        .map_err(csv_error)?;
    }
    let compressed = gzip(&csv.into_inner().map_err(csv_error)?)?;
    let output_name = format!("{filename}.gz");
    tokio::fs::create_dir_all(export_dir).await?;
    let directory = tokio::fs::canonicalize(export_dir).await?;
    // Publish a complete file atomically. Replacing a symlink here replaces the
    // directory entry itself instead of following its target outside export_dir.
    let temporary = directory.join(format!(
        ".shopcore-export-{}-{}",
        std::process::id(),
        NEXT_EXPORT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await?;
    use tokio::io::AsyncWriteExt;
    let result: ApiResult<()> = async {
        file.write_all(&compressed).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&temporary, directory.join(&output_name)).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result?;
    Ok(output_name)
}

pub async fn download_path(export_dir: &Path, filename: &str) -> ApiResult<PathBuf> {
    validate_filename(filename)?;
    let directory = tokio::fs::canonicalize(export_dir)
        .await
        .map_err(|_| ApiError::not_found())?;
    let path = tokio::fs::canonicalize(directory.join(filename))
        .await
        .map_err(|_| ApiError::not_found())?;
    if !path.starts_with(&directory) || !tokio::fs::metadata(&path).await?.is_file() {
        return Err(ApiError::not_found());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn report_names_cannot_escape_directory() {
        for name in [
            "",
            ".",
            "..",
            "../sales.csv",
            "/tmp/sales.csv",
            "a/b",
            "a\\b",
            "a\0b",
        ] {
            assert!(validate_filename(name).is_err(), "accepted {name:?}");
        }
        for name in ["sales.csv", "sales report.csv", "quoted\"name.csv"] {
            assert!(validate_filename(name).is_ok());
        }
    }

    #[test]
    fn dates_preserve_exclusive_boundary_precision() {
        assert_eq!(
            date_bound("2026-01-01").unwrap().to_string(),
            "2026-01-01 00:00:00"
        );
        assert_eq!(
            date_bound("2026-01-01T12:30:00.123456")
                .unwrap()
                .to_string(),
            "2026-01-01 12:30:00.123456"
        );
        assert!(date_bound("2026-02-30").is_err());
        assert!(date_bound("2026-01-01' OR TRUE --").is_err());
    }

    #[test]
    fn gzip_is_a_real_gzip_stream() {
        let input = b"order_no,date,status,user,product,qty,price\n";
        let compressed = gzip(input).unwrap();
        assert_eq!(&compressed[..2], &[0x1f, 0x8b]);
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(compressed.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, input);
    }
}
