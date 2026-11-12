# Submission evidence

This checklist maps the migration requirements to source and repeatable checks. It records coverage, not a new test-run result. The HTTP contract is [openapi.yaml](openapi.yaml), and approved safety changes are in [CONTRACT.md](../CONTRACT.md).

## HTTP operations

Each operation below has coverage in the supplied, unchanged [test_smoke.py](../tests/test_smoke.py). Function names identify the permanent test that exercises the operation. These smoke tests establish the original acceptance baseline. The migration's own regression tests are listed separately below.

| Method and path | Supplied test function |
| --- | --- |
| `GET /health` | `test_health` |
| `POST /api/register` | `test_register_login_me_update`, `test_register_rejects_duplicates_and_missing` |
| `POST /api/login` | `test_register_login_me_update` |
| `POST /api/logout` | `test_register_login_me_update` |
| `GET /api/users/me` | `test_register_login_me_update` |
| `PATCH /api/users/me` | `test_register_login_me_update` |
| `GET /api/products` | `test_products_list_sort_search_get` |
| `POST /api/products` | `test_admin_product_create_update_and_forbidden_for_user` |
| `GET /api/products/{id}` | `test_products_list_sort_search_get` |
| `PUT /api/products/{id}` | `test_admin_product_create_update_and_forbidden_for_user` |
| `POST /api/orders` | `test_order_lifecycle_wallet_pay_and_invoice`, `test_coupon_discount_and_single_use`, `test_order_validation_errors` |
| `GET /api/orders` | `test_order_lifecycle_wallet_pay_and_invoice` |
| `GET /api/orders/{id}` | `test_order_lifecycle_wallet_pay_and_invoice`, `test_order_validation_errors` |
| `GET /api/orders/{id}/invoice` | `test_order_lifecycle_wallet_pay_and_invoice` |
| `POST /api/orders/{id}/pay` | `test_order_lifecycle_wallet_pay_and_invoice`, `test_order_bank_payment_and_settlement_webhook` |
| `POST /api/orders/{id}/cancel` | `test_cancel_restocks_and_refunds_wallet` |
| `POST /api/orders/{id}/refund` | `test_refund_paid_order` |
| `GET /api/wallet` | `test_wallet_topup_and_transfer` |
| `POST /api/wallet/topup` | `test_wallet_topup_and_transfer` |
| `POST /api/wallet/transfer` | `test_wallet_topup_and_transfer` |
| `GET /api/reports/sales` | `test_sales_report_and_export_download` |
| `POST /api/reports/export` | `test_sales_report_and_export_download` |
| `GET /api/reports/download` | `test_sales_report_and_export_download` |
| `GET /api/admin/users` | `test_admin_users_roles_and_order_status` |
| `POST /api/admin/users/{id}/role` | `test_admin_users_roles_and_order_status` |
| `POST /api/admin/orders/{id}/status` | `test_admin_users_roles_and_order_status` |
| `POST /api/webhooks/bank` | `test_bank_webhook_with_valid_signature` |

The migration adds the following evidence beyond the supplied smoke tests.

| Behavior | Migration test file and function |
| --- | --- |
| Admin authentication, profile field restrictions, logout | [test_auth_catalog.py](../tests/test_auth_catalog.py), `test_admin_authorization_profile_allowlist_and_logout` |
| Concurrent duplicate registration and email uniqueness | [test_auth_catalog.py](../tests/test_auth_catalog.py), `test_registration_duplicates_are_atomic_and_email_update_is_unique` |
| Product sorting, search, partial updates, unsafe identifiers | [test_auth_catalog.py](../tests/test_auth_catalog.py), `test_catalog_sort_search_partial_update_and_identifier_rejection` |
| Empty and malformed bodies, JSON errors | [test_auth_catalog.py](../tests/test_auth_catalog.py), `test_malformed_bodies_and_unknown_resources_have_json_errors` |
| Order rollback and duplicate item stock demand | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_invalid_later_item_rolls_back_order_and_stock`, `test_duplicate_items_cannot_overreserve_and_are_preserved` |
| Stock and single-use coupon contention | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_concurrent_stock_exhaustion_has_one_winner`, `test_single_use_coupon_is_consumed_once` |
| Omitted, null, and empty coupon representation without a discount | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_no_coupon_preserves_representation_without_discount` |
| Expired coupon leaves orders, stock, and usage unchanged | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_expired_coupon_does_not_change_order_stock_or_usage` |
| Cancellation credit and stock change happen once | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_concurrent_cancel_restocks_and_credits_once` |
| Refund cap and cancellation interaction | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_partial_refunds_stop_at_total_and_cancelled_orders_cannot_refund`, `test_concurrent_refunds_cannot_exceed_total`, `test_cancel_after_partial_refund_and_admin_status_change_credits_only_remainder` |
| Invalid quantities leave stock unchanged | [test_orders_atomic.py](../tests/test_orders_atomic.py), `test_invalid_quantity_does_not_change_stock` |
| Wallet concurrency and lock ordering | [test_money_safety.py](../tests/test_money_safety.py), `test_concurrent_wallet_topups_transfers_and_self_transfer_conserve_money`, `test_opposing_transfers_finish_without_deadlock_and_conserve_money` |
| Concurrent payments and shared wallet funds | [test_money_safety.py](../tests/test_money_safety.py), `test_simultaneous_order_payments_debit_once`, `test_payments_and_transfer_compete_for_the_same_wallet_funds` |
| Stable bank retry key | [test_money_safety.py](../tests/test_money_safety.py), `test_bank_retry_uses_one_logical_charge` |
| Exact-body HMAC, repeated callback, unmatched bank reference | [test_money_safety.py](../tests/test_money_safety.py), `test_raw_webhook_signature_and_duplicate_settlement` |
| Pending and paid notification delivery | [test_money_safety.py](../tests/test_money_safety.py), `test_pending_and_paid_order_notifications_reach_http_receiver` |
| Invalid wallet amounts | [test_money_safety.py](../tests/test_money_safety.py), `test_invalid_wallet_amounts_cannot_create_credit` |
| Current reports, all statuses, gross product revenue | [test_reports_exports.py](../tests/test_reports_exports.py), `test_reports_refresh_group_all_statuses_and_rank_gross_product_revenue` |
| CSV quoting, gzip bytes, half-open date range | [test_reports_exports.py](../tests/test_reports_exports.py), `test_http_export_is_escaped_gzip_and_date_range_is_half_open` |
| Export and download path restrictions | [test_reports_exports.py](../tests/test_reports_exports.py), `test_report_export_and_download_reject_paths` |

## Maintenance commands

All command logic is in [src/cli.rs](../src/cli.rs), with initialization and seed logic in [src/db.rs](../src/db.rs). [manage.py](../manage.py) only forwards arguments to Rust. [scripts/test-cli.sh](../scripts/test-cli.sh) exercises the shim against the existing seeded Compose stack.

| Command | Permanent CLI acceptance check |
| --- | --- |
| `init-db` | Repeated success message, six expected tables, existing data unchanged |
| `seed` | Repeated success message, existing data unchanged, original product values, coupons, and admin credentials |
| `import-products PATH` | Quoted CSV, first duplicate SKU within input, existing database SKU accepted, invalid later row writes nothing |
| `export-sales PATH` | Success message and row count, plain CSV header, one row per order including a cancelled order |
| `purge-sessions` | Success message, existing server session remains valid because command memory is separate |

Rust unit tests in `src/cli.rs` also cover `csv_import_handles_quotes_and_keeps_first_duplicate_sku`, `csv_import_rejects_invalid_numbers_before_writing`, and `legacy_seed_values_are_read_without_changes`.

## Packaging and execution

| Requirement | Evidence and repeatable check |
| --- | --- |
| Standalone repository | [Dockerfile](../Dockerfile) copies only repository files. [compose.yaml](../compose.yaml) defines app, PostgreSQL 16, bank simulator, and tests without a sibling checkout. |
| Original schema | [schema.sql](../schema.sql) defines `users`, `products`, `coupons`, `orders`, `order_items`, and `payments`. `src/db.rs` embeds that file. CLI acceptance checks the tables. |
| Original local assets | [bank_sim/app.py](../bank_sim/app.py), [seed/products.csv](../seed/products.csv), [openapi.yaml](openapi.yaml), and [test_smoke.py](../tests/test_smoke.py) are the copied reference assets. Byte comparison with the supplied originals is an external packaging check. |
| Original seed behavior | `src/db.rs` seeds the admin, product CSV, `SAVE10`, and `ONCE50`. CLI acceptance checks values and repeatability. |
| One test command | `./scripts/test.sh` runs Rust tests, builds and starts the stack, runs the HTTP suite, then runs CLI acceptance. The script needs Docker Compose. |
| Existing stack CLI check | `./scripts/test-cli.sh` requires running seeded `app` and `db` services. Run it without concurrent writers because it checks database snapshots and export row counts. |
| Offline runtime | Built images contain the executables and dependencies. No external Internet service is required for normal operation. PostgreSQL and the internal bank service still require network connectivity. An optional `notify_url` must be reachable when used. |
| Build dependencies | The first image build and Cargo dependency download require network access. Cached Cargo dependencies allow `cargo build --locked --offline`. Offline runtime does not imply an uncached offline build. |
| Comparative compatibility | [scripts/compare-api.py](../scripts/compare-api.py) compares HTTP observations from isolated, equivalently seeded legacy and Rust stacks. It mutates both. The legacy executable must be supplied outside the submitted repository. Normal tests never depend on `legacy/`. |

The comparison script supplements the supplied acceptance tests. Its result applies to the scenarios it executes, with documented safety differences reported separately. It does not establish equivalence for every input or concurrent execution.

## Step 4 defense

Transactions keep each stock, coupon, or money change consistent with its order state. A failed order creation rolls back inserted rows, stock reservation, and coupon use together. Product locks follow product ID order. Wallet transfers lock users in ascending ID order. Payment, cancellation, refund, and settlement lock the order first. This ordering avoids competing operations observing stale state or taking the same locks in opposite orders. See [src/services/orders.rs](../src/services/orders.rs), [src/routes/orders.rs](../src/routes/orders.rs), and [src/routes/wallet.rs](../src/routes/wallet.rs).

Bank requests use an order-derived idempotency key so retries can recover the same logical charge. [src/services/bank.rs](../src/services/bank.rs) bounds attempts and request timeouts. The payment transaction holds the order lock during bank I/O, while avoiding a wallet-user lock on the bank path. The callback takes the order lock before updating payments. This is a bounded implementation tradeoff, not a distributed transaction or an exactly-once guarantee.

[src/routes/webhooks.rs](../src/routes/webhooks.rs) verifies HMAC-SHA256 over the exact received bytes. Parsing and reserializing JSON before verification would change the signed message. A valid signed settlement with an arbitrary `bank_ref` must still pay an existing pending order. The supplied `test_bank_webhook_with_valid_signature` requires this compatibility behavior. The callback updates any matching payment independently, without inserting a payment or changing `paid_amount`.

The two export formats serve separate existing contracts. CLI export is plain CSV with `order_no,status,total,created_at`, one row per order. HTTP export is gzip CSV with `order_no,date,status,user,product,qty,price`, one row per item. CSV libraries preserve quoted fields. The HTTP exporter confines filenames and publishes complete gzip files through atomic rename. See [src/cli.rs](../src/cli.rs) and [src/services/exporter.rs](../src/services/exporter.rs).

The approved safety changes remain those documented in [README.md](../README.md#intentional-safety-changes) and [CONTRACT.md](../CONTRACT.md#intentional-safety-fixes-and-transaction-rules). They include admin authentication, constrained profile updates, input validation, refund limits, terminal cancellation, and transaction protection. They do not authorize a broader policy redesign.

The remaining choices below preserve current behavior. Changing any of them requires a user decision.

- Any authenticated user can access order detail, invoice, cancellation, and refund for another user's order. Order listing and payment remain owner-scoped.
- A slow bank can accept a charge after the API times out. If the order remains pending and the client switches to wallet payment before settlement, both payment methods can collect money. Stable bank retry keys do not prevent this cross-method case.
- Partial refund changes status to `refunded`. Cancelling while that status remains restores stock without crediting the remaining total. If an admin first changes status to `paid` or `shipped`, cancellation credits only the remainder.
- Existing password hashes use MD5, and money remains `f64` with PostgreSQL `DOUBLE PRECISION`. This preserves the schema and existing credentials.
- Sessions, background jobs, and the email outbox are process-local and disappear on restart. Standalone `purge-sessions` does not revoke sessions in the running server. Email jobs append to memory rather than sending through a provider.

These are declared limits of the submission. The checklist does not claim a complete authorization redesign, durable job delivery, exact decimal money, or reconciliation of uncertain bank outcomes.
