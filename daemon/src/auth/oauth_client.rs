//! OAuth flow driver: authorize URL construction, code exchange, token refresh.
//!
//! All server-to-server calls use `reqwest` directly (no `oauth2` crate) since
//! the auth server uses PASETO tokens and a non-standard refresh endpoint.

use pasetors::keys::AsymmetricPublicKey;
use pasetors::version4::V4;
use secrecy::ExposeSecret;

use crate::configs::OAuthConfig;
use crate::utils::logging::{
    category,
    operation,
};

use super::errors::{
    OAuthError,
    RefreshError,
};
use super::state_encryption::{
    StateKey,
    decrypt_state_with_fallback,
    derive_state_key,
    encrypt_state,
};
use super::token::{
    TokenClaims,
    verify_token,
};

/// Response shape from the auth server's token and refresh endpoints.
#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    // token_type and expires_in are present but unused — the PASETO token's
    // own claims are authoritative.
}

/// OAuth error response per RFC 6749 §5.2.
#[derive(serde::Deserialize)]
struct OAuthErrorResponse {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Drives the OAuth 2.0 Authorization Code + PKCE flow and token refresh.
///
/// Stateless except for pre-derived encryption keys and parsed public keys.
pub struct OAuthClient {
    http: reqwest::Client,
    config: OAuthConfig,
    public_keys: Vec<AsymmetricPublicKey<V4>>,
    state_key: StateKey,
    previous_state_key: Option<StateKey>,
}

impl OAuthClient {
    /// Create a new OAuth client from validated config.
    ///
    /// Parses PASERK public keys and derives HKDF state encryption keys.
    pub fn new(config: OAuthConfig) -> Self {
        let public_keys = super::token::parse_public_keys(&config.token_public_keys);

        let state_key = derive_state_key(
            config
                .client_secret
                .expose_secret()
                .as_bytes(),
            &config.client_id,
        );

        let previous_state_key = config
            .previous_client_secret
            .as_ref()
            .map(|prev| {
                derive_state_key(
                    prev.expose_secret().as_bytes(),
                    &config.client_id,
                )
            });

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client for OAuth");

        Self {
            http,
            config,
            public_keys,
            state_key,
            previous_state_key,
        }
    }

    /// Build the authorization URL and encrypted state for starting the SSO
    /// flow.
    ///
    /// Returns `(authorize_url, state)`. The daemon redirects the user's
    /// browser to `authorize_url`. The `state` is opaque to the auth server
    /// and returned unchanged via `form_post`.
    pub fn start_auth_flow(&self) -> (String, PkceVerifier) {
        let code_verifier = generate_code_verifier();
        let code_challenge = compute_s256_challenge(&code_verifier);
        let state = encrypt_state(&code_verifier, &self.state_key);

        let redirect_uri = self.redirect_uri();

        let mut url = url::Url::parse(&format!(
            "{}/oauth/authorize",
            self.config.auth_server_url
        ))
        .expect("auth_server_url should be a valid URL");

        url.query_pairs_mut()
            .append_pair("response_mode", "form_post")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("redirect_uri", &redirect_uri);

        (
            url.to_string(),
            PkceVerifier {
                state,
            },
        )
    }

    /// Exchange an authorization code for a PASETO access token.
    ///
    /// Called from the `/auth/callback` handler after receiving `code` and
    /// `state` from the auth server's `form_post`.
    pub async fn exchange_code(
        &self,
        code: &str,
        state: &str,
    ) -> Result<TokenClaims, OAuthError> {
        // Decrypt state to recover code_verifier (spec §6.2)
        let code_verifier = decrypt_state_with_fallback(
            state,
            &self.state_key,
            self.previous_state_key.as_ref(),
        )?;

        let redirect_uri = self.redirect_uri();

        // POST to /oauth/token (spec §6.5)
        let response = self
            .http
            .post(format!(
                "{}/oauth/token",
                self.config.auth_server_url
            ))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", &code_verifier),
                ("client_id", &self.config.client_id),
                (
                    "client_secret",
                    self.config
                        .client_secret
                        .expose_secret(),
                ),
                ("redirect_uri", &redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    error.category = category::AUTH,
                    error.operation = operation::CODE_EXCHANGE,
                    error.source = ?e,
                    "Auth server unreachable during code exchange"
                );
                OAuthError::AuthServerUnreachable
            })?;

        if !response.status().is_success() {
            return Err(self
                .handle_oauth_error(response, operation::CODE_EXCHANGE)
                .await);
        }

        let token_response: TokenResponse = response.json().await.map_err(|e| {
            tracing::warn!(
                error.category = category::AUTH,
                error.operation = operation::CODE_EXCHANGE,
                error.source = ?e,
                "Failed to parse token response"
            );
            OAuthError::CodeExchangeFailed {
                error_code: "invalid_response".to_string(),
                description: "Failed to parse token response".to_string(),
                http_status: 0,
            }
        })?;

        // Verify the returned PASETO token (spec §6.5)
        verify_token(
            &token_response.access_token,
            &self.public_keys,
            &self.config.auth_server_url,
            &self.config.client_id,
        )
        .map_err(|e| {
            tracing::warn!(
                error.category = category::AUTH,
                error.operation = operation::CODE_EXCHANGE,
                error.source = ?e,
                "Token verification failed after code exchange"
            );
            OAuthError::TokenVerificationFailed(e)
        })
    }

    /// Refresh an access token via the non-standard `/auth/token-refresh`
    /// endpoint (spec §7.3).
    ///
    /// Returns the new raw PASETO token string and parsed claims on success.
    pub async fn refresh_token(
        &self,
        current_token: &str,
    ) -> Result<TokenClaims, RefreshError> {
        let response = self
            .http
            .post(format!(
                "{}/auth/token-refresh",
                self.config.auth_server_url
            ))
            .form(&[
                ("access_token", current_token),
                (
                    "client_id",
                    self.config.client_id.as_str(),
                ),
                (
                    "client_secret",
                    self.config
                        .client_secret
                        .expose_secret(),
                ),
            ])
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    error.category = category::AUTH,
                    error.operation = operation::TOKEN_REFRESH,
                    error.source = ?e,
                    "Auth server unreachable during token refresh"
                );
                RefreshError::AuthServerUnreachable
            })?;

        if !response.status().is_success() {
            return Err(self
                .handle_refresh_error(response)
                .await);
        }

        let token_response: TokenResponse = response.json().await.map_err(|e| {
            tracing::warn!(
                error.category = category::AUTH,
                error.operation = operation::TOKEN_REFRESH,
                error.source = ?e,
                "Failed to parse refresh response"
            );
            RefreshError::InvalidResponse
        })?;

        verify_token(
            &token_response.access_token,
            &self.public_keys,
            &self.config.auth_server_url,
            &self.config.client_id,
        )
        .map_err(|e| {
            tracing::warn!(
                error.category = category::AUTH,
                error.operation = operation::TOKEN_REFRESH,
                error.source = ?e,
                "Refreshed token verification failed"
            );
            RefreshError::TokenVerificationFailed(e)
        })
    }

    /// The configured `auth_server_url` for Origin validation in the callback
    /// handler.
    pub fn auth_server_url(&self) -> &str {
        &self.config.auth_server_url
    }

    /// The configured `client_id`.
    pub fn client_id(&self) -> &str {
        &self.config.client_id
    }

    /// Clock tolerance in seconds.
    pub fn clock_tolerance(&self) -> u64 {
        self.config.clock_tolerance
    }

    /// The parsed PASETO public keys.
    pub fn public_keys(&self) -> &[AsymmetricPublicKey<V4>] {
        &self.public_keys
    }

    fn redirect_uri(&self) -> String {
        format!("{}/auth/callback", self.config.base_url)
    }

    /// Parse an OAuth error response and map to `OAuthError`.
    async fn handle_oauth_error(
        &self,
        response: reqwest::Response,
        op: &str,
    ) -> OAuthError {
        let status = response.status().as_u16();
        let error_response: OAuthErrorResponse = response
            .json()
            .await
            .unwrap_or_else(|_| OAuthErrorResponse {
                error: "unknown".to_string(),
                error_description: "Failed to parse error response".to_string(),
            });

        tracing::warn!(
            error.category = category::AUTH,
            error.operation = op,
            error.code = %error_response.error,
            error.description = %error_response.error_description,
            http.status = status,
            "OAuth error from auth server"
        );

        OAuthError::CodeExchangeFailed {
            error_code: error_response.error,
            description: error_response.error_description,
            http_status: status,
        }
    }

    /// Parse a refresh error response and map to `RefreshError` per spec §7.3.
    async fn handle_refresh_error(
        &self,
        response: reqwest::Response,
    ) -> RefreshError {
        let status = response.status().as_u16();
        let error_response: OAuthErrorResponse = response
            .json()
            .await
            .unwrap_or_else(|_| OAuthErrorResponse {
                error: "unknown".to_string(),
                error_description: "Failed to parse error response".to_string(),
            });

        tracing::warn!(
            error.category = category::AUTH,
            error.operation = operation::TOKEN_REFRESH,
            error.code = %error_response.error,
            error.description = %error_response.error_description,
            http.status = status,
            "Refresh error from auth server"
        );

        match (status, error_response.error.as_str()) {
            (401, "invalid_client") => RefreshError::ConfigError,
            (403, "access_denied") => RefreshError::AccessDenied,
            (403, "token_expired") => RefreshError::TokenTooOld,
            (403, "support_session_expired") => RefreshError::SupportSessionExpired,
            _ => RefreshError::InvalidResponse,
        }
    }
}

/// Opaque type holding the encrypted state from `start_auth_flow`. Passed back
/// to `exchange_code` via the callback.
#[expect(dead_code)]
pub struct PkceVerifier {
    pub state: String,
}

/// Generate a cryptographically random PKCE code verifier (43–128 chars,
/// unreserved characters per RFC 7636 §4.1).
fn generate_code_verifier() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let random_bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(random_bytes)
}

/// Compute the S256 PKCE code challenge from a verifier.
/// `challenge = BASE64URL(SHA256(verifier))`
fn compute_s256_challenge(verifier: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use sha2::Digest;

    let hash = sha2::Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_verifier_length() {
        let verifier = generate_code_verifier();
        // 32 bytes → 43 base64url chars (no padding)
        assert_eq!(verifier.len(), 43);
        // Must be URL-safe
        assert!(!verifier.contains('+'));
        assert!(!verifier.contains('/'));
        assert!(!verifier.contains('='));
    }

    #[test]
    fn test_s256_challenge_deterministic() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let c1 = compute_s256_challenge(verifier);
        let c2 = compute_s256_challenge(verifier);
        assert_eq!(c1, c2);
        // Must be URL-safe base64
        assert!(!c1.contains('+'));
        assert!(!c1.contains('/'));
    }

    #[test]
    fn test_verifiers_are_unique() {
        let v1 = generate_code_verifier();
        let v2 = generate_code_verifier();
        assert_ne!(v1, v2);
    }
}
