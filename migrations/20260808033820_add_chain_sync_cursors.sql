-- Track how far the catch-up sweep has processed each chain (issue #333).
--
-- The sweep re-reads logs the live subscription may have missed, so it needs a
-- restart-surviving watermark. This cannot be derived from existing tables: on
-- Polygon the transfer path stores no block number at all (see the TODO above
-- `log_to_transfer` in `chain_client/polygon.rs`), so `transactions` holds NULL
-- there and `SELECT MAX(block_number)` would silently return nothing.
--
-- One row per chain. The database file belongs to a single daemon
-- (`DatabaseConfig` is a local SQLite path), so no instance column is needed.
--
-- `last_processed_block` is INTEGER, not TEXT, on purpose: the cursor must
-- never move backwards, and that guard is a numeric comparison. Stored as TEXT
-- it would compare lexicographically, where '1000' < '999'.

CREATE TABLE IF NOT EXISTS chain_sync_cursors (
    chain                TEXT PRIMARY KEY NOT NULL,
    last_processed_block INTEGER NOT NULL CHECK(last_processed_block >= 0),
    updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
);
