# ShopCore Rust

ShopCore is a Rust replacement for a Python storefront backend, built with Axum, Tokio, SQLx, and PostgreSQL 16. It provides accounts, a product catalog, orders, coupons, wallet transfers, bank payments, administration, and sales reports.

The migration preserves the original HTTP and command contracts, six-table schema, seed data, bank simulator, and supplied smoke tests, with [documented safety changes](#intentional-safety-changes). The repository builds and runs without a separate legacy checkout.

## Start the service

Install Docker with Docker Compose, then run:

```sh
docker compose up -d --build --wait app
curl http://localhost:8000/health
```

The health response is `{"status":"ok"}`. The API listens on port 8000, and the included bank simulator listens on port 5001. Compose starts PostgreSQL and the simulator with the app, and persists database data and HTTP exports in named volumes.

Startup creates missing tables and seeds ten products, coupons `SAVE10` and `ONCE50`, and an admin account. The development defaults are username `admin` and password `admin123`. Set custom credentials in the app environment before the initial seed. See [configuration](#configuration).

For a local Rust 1.88+ toolchain, use a separately reachable PostgreSQL server and bank simulator:

```sh
cargo build --locked --release
DATABASE_URL=postgresql://shop:shop@localhost:5432/shop cargo run --locked -- serve
```

The initial image build and Cargo dependency download require network access. Built images contain their runtime dependencies. Cached Cargo dependencies support `cargo build --locked --offline`. Bank payments require access to `BANK_URL`, and optional order notifications require access to their `notify_url`.

## Run the tests

```sh
./scripts/test.sh
```

This command requires Docker Compose. It runs Rust tests, builds and starts the stack, runs the HTTP acceptance suite, and checks all five maintenance commands. The supplied [smoke tests](tests/test_smoke.py) remain unchanged. Additional tests cover validation, authorization, concurrent transactions, rollback, reports, and CSV exports.

[scripts/cargo.sh](scripts/cargo.sh) provides the containerized Rust toolchain. [API and command coverage](docs/submission.md) maps behavior to tests and explains the optional comparison against a separately supplied Python service. Normal tests do not require that service. Run checks without concurrent writers because CLI checks compare database snapshots and export row counts.

## API and contracts

- [OpenAPI specification](docs/openapi.yaml) defines operations, request and response fields, status codes, defaults, and pagination.
- [Compatibility contract](CONTRACT.md) describes schema preservation, authentication, order and payment behavior, report formats, and transaction guarantees.
- [API and command coverage](docs/submission.md) links operations and compatibility requirements to source and repeatable checks.

Product reads are public. Protected routes use `Authorization: Bearer <token>` from `POST /api/login`. Admin routes require an admin account. Errors return JSON with an `error` string. See [known limits](#known-limits) for the remaining order authorization gaps.

## Configuration

The app reads these environment variables. Defaults apply when a variable is unset.

| Variable | Default | Purpose |
| --- | --- | --- |
| `DATABASE_URL` | `postgresql://shop:shop@localhost:5432/shop` | PostgreSQL connection |
| `BANK_URL` | `http://localhost:5001` | Bank charge endpoint base URL |
| `BANK_SECRET` | `bank-shared-secret-2019` | Shared HMAC secret for bank callbacks |
| `SECRET_KEY` | `shopcore-dev-secret` | Compatibility setting, currently unused |
| `ADMIN_USER` | `admin` | Admin username for seed creation |
| `ADMIN_PASSWORD` | `admin123` | Admin password for seed creation |
| `EXPORT_DIR` | `/tmp/exports` | HTTP gzip export directory |
| `BIND_ADDR` | `0.0.0.0:8000` | API listen address |
| `RUST_LOG` | `shopcore=info` | Tracing filter |

[compose.yaml](compose.yaml) sets `DATABASE_URL` to `postgresql://shop:shop@db:5432/shop` and `BANK_URL` to `http://bank:5001`. Configure container variables in the service's `environment` block or a Compose override. Exporting a host variable alone does not override a literal Compose value. Changing `ADMIN_PASSWORD` does not update an existing seeded account.

The bank simulator also reads these variables, plus `BANK_SECRET`. Keep the secret equal in the app and simulator.

| Variable | Default | Compose value |
| --- | --- | --- |
| `APP_URL` | `http://localhost:8000` | `http://app:8000` |
| `BANK_LATENCY_MS` | `0` | `0` |
| `BANK_DROP_RATE` | `0` | `0` |

`APP_URL` selects the settlement callback destination. The simulator uses the latency and drop rate settings to delay charges and simulate failed responses.

## Maintenance commands

Run commands in the app container. Import and export paths refer to files inside that container.

```sh
docker compose exec app shopcore init-db
docker compose exec app shopcore seed
docker compose exec app shopcore import-products /srv/shopcore/seed/products.csv
docker compose exec app shopcore export-sales /tmp/orders.csv
docker compose exec app shopcore purge-sessions
```

[manage.py](manage.py) forwards `python manage.py <command> [args]` to Rust. It uses `SHOPCORE_BIN` when set, then searches for an installed or local binary, and finally falls back to Cargo. Local commands can also use `cargo run --locked -- <command> [args]` or `target/release/shopcore <command> [args]`.

CSV import accepts `sku,name,price,stock`, supports quoted fields, validates all records before writing, and commits them together. It keeps the first occurrence of each SKU within the input and permits SKUs already in the database. CLI export writes plain CSV with one row per order. HTTP export writes gzip CSV with one row per order item. See the [format contracts](CONTRACT.md#http-and-command-behavior).

`purge-sessions` clears only the command process's memory. It does not revoke sessions in a running server.

## Architecture

```mermaid
flowchart LR
    Client["HTTP client"] --> API
    subgraph Server["Rust server process"]
        API["Axum API :8000"] --> Sessions["Sessions: memory"]
        API --> Jobs["Job queue and worker: memory"]
        Jobs --> Outbox["Email outbox: memory"]
    end
    API <--> DB[("PostgreSQL: six existing tables")]
    API -->|charge| Bank["Bank simulator :5001"]
    Bank -->|signed settlement webhook| API
    Jobs -. optional POST .-> Notify["Order notify_url"]
    API -->|write gzip CSV| Exports["Export volume: /tmp/exports"]
    Exports -->|download bytes| API
    Manage["manage.py"] --> CLI["Rust CLI process"]
    CLI <--> DB
    CLI --> PlainCSV["Plain order CSV at supplied path"]
```

Routes in `src/routes/` handle HTTP requests. Services in `src/services/` implement order creation, inventory, pricing, bank calls, exports, and notifications. `src/cli.rs` implements maintenance commands. PostgreSQL transactions and locks protect stock, coupons, order state, and wallet balances. Notifications enter the queue only after commit.

Bank payment holds the order lock during bounded bank calls and marks an order paid before settlement. The later webhook verifies HMAC-SHA256 over the exact received bytes. The [transaction contract](CONTRACT.md#transactions-and-money) describes retry behavior, lock ordering, and settlement compatibility.

## Intentional safety changes

Admin routes require authentication and an admin role. Logout revokes its token, profile updates accept only name and email, and responses exclude password hashes. Input validation rejects invalid money, nonpositive quantities, negative CSV stock, and invalid report identifiers. SQL values use bound parameters, and dynamic sort and grouping columns use allowlists.

Transactions prevent partial order creation and CSV imports. Refunds cannot exceed the order total, and cancelled orders cannot be reopened or refunded. HTTP exports confine filenames to the export directory and publish complete gzip files atomically. See [authentication and input validation](CONTRACT.md#authentication-and-input-validation) and [transactions and money](CONTRACT.md#transactions-and-money) for the full rules and legacy exceptions.

## Known limits

- Any authenticated user can access another user's order detail, invoice, cancellation, and refund. Only order listing and payment enforce ownership.
- A bank may accept a charge after the API times out. Switching a still-pending order to wallet payment before settlement can collect both payments. Stable bank retry keys do not prevent this case.
- A partial refund changes status to `refunded`. Cancelling in that status restores stock without crediting the remainder. An admin change to `paid` or `shipped` first makes cancellation credit the remainder.
- Password hashes remain MD5, and money uses Rust `f64` with PostgreSQL `DOUBLE PRECISION` to preserve existing credentials and schema.
- Sessions, queued jobs, and the email outbox belong to one process and disappear on restart. Email jobs append to the outbox without contacting an email provider.

A valid signed settlement can mark a pending order paid without a matching payment row, preserving compatibility. It does not insert a payment or change `paid_amount`. These limits also appear in the [compatibility contract](CONTRACT.md#known-limits) and [design decisions](docs/submission.md#design-decisions-and-limits).
