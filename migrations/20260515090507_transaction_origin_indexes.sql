-- Expose payout_id / refund_id from the transactions.origin JSON object as
-- indexed virtual columns so list/filter queries can match them with a
-- sargable equality lookup instead of scanning every row's JSON.

ALTER TABLE transactions ADD COLUMN payout_id TEXT
    GENERATED ALWAYS AS (origin ->> '$.payout_id') VIRTUAL;
ALTER TABLE transactions ADD COLUMN refund_id TEXT
    GENERATED ALWAYS AS (origin ->> '$.refund_id') VIRTUAL;

CREATE INDEX idx_transactions_payout_id ON transactions(payout_id);
CREATE INDEX idx_transactions_refund_id ON transactions(refund_id);
