# Compatibility contract

ShopCore preserves the original service's HTTP, command, and data contracts, with the safety changes below. [docs/openapi.yaml](docs/openapi.yaml) defines the HTTP operations, response keys, status codes, defaults, and pagination. [API and command coverage](docs/submission.md) maps these behaviors to tests.

## Data and packaging

The schema retains the six original tables, `users`, `products`, `coupons`, `orders`, `order_items`, and `payments`, with their existing PostgreSQL column types. `SERIAL` IDs decode as `i32`, aggregate counts as `i64`, and `TIMESTAMP` values as `NaiveDateTime`. Dates remain JSON strings. Money remains JSON numeric with PostgreSQL `DOUBLE PRECISION` storage, and order totals round to two decimals with Python-compatible rounding behavior. SQLx queries resolve at runtime without a database connection during compilation.

The bank simulator, seed CSV, OpenAPI document, and supplied smoke tests remain unchanged. Startup seeds the original admin credentials, sample products, `SAVE10`, and `ONCE50`. The repository builds and runs without a sibling `legacy/` directory. Operational command logic is Rust, and `manage.py` forwards arguments to the Rust executable.

## HTTP and command behavior

The API contains 27 HTTP operations across health, accounts, products, orders, wallet payments, bank settlement, administration, and reports. Product reads are public. Order listing and payment are owner-scoped. Responses preserve the documented keys, status codes, defaults, and pagination. OpenAPI permits extra response fields, but responses exclude password hashes. Errors are JSON objects with an `error` string, including malformed request bodies and unknown resources.

Order creation preserves coupon expiry and use rules, stock reservation, and wallet credit behavior. An omitted item quantity defaults to one. Notifications send pending and paid payloads to an order's `notify_url`. Background email jobs append to the in-memory outbox.

Reports read current committed data and bucket all order statuses in the half-open interval `[from,to)`. Date bounds accept dates or timestamps without time zones and default to `1970-01-01` and `2999-12-31`. Top products group by product name, include only paid and shipped orders, and use original line prices without allocating coupon discounts.

The export formats retain separate contracts:

- HTTP export is gzip CSV with header `order_no,date,status,user,product,qty,price` and one row per order item. It returns `<filename>.gz`, and downloads use `application/gzip`.
- CLI export is plain CSV with header `order_no,status,total,created_at` and one row per order, including all statuses, ordered by database ID.

The CLI supports `init-db`, `seed`, `import-products PATH`, `export-sales PATH`, and `purge-sessions`. Commands retain the messages `tables ready`, `seeded`, `imported N products`, `wrote PATH (N rows)`, and `purged sessions`. CSV import accepts the `sku,name,price,stock` header and quoted fields. It keeps the first occurrence of each SKU within the input and permits SKUs already in the database. Import validates every record before committing all rows together.

## Authentication and input validation

Admin routes return 401 for a missing or invalid bearer token and 403 for an authenticated non-admin. Logout invalidates its token. Profile updates accept only name and email, and duplicate email changes return 409. Tokens do not reproduce the legacy predictable MD5 values. Existing password hashes remain readable. Role changes accept `user` or `admin`.

SQL values are bound. Sort and `group_by` columns use allowlists, so untrusted input never becomes a SQL identifier. Invalid sort or group values, product pagination below one, malformed or nonfinite monetary values, negative money or CSV stock, and malformed or nonpositive item quantities return JSON errors. Topup, transfer, and refund amounts are strictly positive.

CSV and gzip libraries handle exports. HTTP report filenames stay within the export directory, and atomic rename publishes complete gzip files. Exports do not use shell commands.

## Transactions and money

Each stock, coupon, or money mutation uses one transaction. Payment, cancellation, refund, and settlement lock the order before changing state. Transfers lock users in ascending ID order, and product locks follow product ID order. Atomic increments and decrements prevent stale balances. Failed order creation rolls back inserted rows, stock changes, and coupon use together. Single-use coupon consumption and order number allocation remain safe under concurrent requests without a schema change.

Cumulative refunds cannot exceed the order total. Sequential partial refunds remain supported within that cap. Cancelled orders cannot be refunded. Cancellation and refund cannot create duplicate wallet credit. Cancelled is terminal for admin status changes, preventing repeated stock restoration or credit. Other legacy status strings remain accepted. Cancelling a paid or shipped order credits the total minus prior refunds. The existing `refunded` response-field semantics remain unchanged.

Bank requests have finite timeouts, and all retries for an order reuse a stable idempotency key. Bank I/O does not hold a wallet-user lock. The payment transaction holds the order lock during bounded bank calls, and callback processing waits for that lock without blocking the bank response.

Webhooks verify HMAC-SHA256 over the exact raw request bytes and reject missing or invalid signatures. A correctly signed settled event with an arbitrary `bank_ref` moves an existing pending order to paid even without a matching payment row. It independently marks any matching payment settled. The callback does not require a payment match, insert a synthetic payment, or change `paid_amount`.

External notifications enter the queue only after a successful transaction commit. Notification delivery success is not required to commit a purchase.

## Known limits

- Any authenticated user can access another user's order detail, invoice, cancellation, and refund. Order listing and payment remain owner-scoped.
- A slow bank can accept a charge after the API times out. If the order remains pending and the client switches to wallet payment before settlement, both methods can collect money. Stable bank retry keys do not prevent this cross-method case.
- A partial refund changes status to `refunded`. Cancellation while that status remains restores stock without crediting the remaining total. If an admin first changes status to `paid` or `shipped`, cancellation credits only the remainder.
- Existing password hashes use MD5, and money uses `f64` with PostgreSQL `DOUBLE PRECISION` to preserve the schema and existing credentials.
- Sessions, background jobs, and the email outbox belong to one server process and disappear on restart. A standalone `purge-sessions` command does not revoke the running server's sessions. Email jobs do not send through a provider.

## Verification

`./scripts/test.sh` runs Rust tests, builds and starts the stack, runs the HTTP suite, and checks the CLI commands. The supplied smoke tests remain unchanged. Additional tests cover authorization, validation, concurrency, rollback, idempotency, reports, and export formats.

`python3 scripts/compare-api.py --legacy http://localhost:8001 --rust http://localhost:8000` compares isolated, equivalently seeded legacy and Rust stacks. It mutates both stacks and requires a legacy executable outside this repository. The comparison covers its executed scenarios and reports documented safety differences separately. It does not establish equivalence for every input or concurrent execution.
