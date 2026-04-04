-- Make destination_address optional and add cross-chain destination fields
-- This supports refunds that go through a swap to a different chain/asset

-- Step 1: Recreate the refunds table with the new schema
-- SQLite doesn't support ALTER COLUMN to change nullability,
-- so we recreate the table.

-- Drop the old trigger first
DROP TRIGGER IF EXISTS enforce_refund_status_transition;

-- Rename old table
ALTER TABLE refunds RENAME TO refunds_old;

-- Create new table with updated schema
CREATE TABLE refunds (
    id BLOB PRIMARY KEY,                -- UUID v4
    invoice_id BLOB NOT NULL REFERENCES invoices(id),
    asset_id TEXT NOT NULL,             -- Source token identifier
    asset_name TEXT NOT NULL,           -- Source token name
    chain TEXT NOT NULL,                -- Source blockchain (ChainType)
    amount TEXT NOT NULL,               -- Decimal as text for precision
    source_address TEXT NOT NULL,       -- Sender address
    destination_address TEXT,           -- Recipient address (now optional)
    destination_chain TEXT,             -- Destination chain for cross-chain refunds (SwapChainType)
    destination_asset_id TEXT,          -- Destination asset for cross-chain refunds
    initiator_type TEXT NOT NULL CHECK(initiator_type IN ('System', 'Admin')),
    initiator_id TEXT,                  -- Optional UUID
    status TEXT NOT NULL CHECK(status IN ('Waiting', 'InProgress', 'Completed', 'FailedRetriable', 'Failed')),
    retry_count INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TEXT,
    next_retry_at TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Copy data from old table
INSERT INTO refunds (
    id, invoice_id, asset_id, asset_name, chain, amount,
    source_address, destination_address,
    initiator_type, initiator_id, status,
    retry_count, last_attempt_at, next_retry_at, failure_message,
    created_at, updated_at
)
SELECT
    id, invoice_id, asset_id, asset_name, chain, amount,
    source_address, destination_address,
    initiator_type, initiator_id, status,
    retry_count, last_attempt_at, next_retry_at, failure_message,
    created_at, updated_at
FROM refunds_old;

-- Drop old table
DROP TABLE refunds_old;

-- Recreate indexes
CREATE INDEX idx_refunds_invoice_id ON refunds(invoice_id);
CREATE INDEX idx_refunds_status ON refunds(status);
CREATE INDEX idx_refunds_created_at ON refunds(created_at);

-- Add origin field to swaps table (same pattern as transactions.origin)
-- Stores JSON with optional refund_id, payout_id, internal_transfer_id
ALTER TABLE swaps ADD COLUMN origin TEXT NOT NULL DEFAULT '{}';

-- Add destination fields to payouts table for cross-chain payout support.
-- Default to Polygon — currently the only supported swap destination chain.
ALTER TABLE payouts ADD COLUMN destination_chain TEXT NOT NULL DEFAULT 'Polygon';
ALTER TABLE payouts ADD COLUMN destination_asset_id TEXT;
UPDATE payouts SET destination_asset_id = asset_id WHERE destination_asset_id IS NULL;

-- Recreate status transition trigger
CREATE TRIGGER IF NOT EXISTS enforce_refund_status_transition
BEFORE UPDATE OF status ON refunds
FOR EACH ROW
BEGIN
    SELECT CASE
        WHEN OLD.status = 'Waiting' AND NEW.status != OLD.status AND NEW.status NOT IN ('InProgress')
        THEN RAISE(ABORT, 'REFUND_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)

        WHEN OLD.status = 'InProgress' AND NEW.status != OLD.status AND NEW.status NOT IN ('Completed', 'FailedRetriable', 'Failed')
        THEN RAISE(ABORT, 'REFUND_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)

        WHEN OLD.status = 'FailedRetriable' AND NEW.status != OLD.status AND NEW.status NOT IN ('InProgress')
        THEN RAISE(ABORT, 'REFUND_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)

        WHEN OLD.status IN ('Completed', 'Failed') AND NEW.status != OLD.status
        THEN RAISE(ABORT, 'REFUND_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)
    END;
END;
