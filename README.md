# ShopCore Rust

ShopCore's storefront backend uses Rust, Axum, and PostgreSQL. It keeps the original six-table schema, `DOUBLE PRECISION` amounts, and timestamp columns. The bank simulator, seed data, OpenAPI contract, and original smoke tests are included. Running the project does not require `../legacy`.

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

Built images contain the executables and dependencies needed at runtime. Bank payments contact `BANK_URL`, and notification jobs contact the order's `notify_url`. The first image build or Cargo dependency download requires network access. After caching dependencies, `cargo build --locked --offline` works without contacting the registry.

## Architecture and flows

### Running services

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

Sessions, queued jobs, and the email outbox belong to one server process and disappear on restart. Email jobs append to the outbox; they do not contact an email provider. Compose persists PostgreSQL data and gzip exports in volumes. Maintenance commands run in a separate Rust process.

### Order creation

```mermaid
flowchart TD
    Request["POST /api/orders"] --> Input{"Items and quantities valid?"}
    Input -->|no| Error["Return JSON error"]
    Input -->|yes| Begin["Begin PostgreSQL transaction"]
    Begin --> Locks["Lock coupon if supplied; lock products by id"]
    Locks --> Validate{"Coupon, prices, stock and total valid?"}
    Validate -->|no| Rollback["Rollback all transaction changes"]
    Validate -->|yes| Number["Acquire advisory lock; allocate order number"]
    Number --> Write["Insert pending order and lines; reserve stock; consume coupon"]
    Locks -. database error .-> Rollback
    Number -. database error .-> Rollback
    Write -. database error .-> Rollback
    Rollback --> Error
    Write --> Commit["Commit"]
    Commit --> Enqueue["Enqueue email and optional pending notification"]
    Enqueue --> Response["Return created order"]
```

Any validation or database failure inside the transaction takes the rollback path. Jobs are enqueued only after commit; notification delivery failure does not undo the order.

### Bank payment and settlement

```mermaid
sequenceDiagram
    participant Client
    participant API as Rust API
    participant DB as PostgreSQL
    participant Bank as Bank simulator
    Client->>API: POST /api/orders/{id}/pay (bank)
    API->>DB: Begin; lock owned pending order
    Note over API,DB: Order lock remains held during bank calls
    API->>Bank: POST /charge (order_id, amount, stable Idempotency-Key)
    Note over API,Bank: Up to 3 attempts, each with a 2 second timeout
    Bank-->>API: Accepted response with bank_ref
    API->>DB: Insert captured payment; set order paid and paid_amount
    API->>DB: Commit
    Note over API: Enqueue email and optional paid notification
    API-->>Client: 200 paid, bank_ref
    Bank->>API: Later POST /api/webhooks/bank (settled, X-Signature)
    API->>API: Verify HMAC-SHA256 over exact raw bytes
    alt Invalid signature
        API-->>Bank: 401
    else Valid signed settled event
        API->>DB: Begin; lock order
        API->>DB: Mark matching payment settled
        API->>DB: Set pending order paid; preserve paid_amount
        API->>DB: Commit
        API-->>Bank: 200 ok
    end
```

Payment marks the order paid without waiting for settlement. A callback arriving during payment waits for the order lock. If no bank attempt produces an accepted response, the API rolls back its database transaction and returns 502. The stable retry key does not guarantee exactly-once bank execution. For compatibility, a valid signed settlement with an unmatched `bank_ref` still marks an existing pending order paid, without inserting a payment or changing `paid_amount`.

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
./scripts/test.sh
```

`./scripts/test.sh` runs Rust tests, starts the stack, runs the HTTP suite, and checks the CLI commands. `./scripts/cargo.sh` provides a reusable Docker Rust toolchain if Rust is not installed locally. The original smoke tests remain unchanged in `tests/test_smoke.py`. Additional tests cover safety, concurrency, reports, and CSV/gzip behavior. With a local pytest environment, use `BASE_URL=http://localhost:8000 pytest -q tests` against a running stack.

The [API and command coverage](docs/submission.md) reference maps behavior to tests and describes known limits. To check only the CLI on an existing seeded Compose stack, run `./scripts/test-cli.sh` without concurrent writers. For an optional comparison, run `python3 scripts/compare-api.py --legacy http://localhost:8001 --rust http://localhost:8000` against isolated, equivalently seeded stacks. This comparison mutates both stacks and needs a legacy executable outside this repository. Normal tests never depend on `legacy/`. Its result covers only the scenarios the comparison executes, not every input or concurrent execution.

## Intentional safety changes

Admin endpoints require a valid admin token (missing/invalid token 401, non-admin 403); logout revokes its token. Profile updates accept only name/email, duplicate email changes return 409, and API responses exclude password hashes. Existing legacy MD5 password hashes remain readable. Role changes accept `user` or `admin`. Cancelled orders cannot be reopened through admin status changes; other legacy status strings remain accepted.

SQL values are bound and sort/group columns are allowlisted. Invalid sort/group values, product pagination below one, malformed/nonfinite/negative monetary values, negative CSV stock, and malformed/nonpositive quantities return JSON errors. An omitted item quantity still defaults to one. Refunds reject cancelled orders and amounts beyond the remaining total; sequential partial refunds can reach the total. Cancelling paid/shipped orders credits the total minus prior refunds. Cancelling an order already in refunded status retains the legacy stock-only behavior. Stock, coupons, transfers, and payments use transactions and locks. Bank calls have timeouts and stable idempotency keys; callbacks verify HMAC over the original bytes and preserve settlement without a matching payment row.

Reports always read current committed data. Export uses CSV/gzip libraries and confines client filenames to the export directory; complete gzip files are published atomically. Input validation and transaction rollback prevent partial CSV imports. The application does not use shell commands to generate exports.
