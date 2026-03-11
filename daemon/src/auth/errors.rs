use axum::http::StatusCode;
use thiserror::Error;

use crate::api::ApiErrorExt;

// ============================================================================
// Domain 1: Token Verification Errors
// ============================================================================

/// Errors from PASETO token signature verification and claims validation.
#[derive(Debug, Error)]
pub enum TokenError {
    /// No configured public key produced a valid signature.
    #[error("Token signature verification failed")]
    InvalidSignature,

    /// Token claims are missing or malformed.
    #[error("Invalid token claims: {reason}")]
    InvalidClaims { reason: String },

    /// The `iss` claim does not match the configured `auth_server_url`.
    #[error("Token issuer mismatch")]
    IssuerMismatch,

    /// The `aud` claim does not match the daemon's `client_id`.
    #[error("Token audience mismatch")]
    AudienceMismatch,
}

// ============================================================================
// Domain 2: OAuth Flow Errors
// ============================================================================

/// Errors during the OAuth authorization code flow (callback handling and code
/// exchange).
#[derive(Debug, Error)]
pub enum OAuthError {
    /// AEAD decryption of the `state` parameter failed (tampered or wrong key).
    #[error("State parameter decryption failed")]
    StateDecryptionFailed,

    /// The `Origin` / `Referer` header does not match `auth_server_url`.
    #[error("Callback origin validation failed")]
    InvalidOrigin,

    /// The auth server returned an error during code exchange.
    #[error("Code exchange failed: {error_code}")]
    CodeExchangeFailed {
        error_code: String,
        description: String,
        http_status: u16,
    },

    /// The auth server is unreachable (network error, DNS failure, timeout).
    #[error("Auth server unreachable")]
    AuthServerUnreachable,

    /// The token returned by the auth server failed verification.
    #[error("Token verification failed after code exchange")]
    TokenVerificationFailed(#[source] TokenError),
}

// ============================================================================
// Domain 3: Token Refresh Errors
// ============================================================================

/// Errors during token refresh (s2s call to `/auth/token-refresh`).
///
/// The caller uses these to decide between 401 (access definitively gone) and
/// 503 (cannot determine — retry later). See spec §7.2.
#[derive(Debug, Error)]
pub enum RefreshError {
    /// Auth server denied refresh — user's access has been revoked.
    #[error("Access denied: user revoked")]
    AccessDenied,

    /// Token is too old for refresh (past the 5-minute grace window).
    #[error("Token expired beyond refresh grace window")]
    TokenTooOld,

    /// Support session has reached its time boundary.
    #[error("Support session expired")]
    SupportSessionExpired,

    /// `client_secret` authentication failed — configuration error.
    #[error("Daemon authentication failed (invalid_client)")]
    ConfigError,

    /// Auth server is unreachable (network error).
    #[error("Auth server unreachable during refresh")]
    AuthServerUnreachable,

    /// Auth server returned a malformed or unexpected response.
    #[error("Invalid response from auth server during refresh")]
    InvalidResponse,

    /// The refreshed token failed signature verification.
    #[error("Refreshed token verification failed")]
    TokenVerificationFailed(#[source] TokenError),
}

impl RefreshError {
    /// Whether this error means the auth server was reachable but explicitly
    /// denied the request — i.e., the user's session is definitively over.
    pub fn is_access_denied(&self) -> bool {
        matches!(
            self,
            RefreshError::AccessDenied
                | RefreshError::TokenTooOld
                | RefreshError::SupportSessionExpired
                | RefreshError::ConfigError
                | RefreshError::TokenVerificationFailed(_)
        )
    }
}

// ============================================================================
// Domain 4: Session Middleware Errors (maps to HTTP responses)
// ============================================================================

/// Errors produced by the session middleware. Each variant maps to a specific
/// HTTP response for the caller.
#[derive(Debug, Error)]
pub enum SessionError {
    /// No session cookie present.
    #[error("No active session")]
    NoSession,

    /// Session cookie present but token is invalid.
    #[error("Invalid session token")]
    InvalidToken(#[source] TokenError),

    /// Token expired and refresh was explicitly denied by the auth server.
    #[error("Session expired: refresh denied")]
    RefreshDenied,

    /// Token expired and the auth server is unreachable — cannot determine
    /// session validity.
    #[error("Auth server unavailable")]
    ServiceUnavailable,

    /// Mutating request missing the `X-Kalatori: 1` CSRF header.
    #[error("Missing CSRF header")]
    CsrfMissing,

    /// Duplicate `__Host-kalatori_session` cookies detected (possible cookie
    /// injection).
    #[error("Duplicate session cookies detected")]
    DuplicateCookie,

    #[expect(dead_code)]
    /// Insufficient role for the requested action.
    #[error("Insufficient permissions")]
    InsufficientRole,
}

impl ApiErrorExt for SessionError {
    fn category(&self) -> &str {
        match self {
            SessionError::NoSession
            | SessionError::InvalidToken(_)
            | SessionError::RefreshDenied
            | SessionError::DuplicateCookie => "AUTHENTICATION",
            SessionError::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            SessionError::CsrfMissing => "FORBIDDEN",
            SessionError::InsufficientRole => "AUTHORIZATION",
        }
    }

    fn code(&self) -> &str {
        match self {
            SessionError::NoSession => "NO_SESSION",
            SessionError::InvalidToken(_) => "INVALID_TOKEN",
            SessionError::RefreshDenied => "SESSION_EXPIRED",
            SessionError::DuplicateCookie => "DUPLICATE_COOKIE",
            SessionError::ServiceUnavailable => "AUTH_SERVER_UNAVAILABLE",
            SessionError::CsrfMissing => "CSRF_HEADER_MISSING",
            SessionError::InsufficientRole => "INSUFFICIENT_ROLE",
        }
    }

    fn message(&self) -> &str {
        match self {
            SessionError::NoSession => "Authentication required.",
            SessionError::InvalidToken(_) => "Session token is invalid.",
            SessionError::RefreshDenied => "Session has expired. Please log in again.",
            SessionError::DuplicateCookie => {
                "Duplicate session cookies detected. Session has been cleared."
            },
            SessionError::ServiceUnavailable => {
                "Authentication service is temporarily unavailable. Please retry later."
            },
            SessionError::CsrfMissing => "Missing required X-Kalatori header.",
            SessionError::InsufficientRole => "You do not have permission to perform this action.",
        }
    }

    fn http_status_code(&self) -> StatusCode {
        match self {
            SessionError::NoSession
            | SessionError::InvalidToken(_)
            | SessionError::RefreshDenied
            | SessionError::DuplicateCookie => StatusCode::UNAUTHORIZED,
            SessionError::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            SessionError::CsrfMissing | SessionError::InsufficientRole => StatusCode::FORBIDDEN,
        }
    }
}
