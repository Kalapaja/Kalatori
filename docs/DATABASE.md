# Database Schema

SQLite via sqlx 0.8. Requires SQLite >= 3.47.0 at runtime.

## Migration Files

- `migrations/20250104000001_initial_schema.sql` — Core tables: invoices, transactions, payouts, refunds, webhook_events
- `migrations/20250211000001_create_front_end_swaps.sql` — Front-end swap tracking
- `migrations/20250218000001_add_transaction_uniqueness_constraints.sql` — Uniqueness constraints

Run migrations: `make sqlx-migrate`
Prepare for compile-time verification: `make sqlx-prepare`

## Core Tables

### invoices
Primary entity. Tracks payment requests from merchants.

| Column | Type | Notes |
|--------|------|-------|
| id | BLOB (UUID v4) | Internal ID |
| order_id | TEXT | Merchant-provided, unique |
| asset_id, asset_name, chain | TEXT | Denormalized asset info |
| amount | TEXT | Decimal string (e.g., "123.456789") |
| payment_address | TEXT | Derived HD address |
| status | TEXT | See status transitions below |
| cart | TEXT | JSONB metadata |
| redirect_url | TEXT | Post-payment redirect |
| valid_till | TEXT | ISO 8601 expiration |
| created_at, updated_at | TEXT | ISO 8601 timestamps |

**Invoice statuses**: `Waiting` -> `PartiallyPaid` -> `Paid` / `OverPaid` / `PartiallyPaidExpired` / `AdminCanceled`. Also: `Waiting` -> `UnpaidExpired` / `CustomerCanceled` / `AdminCanceled`. Final statuses cannot transition further (enforced by DB trigger).

### transactions
Unified table for both incoming (customer payments) and outgoing (payouts/refunds).

| Column | Type | Notes |
|--------|------|-------|
| id | BLOB (UUID v4) | Internal ID |
| invoice_id | BLOB | FK to invoices |
| asset_id, asset_name, chain, amount | TEXT | Asset details |
| source_address, destination_address | TEXT | Addresses |
| block_number, position_in_block | INTEGER | NULL until finalized |
| tx_hash | TEXT | NULL until finalized |
| status | TEXT | `Waiting` -> `InProgress` -> `Completed` / `Failed` |
| transaction_type | TEXT | `Incoming` or `Outgoing` |
| outgoing_meta | TEXT | JSONB: extrinsic bytes, timestamps, failure info |

### payouts
Transfers from payment address to merchant's wallet.

| Column | Type | Notes |
|--------|------|-------|
| id | BLOB (UUID v4) | Internal ID |
| invoice_id | BLOB | FK to invoices |
| initiator_type | TEXT | `System` or `Admin` |
| status | TEXT | `Waiting` -> `InProgress` -> `Completed` / `FailedRetriable` / `Failed` |
| retry_count | INTEGER | Retry mechanism |
| next_retry_at, last_attempt_at | TEXT | Retry scheduling |

`FailedRetriable` -> `InProgress` allows retry. `Completed` and `Failed` are terminal.

### refunds
Refunds from payment address back to customer.
Same structure as payouts (status, retry mechanism, initiator).

### webhook_events
Queue of webhook notifications to send to merchant's configured URL.

| Column | Type | Notes |
|--------|------|-------|
| id | BLOB (UUID v4) | Internal ID |
| entity_id | BLOB | References any entity |
| payload | TEXT | JSONB payload |
| sent | INTEGER | 0 = pending, 1 = sent |

## Status Transition Triggers

Database-level triggers enforce valid status transitions. Error format: `ERROR_TYPE|old_status=VALUE|new_status=VALUE` — parsed by application code in `daemon/src/dao/error_parsing.rs`.

## DAO Pattern

`DaoInterface` and `DaoTransactionInterface` traits in `daemon/src/dao/interface.rs` define the data access contract. Both are mockable via `mockall` for unit testing.

`DaoExecutor` trait in `daemon/src/dao.rs` provides generic query execution for both `DAO` (direct) and `DaoTransaction` (within SQLite transaction).

**Conventions** (see [docs/conventions.md](conventions.md)):
- Methods are single-responsibility (create, read, update)
- All mutations return the full updated object
- `updated_at` managed manually in SQL (not via triggers)
- `NaiveDateTime` for SQL parameter binding
