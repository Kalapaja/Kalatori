//! PKCE state encryption for the OAuth authorization flow.
//!
//! The OAuth `state` parameter carries an encrypted PKCE `code_verifier` so the
//! daemon can recover it on the callback without storing server-side state.
//!
//! Key derivation: HKDF-SHA256 with `client_secret` as IKM, `client_id`
//! (UTF-8) as salt, and `"kalatori-state-v1"` as info string.
//!
//! Encryption: XChaCha20-Poly1305 with a fresh 24-byte random nonce per state
//! value. Output: `base64url(nonce || ciphertext || auth_tag)`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{
    Aead,
    KeyInit,
};
use hkdf::Hkdf;
use secrecy::{
    ExposeSecret,
    SecretBox,
};
use sha2::Sha256;

use super::errors::OAuthError;

const HKDF_INFO: &[u8] = b"kalatori-state-v1";
const NONCE_LEN: usize = 24;

/// A derived 256-bit key for state encryption, zeroized on drop via `secrecy`.
pub type StateKey = SecretBox<[u8; 32]>;

/// Derive a state encryption key from `client_secret` using HKDF-SHA256.
///
/// - IKM: raw bytes of `client_secret`
/// - Salt: UTF-8 encoding of `client_id` (binds key to specific daemon)
/// - Info: `"kalatori-state-v1"`
pub fn derive_state_key(
    client_secret: &[u8],
    client_id: &str,
) -> StateKey {
    let hk = Hkdf::<Sha256>::new(
        Some(client_id.as_bytes()),
        client_secret,
    );
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("HKDF-SHA256 expand should not fail for 32-byte output");
    SecretBox::new(Box::new(okm))
}

/// Encrypt a PKCE `code_verifier` into a base64url-encoded state parameter.
///
/// Generates a fresh 24-byte random nonce for each call.
/// Output format: `base64url(nonce || ciphertext_with_tag)`.
pub fn encrypt_state(
    code_verifier: &str,
    key: &StateKey,
) -> String {
    let cipher = XChaCha20Poly1305::new(key.expose_secret().into());
    let nonce = chacha20poly1305::XNonce::from(rand::random::<[u8; NONCE_LEN]>());

    let ciphertext = cipher
        .encrypt(&nonce, code_verifier.as_bytes())
        .expect("XChaCha20-Poly1305 encryption should not fail");

    let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&ciphertext);

    URL_SAFE_NO_PAD.encode(&output)
}

/// Decrypt a base64url-encoded state parameter back to the PKCE
/// `code_verifier`.
pub fn decrypt_state(
    encoded: &str,
    key: &StateKey,
) -> Result<String, OAuthError> {
    let data = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| OAuthError::StateDecryptionFailed)?;

    if data.len() <= NONCE_LEN {
        return Err(OAuthError::StateDecryptionFailed);
    }

    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = chacha20poly1305::XNonce::from_slice(nonce_bytes);

    let cipher = XChaCha20Poly1305::new(key.expose_secret().into());
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| OAuthError::StateDecryptionFailed)?;

    String::from_utf8(plaintext).map_err(|_| OAuthError::StateDecryptionFailed)
}

/// Attempt decryption with the current key, falling back to the previous key
/// during secret rotation (spec §6.2.2).
pub fn decrypt_state_with_fallback(
    encoded: &str,
    current_key: &StateKey,
    previous_key: Option<&StateKey>,
) -> Result<String, OAuthError> {
    match decrypt_state(encoded, current_key) {
        Ok(verifier) => Ok(verifier),
        Err(_) => match previous_key {
            Some(prev) => decrypt_state(encoded, prev),
            None => Err(OAuthError::StateDecryptionFailed),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let key = derive_state_key(b"my-secret", "daemon-01");
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

        let encrypted = encrypt_state(verifier, &key);
        let decrypted = decrypt_state(&encrypted, &key).unwrap();
        assert_eq!(decrypted, verifier);
    }

    #[test]
    fn test_different_keys_fail() {
        let key1 = derive_state_key(b"secret-1", "daemon-01");
        let key2 = derive_state_key(b"secret-2", "daemon-01");

        let encrypted = encrypt_state("verifier", &key1);
        assert!(decrypt_state(&encrypted, &key2).is_err());
    }

    #[test]
    fn test_different_client_ids_produce_different_keys() {
        let key_a = derive_state_key(b"same-secret", "daemon-a");
        let key_b = derive_state_key(b"same-secret", "daemon-b");

        let encrypted = encrypt_state("verifier", &key_a);
        assert!(decrypt_state(&encrypted, &key_b).is_err());
    }

    #[test]
    fn test_fallback_to_previous_key() {
        let old_key = derive_state_key(b"old-secret", "daemon-01");
        let new_key = derive_state_key(b"new-secret", "daemon-01");

        // Encrypted with old key (in-flight auth callback during rotation)
        let encrypted = encrypt_state("verifier", &old_key);

        // Current key fails, falls back to previous
        let result = decrypt_state_with_fallback(&encrypted, &new_key, Some(&old_key));
        assert_eq!(result.unwrap(), "verifier");
    }

    #[test]
    fn test_fallback_without_previous_key_fails() {
        let old_key = derive_state_key(b"old-secret", "daemon-01");
        let new_key = derive_state_key(b"new-secret", "daemon-01");

        let encrypted = encrypt_state("verifier", &old_key);
        let result = decrypt_state_with_fallback(&encrypted, &new_key, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let key = derive_state_key(b"secret", "daemon-01");
        let encrypted = encrypt_state("verifier", &key);

        let mut data = URL_SAFE_NO_PAD
            .decode(&encrypted)
            .unwrap();
        // Flip a byte in the ciphertext portion
        let last = data.len() - 1;
        data[last] ^= 0xFF;
        let tampered = URL_SAFE_NO_PAD.encode(&data);

        assert!(decrypt_state(&tampered, &key).is_err());
    }

    #[test]
    fn test_output_is_base64url_safe() {
        let key = derive_state_key(b"secret", "daemon-01");
        let encrypted = encrypt_state("some-verifier-value", &key);

        // Must not contain +, /, or = (base64url without padding)
        assert!(!encrypted.contains('+'));
        assert!(!encrypted.contains('/'));
        assert!(!encrypted.contains('='));
    }

    #[test]
    fn test_each_encryption_produces_unique_output() {
        let key = derive_state_key(b"secret", "daemon-01");
        let a = encrypt_state("verifier", &key);
        let b = encrypt_state("verifier", &key);
        // Fresh nonce each time → different ciphertext
        assert_ne!(a, b);
    }

    #[test]
    fn test_hkdf_deterministic() {
        let k1 = derive_state_key(b"secret", "daemon-01");
        let k2 = derive_state_key(b"secret", "daemon-01");
        assert_eq!(k1.expose_secret(), k2.expose_secret());
    }
}
