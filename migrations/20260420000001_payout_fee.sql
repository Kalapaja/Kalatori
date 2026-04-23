ALTER TABLE payouts ADD COLUMN fee_wallet TEXT;
ALTER TABLE payouts ADD COLUMN fee_bps INTEGER;
ALTER TABLE payouts ADD COLUMN fee_source TEXT;
ALTER TABLE payouts ADD COLUMN fee_amount TEXT;
