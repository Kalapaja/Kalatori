-- Swaps table
-- raw_order stored as JSON
-- secrets stored as JSON array of hex-encoded B256 values
-- request fields are flattened into columns

CREATE TABLE IF NOT EXISTS swaps (
    -- Identity
    id BLOB PRIMARY KEY NOT NULL,  -- UUID v4 - internal ID
    invoice_id BLOB NOT NULL,  -- References invoices.id
    swap_executor TEXT NOT NULL,  -- enum value

    -- Request fields
    from_chain TEXT NOT NULL,       -- enum value
    to_chain TEXT NOT NULL,         -- enum value
    from_token_address TEXT NOT NULL,  -- EVM address (hex)
    to_token_address TEXT NOT NULL,    -- EVM address (hex)
    from_amount_units TEXT NOT NULL,     -- u128 as text to preserve precision
    expected_to_amount_units TEXT NOT NULL,  -- u128 as text to preserve precision
    from_address TEXT NOT NULL,        -- EVM address (hex)
    to_address TEXT NOT NULL,          -- EVM address (hex)

    -- Swap data
    status TEXT NOT NULL CHECK(status IN (
        'Created', 'Submitted', 'Pending', 'Completed', 'Failed'
    )) DEFAULT 'Created',
    estimated_to_amount TEXT NOT NULL,    -- Decimal string (approximate)
    swap_details TEXT NOT NULL,           -- JSON: depending on swap_executor

    -- Timestamps
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    submitted_at TEXT,                 -- NULL until submitted
    finished_at TEXT,                  -- NULL until completed/failed
    valid_till TEXT NOT NULL,

    -- Error tracking
    error_message TEXT,

    FOREIGN KEY (invoice_id) REFERENCES invoices(id) ON DELETE CASCADE
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_swaps_invoice_id ON swaps(invoice_id);
CREATE INDEX IF NOT EXISTS idx_swaps_status ON swaps(status);
CREATE INDEX IF NOT EXISTS idx_swaps_created_at ON swaps(created_at);

-- Status transition enforcement
CREATE TRIGGER IF NOT EXISTS enforce_swap_status_transition
BEFORE UPDATE OF status ON swaps
FOR EACH ROW
BEGIN
    SELECT CASE
        WHEN OLD.status = 'Created' AND NEW.status != OLD.status AND NEW.status NOT IN ('Submitted', 'Failed')
        THEN RAISE(ABORT, 'SWAP_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)

        WHEN OLD.status = 'Submitted' AND NEW.status != OLD.status AND NEW.status NOT IN ('Pending', 'Failed')
        THEN RAISE(ABORT, 'SWAP_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)

        WHEN OLD.status = 'Pending' AND NEW.status != OLD.status AND NEW.status NOT IN ('Completed', 'Failed')
        THEN RAISE(ABORT, 'SWAP_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)

        WHEN OLD.status IN ('Completed', 'Failed') AND NEW.status != OLD.status
        THEN RAISE(ABORT, 'SWAP_STATUS_TRANSITION|old_status=' || OLD.status || '|new_status=' || NEW.status)
    END;
END;
