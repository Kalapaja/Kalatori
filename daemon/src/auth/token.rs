use std::convert::TryFrom;

use chrono::{
    DateTime,
    Utc,
};
use pasetors::claims::ClaimsValidationRules;
use pasetors::keys::AsymmetricPublicKey;
use pasetors::token::UntrustedToken;
use pasetors::version4::V4;
use pasetors::{
    Public,
    public,
};
use serde::{
    Deserialize,
    Serialize,
};

use super::errors::TokenError;

/// Parsed and verified PASETO token claims.
#[derive(Debug, Clone)]
pub struct TokenClaims {
    /// Auth server URL (must match configured `auth_server_url`).
    pub iss: String,
    /// Stable, opaque user identifier from the auth server.
    pub sub: String,
    /// User's email address.
    pub email: String,
    /// The `client_id` this token was issued for.
    pub aud: String,
    /// User's role for this daemon.
    pub role: Role,
    /// When the token was issued.
    pub iat: DateTime<Utc>,
    /// When the token expires.
    pub exp: DateTime<Utc>,
    /// The raw PASETO token string (needed for refresh requests).
    pub raw_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Operator,
    Viewer,
    Support,
}

/// Parse PASERK `k4.public.*` strings into public keys at startup.
///
/// # Panics
///
/// Panics if any key string is not a valid PASERK k4.public key. This is
/// called during config validation, so a panic is appropriate.
pub fn parse_public_keys(paserk_keys: &[String]) -> Vec<AsymmetricPublicKey<V4>> {
    paserk_keys
        .iter()
        .enumerate()
        .map(|(i, key_str)| {
            AsymmetricPublicKey::<V4>::try_from(key_str.as_str()).unwrap_or_else(|e| {
                panic!("auth config: failed to parse token_public_keys[{i}]: {e}")
            })
        })
        .collect()
}

/// Verify a PASETO v4.public token signature and extract claims.
///
/// Tries each public key in order and accepts the token if any key produces a
/// valid signature. Validates `iss` and `aud` claims. Does NOT check `exp` —
/// the caller decides based on context (request authorization vs refresh
/// eligibility have different rules per spec §7.2).
pub fn verify_token(
    token: &str,
    keys: &[AsymmetricPublicKey<V4>],
    expected_issuer: &str,
    expected_audience: &str,
) -> Result<TokenClaims, TokenError> {
    // Disable all time validation — the caller handles expiry logic
    // (midpoint, grace window, etc.) per spec §7.2.
    let mut rules = ClaimsValidationRules::new();
    rules.allow_non_expiring();
    rules.disable_valid_at();
    rules.validate_issuer_with(expected_issuer);
    rules.validate_audience_with(expected_audience);

    let untrusted =
        UntrustedToken::<Public, V4>::try_from(token).map_err(|_| TokenError::InvalidSignature)?;

    // Try each key in order (supports key rotation per spec §10.1)
    let mut last_err = None;
    for key in keys {
        match public::verify(key, &untrusted, &rules, None, None) {
            Ok(trusted) => return extract_claims(trusted, token),
            Err(e) => last_err = Some(e),
        }
    }

    // No key verified — determine if it's a signature or claims issue
    match last_err {
        Some(pasetors::errors::Error::ClaimValidation(ref cv)) => {
            let desc = format!("{cv:?}");
            if desc.contains("Iss") {
                Err(TokenError::IssuerMismatch)
            } else if desc.contains("Aud") {
                Err(TokenError::AudienceMismatch)
            } else {
                Err(TokenError::InvalidClaims {
                    reason: desc,
                })
            }
        },
        _ => Err(TokenError::InvalidSignature),
    }
}

/// Extract typed claims from a verified token.
fn extract_claims(
    trusted: pasetors::token::TrustedToken,
    raw_token: &str,
) -> Result<TokenClaims, TokenError> {
    let claims = trusted
        .payload_claims()
        .ok_or_else(|| TokenError::InvalidClaims {
            reason: "no claims in token payload".to_string(),
        })?;

    let iss = get_string_claim(claims, "iss")?;
    let sub = get_string_claim(claims, "sub")?;
    let email = get_string_claim(claims, "email")?;
    let aud = get_string_claim(claims, "aud")?;
    let role_str = get_string_claim(claims, "role")?;
    let iat_str = get_string_claim(claims, "iat")?;
    let exp_str = get_string_claim(claims, "exp")?;

    let role: Role = serde_json::from_value(serde_json::Value::String(role_str)).map_err(|_| {
        TokenError::InvalidClaims {
            reason: "invalid role value".to_string(),
        }
    })?;

    let iat = DateTime::parse_from_rfc3339(&iat_str)
        .map_err(|_| TokenError::InvalidClaims {
            reason: "iat is not valid RFC 3339".to_string(),
        })?
        .to_utc();

    let exp = DateTime::parse_from_rfc3339(&exp_str)
        .map_err(|_| TokenError::InvalidClaims {
            reason: "exp is not valid RFC 3339".to_string(),
        })?
        .to_utc();

    Ok(TokenClaims {
        iss,
        sub,
        email,
        aud,
        role,
        iat,
        exp,
        raw_token: raw_token.to_owned(),
    })
}

fn get_string_claim(
    claims: &pasetors::claims::Claims,
    name: &str,
) -> Result<String, TokenError> {
    claims
        .get_claim(name)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| TokenError::InvalidClaims {
            reason: format!("missing or non-string claim: {name}"),
        })
}

/// Check if a token is expired, accounting for clock tolerance.
pub fn is_expired(
    claims: &TokenClaims,
    clock_tolerance_secs: u64,
) -> bool {
    let now = Utc::now();
    let tolerance =
        chrono::Duration::seconds(i64::try_from(clock_tolerance_secs).unwrap_or(i64::MAX));
    now > claims.exp + tolerance
}

/// Check if a token is past its midpoint (should trigger opportunistic
/// refresh). Midpoint = iat + (exp - iat) / 2, adapting to clipped support
/// session tokens per spec §7.2.
pub fn is_past_midpoint(claims: &TokenClaims) -> bool {
    let lifetime = claims.exp - claims.iat;
    let midpoint = claims.iat + lifetime / 2;
    Utc::now() > midpoint
}

/// Check if an expired token is within the 5-minute refresh grace window
/// (auth server will still accept it for refresh).
pub fn is_within_refresh_grace(claims: &TokenClaims) -> bool {
    let now = Utc::now();
    let grace_end = claims.exp + chrono::Duration::minutes(5);
    now <= grace_end
}

// ============================================================================
// Token signing (dev + test only)
// ============================================================================

/// Generate an Ed25519 keypair for dev/test token signing.
///
/// Returns the keypair and the public key as a PASERK `k4.public.*` string
/// suitable for `token_public_keys` in config.
#[cfg(any(feature = "dev_api", test))]
pub fn generate_dev_keypair() -> (pasetors::keys::AsymmetricKeyPair<V4>, String) {
    use pasetors::keys::{
        AsymmetricKeyPair,
        Generate,
    };
    use pasetors::paserk::FormatAsPaserk;

    let kp = AsymmetricKeyPair::<V4>::generate().expect("Ed25519 keypair generation should not fail");
    let mut paserk_public = String::new();
    kp.public
        .fmt(&mut paserk_public)
        .expect("PASERK formatting should not fail");
    (kp, paserk_public)
}

/// Sign a PASETO v4.public token from `TokenClaims`.
///
/// The resulting token string can be verified with `verify_token()`.
#[cfg(any(feature = "dev_api", test))]
pub fn sign_token(
    secret_key: &pasetors::keys::AsymmetricSecretKey<V4>,
    claims: &TokenClaims,
) -> String {
    let role_str = serde_json::to_value(claims.role)
        .expect("Role serialization should not fail")
        .as_str()
        .expect("Role should serialize to a string")
        .to_owned();

    let mut paseto_claims =
        pasetors::claims::Claims::new().expect("Claims construction should not fail");
    paseto_claims
        .issuer(&claims.iss)
        .expect("setting issuer should not fail");
    paseto_claims
        .subject(&claims.sub)
        .expect("setting subject should not fail");
    paseto_claims
        .audience(&claims.aud)
        .expect("setting audience should not fail");
    paseto_claims
        .issued_at(&claims.iat.to_rfc3339())
        .expect("setting iat should not fail");
    paseto_claims
        .expiration(&claims.exp.to_rfc3339())
        .expect("setting exp should not fail");
    paseto_claims
        .add_additional("email", claims.email.clone())
        .expect("setting email should not fail");
    paseto_claims
        .add_additional("role", role_str)
        .expect("setting role should not fail");

    public::sign(secret_key, &paseto_claims, None, None)
        .expect("PASETO v4.public signing should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_deserialization() {
        assert_eq!(
            serde_json::from_str::<Role>("\"owner\"").unwrap(),
            Role::Owner
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"operator\"").unwrap(),
            Role::Operator
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"viewer\"").unwrap(),
            Role::Viewer
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"support\"").unwrap(),
            Role::Support
        );
        assert!(serde_json::from_str::<Role>("\"admin\"").is_err());
    }

    #[test]
    fn test_is_expired_with_tolerance() {
        let claims = TokenClaims {
            iss: String::new(),
            sub: String::new(),
            email: String::new(),
            aud: String::new(),
            role: Role::Owner,
            iat: Utc::now() - chrono::Duration::minutes(16),
            exp: Utc::now() - chrono::Duration::minutes(1),
            raw_token: String::new(),
        };
        // Expired 1 minute ago, 30s tolerance → expired
        assert!(is_expired(&claims, 30));
        // Expired 1 minute ago, 120s tolerance → not expired
        assert!(!is_expired(&claims, 120));
    }

    #[test]
    fn test_is_past_midpoint() {
        let now = Utc::now();
        // Token with 15-min lifetime, issued 8 minutes ago → past midpoint
        let claims = TokenClaims {
            iss: String::new(),
            sub: String::new(),
            email: String::new(),
            aud: String::new(),
            role: Role::Owner,
            iat: now - chrono::Duration::minutes(8),
            exp: now + chrono::Duration::minutes(7),
            raw_token: String::new(),
        };
        assert!(is_past_midpoint(&claims));

        // Token issued 2 minutes ago with 15-min lifetime → not past midpoint
        let claims2 = TokenClaims {
            iss: String::new(),
            sub: String::new(),
            email: String::new(),
            aud: String::new(),
            role: Role::Owner,
            iat: now - chrono::Duration::minutes(2),
            exp: now + chrono::Duration::minutes(13),
            raw_token: String::new(),
        };
        assert!(!is_past_midpoint(&claims2));
    }

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let (kp, paserk_public) = generate_dev_keypair();
        let public_keys = parse_public_keys(&[paserk_public]);

        let now = Utc::now();
        let claims = TokenClaims {
            iss: "https://auth.example.com".to_string(),
            sub: "user-42".to_string(),
            email: "test@example.com".to_string(),
            aud: "daemon-01".to_string(),
            role: Role::Operator,
            iat: now,
            exp: now + chrono::Duration::hours(1),
            raw_token: String::new(),
        };

        let token = sign_token(&kp.secret, &claims);
        assert!(token.starts_with("v4.public."));

        let verified = verify_token(
            &token,
            &public_keys,
            "https://auth.example.com",
            "daemon-01",
        )
        .expect("roundtrip verification should succeed");

        assert_eq!(verified.iss, claims.iss);
        assert_eq!(verified.sub, claims.sub);
        assert_eq!(verified.email, claims.email);
        assert_eq!(verified.aud, claims.aud);
        assert_eq!(verified.role, claims.role);
        assert_eq!(verified.raw_token, token);
    }

    #[test]
    fn test_is_within_refresh_grace() {
        let now = Utc::now();
        // Expired 3 minutes ago → within 5-min grace
        let claims = TokenClaims {
            iss: String::new(),
            sub: String::new(),
            email: String::new(),
            aud: String::new(),
            role: Role::Owner,
            iat: now - chrono::Duration::minutes(18),
            exp: now - chrono::Duration::minutes(3),
            raw_token: String::new(),
        };
        assert!(is_within_refresh_grace(&claims));

        // Expired 6 minutes ago → beyond grace
        let claims2 = TokenClaims {
            iss: String::new(),
            sub: String::new(),
            email: String::new(),
            aud: String::new(),
            role: Role::Owner,
            iat: now - chrono::Duration::minutes(21),
            exp: now - chrono::Duration::minutes(6),
            raw_token: String::new(),
        };
        assert!(!is_within_refresh_grace(&claims2));
    }
}
