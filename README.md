# ShopCore Rust

Standalone Rust/Axum migration of the ShopCore storefront backend. PostgreSQL keeps the original six-table schema, `DOUBLE PRECISION` amounts, and timestamp columns. The bank simulator, seed data, OpenAPI contract, and original smoke tests are included locally; this directory does not depend on `../legacy`.

## Run and build

```sh
docker compose up --build --wait
curl http://localhost:8000/health
```

The API listens on port 8000 and the included bank simulator on 5001. Startup creates missing tables and seeds admin `admin` / `admin123`, the ten original products, and `SAVE10` / `ONCE50`. Database and report files persist in Compose volumes.

For a local Rust 1.88+ toolchain and an available PostgreSQL server:

```sh
cargo build --locked --release
DATABASE_URL=postgresql://shop:shop@localhost:5432/shop cargo run --locked -- serve
```

The runtime needs no network access for dependency installation or compilation. Normal bank payments contact `BANK_URL`; notification jobs contact the order's `notify_url`. Building images or fetching Cargo dependencies requires network access on the first build. After caching dependencies, `cargo build --locked --offline` works without contacting the registry.

## Configuration

Environment variables and defaults:

| Variable | Default |
| --- | --- |
| `DATABASE_URL` | `postgresql://shop:shop@localhost:5432/shop` |
| `BANK_URL` | `http://localhost:5001` |
| `BANK_SECRET` | `bank-shared-secret-2019` |
| `SECRET_KEY` | `shopcore-dev-secret` |
| `ADMIN_USER` / `ADMIN_PASSWORD` | `admin` / `admin123` |
| `EXPORT_DIR` | `/tmp/exports` |
| `BIND_ADDR` | `0.0.0.0:8000` |
| `RUST_LOG` | `shopcore=info` |

Compose sets database/bank URLs to their service names. Change admin credentials before the initial seed when using custom credentials.

## Maintenance compatibility

```sh
docker compose exec app shopcore init-db
docker compose exec app shopcore seed
docker compose exec app shopcore import-products /path/in/container/products.csv
docker compose exec app shopcore export-sales /tmp/orders.csv
docker compose exec app shopcore purge-sessions
```

`python manage.py <command> [args]` remains a compatibility launcher for these Rust commands. Local equivalents are `cargo run --locked -- <command> [args]` or `target/release/shopcore <command> [args]`.

Commands retain legacy success messages (`tables ready`, `seeded`, `imported N products`, `wrote PATH (N rows)`, `purged sessions`). CSV import accepts the `sku,name,price,stock` header and quoted fields, keeps the first occurrence of each SKU within the input, and still permits SKUs already in the database. Import validates all records and commits them together. Sessions remain in server process memory and disappear on restart. As with legacy `manage.py`, running `purge-sessions` in another process clears that command's own memory; it does not revoke a running server's sessions.

CLI export writes plain CSV with `order_no,status,total,created_at`, one row per order, all statuses, ordered by database id. The HTTP admin export writes gzip CSV with `order_no,date,status,user,product,qty,price`, one row per order item, and returns `<filename>.gz`. CSV quoting handles commas, quotes, and newlines. HTTP downloads serve `application/gzip`. Reports use inclusive `from` and exclusive `to` bounds, accepting dates or timestamps without time zones; defaults are `1970-01-01` and `2999-12-31`. Buckets include all order statuses. Top products group by product name, include only paid/shipped orders, and use original line prices without allocating coupons.

## Verification

```sh
./scripts/cargo.sh check --locked --all-targets
./scripts/cargo.sh test --locked
docker compose --profile test run --build --rm test
```

`./scripts/cargo.sh` provides a reusable Docker Rust toolchain if Rust is not installed locally. `./scripts/test.sh` runs Rust tests, starts the stack, and runs the HTTP suite. The 15 original smoke tests remain unchanged in `tests/test_smoke.py`; additional tests cover safety, concurrency, reports, and CSV/gzip behavior. With a local pytest environment, use `BASE_URL=http://localhost:8000 pytest -q tests` against a running stack.

## Intentional safety changes

Admin endpoints require a valid admin token (missing/invalid token 401, non-admin 403); logout revokes its token. Profile updates accept only name/email, duplicate email changes return 409, and API responses exclude password hashes. Existing legacy MD5 password hashes remain readable. Role changes accept `user` or `admin`. Cancelled orders cannot be reopened through admin status changes; other legacy status strings remain accepted.

SQL values are bound and sort/group columns are allowlisted. Invalid sort/group values, product pagination below one, malformed/nonfinite/negative monetary values, negative CSV stock, and malformed/nonpositive quantities return JSON errors. An omitted item quantity still defaults to one. Refunds reject cancelled orders and amounts beyond the remaining total; sequential partial refunds can reach the total. Cancelling paid/shipped orders credits the total minus prior refunds. Cancelling an order already in refunded status retains the legacy stock-only behavior. Stock, coupons, transfers, and payments use transactions and locks. Bank calls have timeouts and stable idempotency keys; callbacks verify HMAC over the original bytes and preserve settlement without a matching payment row.

Reports always read current committed data. Export uses CSV/gzip libraries and confines client filenames to the export directory; complete gzip files are published atomically. Input validation and transaction rollback prevent partial CSV imports. The application does not use shell commands to generate exports.
