# Database Schema

SQLite via sqlx 0.8. Requires SQLite >= 3.47.0 at runtime.

## Migration Files

- `migrations/20250104000001_initial_schema.sql` — Core tables: invoices, transactions, payouts, refunds, webhook_events
- `migrations/20250211000001_create_front_end_swaps.sql` — Front-end swap tracking
- `migrations/20250218000001_add_transaction_uniqueness_constraints.sql` — Uniqueness constraints
- `migrations/20260303181227_add_swaps.sql` — Swaps
- `migrations/20260401000001_refund_destination_fields.sql` — Refund destination fields
- `migrations/20260515090507_transaction_origin_indexes.sql` — Indexed virtual columns over `transactions.origin`
- `migrations/20260808033820_add_chain_sync_cursors.sql` — Catch-up sweep cursor

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
| cart | TEXT | JSON (TEXT) metadata |
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
| outgoing_meta | TEXT | JSON (TEXT): extrinsic bytes, timestamps, failure info |

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
| payload | TEXT | JSON (TEXT) payload |
| sent | INTEGER | 0 = pending, 1 = sent |

## Operational Tables

Not entities — daemon state that has to survive a restart.

### chain_sync_cursors

How far the catch-up sweep has processed each chain (`daemon/src/chain/transfer_tracker.rs`, issue [#333](https://github.com/Kalapaja/Kalatori/issues/333)). One row per chain; the database file belongs to a single daemon, so no instance column.

| Column | Type | Notes |
|--------|------|-------|
| chain | TEXT | Primary key |
| last_processed_block | INTEGER | Highest block whose transfers are recorded, `CHECK(>= 0)` |
| updated_at | TEXT | ISO 8601; moves only when the cursor advances |

Two properties are load-bearing and easy to break:

- **`last_processed_block` must stay INTEGER.** The cursor may never move backwards, and that guard is `MAX(excluded.last_processed_block, chain_sync_cursors.last_processed_block)` in the upsert. Stored as TEXT the comparison becomes lexicographic, where `'1000' < '999'`. An older head is a normal observation — public RPC pools answer from whichever node took the request, and some lag.
- **The cursor cannot be reconstructed from `transactions`.** On Polygon the transfer path stores no block number (see the TODO above `log_to_transfer` in `chain_client/polygon.rs`), so `transactions.block_number` is NULL there and `SELECT MAX(block_number)` returns nothing.

Re-reading a range already processed is safe: incoming transfers are deduplicated by the unique index on `(chain, tx_hash)`, so the sweep only has to be at-least-once.

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
