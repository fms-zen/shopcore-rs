# ShopCore Rust migration contract

Project root is `shopcore-rs/`. The sibling `legacy/` is read-only reference material. The completed project must contain unchanged copies of the bank simulator, seed CSV, OpenAPI document and supplied tests; running it must not require its sibling directory.

## Ownership and coordination

- Foundation/integration owns Cargo files, shared modules, module registries, startup, Docker/Compose and copied reference assets.
- Accounts/catalog/admin owns its route implementations.
- Orders owns order routes and order/pricing/inventory helpers.
- Money owns wallet/payment/webhook routes and bank client.
- Reports/maintenance owns reports, exporter, CLI commands and runbook.
- Astra reviewer owns this contract and review notes. Shared-interface changes require a message to foundation and reviewer before adoption. Executors do not edit another executor's files.

## Shared interface (freeze before feature integration)

Foundation confirmed the following base contract; it publishes the final compiling definitions and any clarification to all executors.

```rust
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub sessions: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, i32>>>,
    pub http: reqwest::Client,
    pub config: std::sync::Arc<Config>,
    pub jobs: tokio::sync::mpsc::UnboundedSender<Job>,
    pub outbox: std::sync::Arc<tokio::sync::RwLock<Vec<serde_json::Value>>>,
}
pub type ApiResult<T> = Result<T, ApiError>;
// ApiError carries an HTTP status and public message; IntoResponse emits {"error": message}.
pub fn json_response(status: axum::http::StatusCode, value: serde_json::Value)
    -> axum::response::Response;
pub async fn require_user(state: &AppState, headers: &axum::http::HeaderMap)
    -> ApiResult<AuthUser>;
pub async fn require_admin(state: &AppState, headers: &axum::http::HeaderMap)
    -> ApiResult<AuthUser>;
// Every HTTP feature module exposes:
pub fn routes() -> axum::Router<AppState>;
```

Frozen AuthUser fields: `id: i32`, `username: String`, `email: Option<String>`, `name: Option<String>`, `role: String`, `wallet_balance: f64`, with `is_admin(&self)->bool`. Config has public String fields `database_url`, `bank_url`, `bank_secret`, `secret_key`, `admin_user`, `admin_password`, `bind_addr`, and `export_dir: PathBuf`. Job has public `kind: String`, `payload: Value`; `jobs::enqueue(&AppState, &str, Value)->()` is synchronous. `ApiError::new(StatusCode, impl Into<String>)` plus `bad_request`, `unauthorized`, `forbidden`, `not_found`, `conflict` constructors and SQLx/I/O conversions are provided. Database entry points are `db::init(&PgPool)->ApiResult<()>` and `db::seed(&PgPool,&Config)->ApiResult<()>`.

No executor invents a second AppState, authentication system or error type. Use runtime SQLx queries (no database-dependent query macros). Route response signatures may use `ApiResult<Response>` and the common JSON helper.

Use existing Postgres column types and all six tables: users, products, coupons, orders, order_items, payments. Database SERIAL ids decode as i32; aggregate counts decode as i64. PostgreSQL TIMESTAMP requires NaiveDateTime, not timezone-bearing DateTime. Date fields remain JSON strings. Money remains JSON numeric and existing DOUBLE PRECISION schema is retained; order totals round to two decimals with attention to Python rounding behavior.

## Required compatibility checklist

- [x] Health: GET /health.
- [x] Accounts: POST /api/register, /api/login, /api/logout; GET and PATCH /api/users/me.
- [x] Products: GET and POST /api/products; GET and PUT /api/products/{id}.
- [x] Orders: GET and POST /api/orders; GET /api/orders/{id}; GET invoice; POST cancel and refund.
- [x] Money: GET /api/wallet; POST topup and transfer; POST /api/orders/{id}/pay; POST /api/webhooks/bank.
- [x] Administration: GET /api/admin/users; POST user role and order status routes.
- [x] Reports: GET /api/reports/sales; POST /api/reports/export; GET /api/reports/download.
- [x] CLI: init-db, seed, import-products PATH, export-sales PATH, purge-sessions.
- [x] All 15 supplied smoke tests pass unchanged against Rust.
- [x] Seed admin credentials, sample products and SAVE10/ONCE50 coupons match legacy assets.
- [x] Notifications send pending/paid payloads to notify_url; background email jobs remain supported.
- [x] Root Compose builds/runs independently of legacy and documented commands succeed.
- [x] Supplied tests run with one documented command; all operational command logic is Rust, with only a forwarding compatibility shim if needed.

The API contains 27 HTTP operations. Preserve required response keys, documented status codes, default values, pagination, public product access, owner-scoped order listing and payment, coupon expiry/use behavior, stock reservation, wallet credits and report semantics. Extra fields are permitted by OpenAPI but password hashes need not be exposed. Error responses are JSON objects with an error string, including extractor/fallback failures where practical.

Reports bucket all orders in [from,to); top products include only paid/shipped orders. HTTP CSV export has header `order_no,date,status,user,product,qty,price`, one row per item, is gzip encoded, and returns `<filename>.gz`. CLI export is plain CSV with header `order_no,status,total,created_at` and one row per order. Preserve both formats.

**Mandatory settlement compatibility:** a correctly HMAC-SHA256 signed settled webhook with an arbitrary bank_ref moves an existing pending order to paid even when no matching payment row exists. It independently marks any matching payment settled. The supplied test explicitly exercises this. Do not require a payment match, insert a synthetic payment, or change paid_amount merely to satisfy the callback.

## Intentional safety fixes and transaction rules

- Missing/invalid bearer token is 401; authenticated non-admin is 403 on admin routes. This follows OpenAPI and fixes the legacy anonymous-admin bypass.
- Logout invalidates its token. Only name/email may be changed through profile update. Tokens need not reproduce legacy predictable MD5 values; existing password hashes must remain readable.
- Bind SQL values. Whitelist documented sort/group_by values; never interpolate untrusted SQL identifiers. Use CSV and gzip libraries and constrain report filenames to the export directory.
- Reject malformed/non-finite/negative monetary inputs and invalid item quantities rather than creating credits or stock through arithmetic abuse. Preserve documented valid requests and explicitly record any additional behavior change.
- Parent-approved correction: topup, transfer and refund amounts must be strictly positive. Cumulative refunds cannot exceed the order total; sequential partial refunds remain supported within that cap. Cancelled orders cannot subsequently be refunded. Cancellation/refund interaction must not create duplicate wallet credit.
- Parent-approved admin correction: cancelled is terminal for admin status changes, preventing reset-and-cancel stock/credit replay. When cancelling a paid/shipped order, wallet credit is the remaining total minus prior refunds. Keep the existing refunded response-field semantics; do not redesign all admin transitions.
- Use one transaction for each stock/coupon/money mutation. Order state is locked before pay/cancel/refund; user locks for transfers are acquired in ascending ID order; product locks are acquired consistently. Atomic increments/decrements avoid stale balances. Failed order creation rolls back all inserted rows, stock changes and coupon use.
- Single-use coupon consumption and order number allocation must be safe under concurrent requests without schema redesign.
- Bank requests have finite timeouts. All retries for an order reuse a stable idempotency key; do not hold wallet-user locks during bank I/O. A bounded order lock during bank calls is acceptable for this timebox if callback handling cannot deadlock the HTTP response.
- Verify HMAC over the exact raw webhook bytes; reject absent/invalid signatures. The settlement compatibility rule above still applies.
- Enqueue external notifications only after successful transaction commit. Do not require notification success to commit a purchase.

## Acceptance gates

Foundation runs shared builds serially with a reusable Cargo cache. First gate: image pulls, compiling skeleton, PostgreSQL connection, health. Integration gate: every operation registered and every command dispatched. Final gate: compile/tests, unchanged smoke suite, targeted concurrency/rollback/idempotency checks, gzip-content check, CLI checks and a clean standalone startup. Each executor reports exact commands/results and remaining blockers; reviewer checks the diff and evidence before declaring completion.

Review division: Astra reviewer owns code, transaction and security/compatibility review. Coordinator owns independent black-box/system acceptance and packaging checks. Do not duplicate Cargo builds or test runs already owned by foundation/coordinator.

Final acceptance passed: coordinator built Git archive `e81bd43` with a brand-new Docker-container BuildKit builder, started fresh database/export volumes, and passed all 47 HTTP tests on an Internet-isolated Compose network. Tested on linux/arm64 through OrbStack. Final source review is approved; see REVIEW.md for evidence and preserved limitations.
