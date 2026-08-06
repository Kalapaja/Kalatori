//! Known-answer test for the webhook HMAC-SHA256 signature scheme.
//!
//! Merchants verify these signatures with their own implementations, so the
//! exact bytes are a compatibility contract: any change to the dependency
//! stack or the canonicalization (header layout, query sorting, trailing
//! newline handling) must keep reproducing them.
//!
//! Provenance: the vectors in `fixtures/hmac_test_vectors.json` were emitted
//! by `examples/generate_hmac_test_vectors.rs` on the hmac 0.12.1 + sha2
//! 0.10.9 stack and committed before the migration to the digest-0.11 stack
//! (hmac 0.13, sha2 0.11), pinning the pre-migration output. HMAC-SHA256
//! itself is RFC 2104/FIPS 198-1; the algorithm cannot drift, so a mismatch
//! here means the *canonicalization* changed. Do not refresh a failing value
//! from new output — that turns a caught regression into a shipped one.

use kalatori_client::utils::compute_webhook_signature;

#[derive(serde::Deserialize)]
struct Vector {
    secret: String,
    method: String,
    path: String,
    body: String,
    timestamp: String,
    expected_signature: String,
}

#[test]
fn webhook_signatures_match_committed_vectors() {
    let vectors: Vec<Vector> = serde_json::from_str(include_str!(
        "fixtures/hmac_test_vectors.json"
    ))
    .expect("committed fixture parses");
    assert_eq!(
        vectors.len(),
        10,
        "fixture must keep all ten vectors"
    );

    for v in &vectors {
        assert_eq!(
            compute_webhook_signature(
                v.secret.as_bytes(),
                &v.method,
                &v.path,
                v.body.as_bytes(),
                &v.timestamp,
            ),
            v.expected_signature,
            "signature drifted for {} {} (timestamp {})",
            v.method,
            v.path,
            v.timestamp,
        );
    }
}
