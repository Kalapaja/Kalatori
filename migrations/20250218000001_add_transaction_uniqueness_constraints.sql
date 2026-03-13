-- Add uniqueness constraints for transaction deduplication.
-- Partial indexes only apply when the columns are NOT NULL,
-- so pending transactions (with NULL coordinates) are unaffected.

-- EVM chains: tx_hash is the natural unique identifier
CREATE UNIQUE INDEX idx_transactions_unique_tx_hash
    ON transactions(chain, tx_hash)
    WHERE tx_hash IS NOT NULL;

-- Substrate chains: (block_number, position_in_block) is the natural unique identifier
CREATE UNIQUE INDEX idx_transactions_unique_block_position
    ON transactions(chain, block_number, position_in_block)
    WHERE block_number IS NOT NULL AND position_in_block IS NOT NULL;
