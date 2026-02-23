use serde_json::json;

use kalatori_client::utils::compute_webhook_signature;

fn main() {
    let test_vectors: Vec<serde_json::Value> = vec![
        // 1. Basic case
        make_vector(
            "secret",
            "POST",
            "/webhooks/invoices",
            r#"{"id":"test"}"#,
            "1706745600",
        ),
        // 2. Empty body
        make_vector(
            "secret",
            "POST",
            "/webhooks/invoices",
            "",
            "1706745600",
        ),
        // 3. Different secret and path
        make_vector(
            "my-webhook-secret-key-2024",
            "POST",
            "/api/v3/webhooks",
            r#"{"event_type":"created","payload":{"amount":"100.00"}}"#,
            "1700000000",
        ),
        // 4. GET method (sorted query params)
        make_vector(
            "secret",
            "GET",
            "/webhooks/invoices",
            "",
            "1706745600",
        ),
        // 5. Long secret
        make_vector(
            "a-very-long-secret-key-that-is-longer-than-sixty-four-bytes-to-test-hmac-key-hashing-behavior",
            "POST",
            "/webhooks/invoices",
            r#"{"test":true}"#,
            "1706745600",
        ),
        // 6. Special characters in body
        make_vector(
            "secret",
            "POST",
            "/webhooks/invoices",
            r#"{"name":"café","price":"€100.00","note":"line1\nline2"}"#,
            "1706745600",
        ),
        // 7. Realistic full GenericEvent<Invoice> payload
        make_vector(
            "production-secret-abc123",
            "POST",
            "/webhooks/invoices",
            &serde_json::to_string(&json!({
                "id": "550e8400-e29b-41d4-a716-446655440000",
                "event_entity": "invoice",
                "event_type": "created",
                "payload": {
                    "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
                    "order_id": "order-12345",
                    "asset_name": "USDT",
                    "asset_id": "1984",
                    "chain": "PolkadotAssetHub",
                    "amount": "100.00",
                    "payment_address": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
                    "status": "Waiting",
                    "payment_url": "https://app.kalatori.com/invoice/7c9e6679-7425-40de-944b-e07fc1f90ae7",
                    "redirect_url": "https://example.com/thank-you",
                    "cart": {
                        "items": [{
                            "name": "Widget Pro",
                            "quantity": 2,
                            "price": "50.00"
                        }]
                    },
                    "total_received_amount": "0",
                    "transactions": [],
                    "valid_till": "2024-02-01T12:00:00Z",
                    "created_at": "2024-01-31T12:00:00Z",
                    "updated_at": "2024-01-31T12:00:00Z"
                },
                "timestamp": "2024-01-31T12:00:00Z"
            })).unwrap(),
            "1706702400",
        ),
        // 8. Paid event with transaction
        make_vector(
            "merchant-secret-xyz",
            "POST",
            "/hooks/kalatori",
            &serde_json::to_string(&json!({
                "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
                "event_entity": "invoice",
                "event_type": "paid",
                "payload": {
                    "id": "deadbeef-1234-5678-9abc-def012345678",
                    "order_id": "ORD-2024-0042",
                    "asset_name": "USDT",
                    "asset_id": "1984",
                    "chain": "PolkadotAssetHub",
                    "amount": "250.50",
                    "payment_address": "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty",
                    "status": "Paid",
                    "payment_url": "https://pay.example.com/inv/deadbeef",
                    "redirect_url": "https://shop.example.com/order/42/complete",
                    "cart": {"items": []},
                    "total_received_amount": "250.50",
                    "transactions": [{
                        "id": "11111111-2222-3333-4444-555555555555",
                        "invoice_id": "deadbeef-1234-5678-9abc-def012345678",
                        "block_number": 12345678,
                        "position_in_block": 2,
                        "tx_hash": "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
                        "transaction_type": "Incoming",
                        "asset_name": "USDT",
                        "asset_id": "1984",
                        "chain": "PolkadotAssetHub",
                        "amount": "250.50",
                        "source_address": "5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy",
                        "destination_address": "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty",
                        "created_at": "2024-01-31T14:30:00Z",
                        "updated_at": "2024-01-31T14:30:00Z",
                        "status": "Confirmed"
                    }],
                    "valid_till": "2024-02-01T12:00:00Z",
                    "created_at": "2024-01-31T12:00:00Z",
                    "updated_at": "2024-01-31T14:30:00Z"
                },
                "timestamp": "2024-01-31T14:30:00Z"
            })).unwrap(),
            "1706711400",
        ),
        // 9. Timestamp edge case: zero
        make_vector(
            "secret",
            "POST",
            "/webhooks/invoices",
            r#"{"minimal":true}"#,
            "0",
        ),
        // 10. Path with trailing slash
        make_vector(
            "secret",
            "POST",
            "/webhooks/invoices/",
            r#"{"id":"test"}"#,
            "1706745600",
        ),
    ];

    println!(
        "{}",
        serde_json::to_string_pretty(&test_vectors).unwrap()
    );
}

fn make_vector(
    secret: &str,
    method: &str,
    path: &str,
    body: &str,
    timestamp: &str,
) -> serde_json::Value {
    let signature = compute_webhook_signature(
        secret.as_bytes(),
        method,
        path,
        body.as_bytes(),
        timestamp,
    );

    json!({
        "secret": secret,
        "method": method,
        "path": path,
        "body": body,
        "timestamp": timestamp,
        "expected_signature": signature
    })
}
