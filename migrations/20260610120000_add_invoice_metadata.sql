-- Optional opaque merchant-provided metadata (JSON), echoed back in API
-- responses and webhook payloads. NULL means "not provided".
ALTER TABLE invoices ADD COLUMN metadata TEXT;
