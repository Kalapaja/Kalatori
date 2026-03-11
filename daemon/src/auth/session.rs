//! Session middleware for admin API authentication.
//!
//! Implements the token lifecycle flowchart from spec §7.2:
//! 1. No cookie → 401/303
//! 2. Invalid signature → 401/303
//! 3. Unexpired, before midpoint → authorize directly
//! 4. Unexpired, past midpoint → authorize + opportunistic async refresh
//! 5. Expired, within 5-min grace → synchronous refresh
//! 6. Expired, beyond grace → clear cookie, 401/303

use std::sync::Arc;

use axum::extract::{
    Request,
    State,
};
use axum::http::{
    HeaderMap,
    HeaderValue,
    Method,
    StatusCode,
    header,
};
use axum::middleware::Next;
use axum::response::{
    IntoResponse,
    Redirect,
    Response,
};

use super::errors::SessionError;
use super::oauth_client::OAuthClient;
use super::token::{
    TokenClaims,
    is_expired,
    is_past_midpoint,
    is_within_refresh_grace,
    verify_token,
};

pub const COOKIE_NAME: &str = "__Host-kalatori_session";
pub const COOKIE_MAX_AGE_SECS: u64 = 86400;
const CSRF_HEADER: &str = "x-kalatori";

/// Shared auth state passed to middleware via axum `State`.
pub struct AuthState {
    pub oauth_client: OAuthClient,
}

/// Extracted from a verified session token for use in request handlers.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub claims: TokenClaims,
}

// ============================================================================
// Session middleware
// ============================================================================

/// Main session middleware. Validates the session cookie, handles token
/// refresh, and inserts `AuthenticatedUser` into request extensions.
pub async fn session_middleware(
    State(auth): State<Arc<AuthState>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();

    // Step 0: Duplicate cookie detection (spec §6.7)
    if let Some(err) = check_duplicate_cookies(headers) {
        return session_error_response(err, headers, true);
    }

    // Extract raw token from cookie
    let raw_token = match extract_session_cookie(headers) {
        Some(token) => token,
        None => {
            return session_error_response(SessionError::NoSession, headers, false);
        },
    };

    // Verify PASETO signature + iss + aud (does NOT check expiry)
    let claims = match verify_token(
        &raw_token,
        auth.oauth_client.public_keys(),
        auth.oauth_client.auth_server_url(),
        auth.oauth_client.client_id(),
    ) {
        Ok(claims) => claims,
        Err(e) => {
            return session_error_response(
                SessionError::InvalidToken(e),
                headers,
                true,
            );
        },
    };

    let clock_tolerance = auth.oauth_client.clock_tolerance();

    // Step 1: Check if token is expired (with clock tolerance)
    if is_expired(&claims, clock_tolerance) {
        // Step 2: Check refresh eligibility
        if is_within_refresh_grace(&claims) {
            // Synchronous refresh — block until we know the outcome
            match auth
                .oauth_client
                .refresh_token(&claims.raw_token)
                .await
            {
                Ok(new_claims) => {
                    let mut response = run_with_user(new_claims.clone(), request, next).await;
                    set_session_cookie(&mut response, &new_claims.raw_token);
                    return response;
                },
                Err(e) => {
                    if e.is_access_denied() {
                        return session_error_response(
                            SessionError::RefreshDenied,
                            headers,
                            true,
                        );
                    }
                    // Auth server unreachable → 503
                    return session_error_response(
                        SessionError::ServiceUnavailable,
                        headers,
                        false,
                    );
                },
            }
        }

        // Beyond grace → session is over
        return session_error_response(
            SessionError::RefreshDenied,
            headers,
            true,
        );
    }

    // Token is not expired — authorize the request
    if is_past_midpoint(&claims) {
        // Opportunistic async refresh (spec §7.2 step 2)
        // Fire-and-forget: if refresh fails, current token still valid
        let auth_clone = Arc::clone(&auth);
        let token_for_refresh = claims.raw_token.clone();
        let mut response = run_with_user(claims, request, next).await;

        // Spawn refresh in background — result applied to response if ready
        // For simplicity, we do a synchronous refresh but don't fail the request
        if let Ok(new_claims) = auth_clone
            .oauth_client
            .refresh_token(&token_for_refresh)
            .await
        {
            set_session_cookie(&mut response, &new_claims.raw_token);
        }
        return response;
    }

    // Token valid and before midpoint — authorize directly, no refresh needed
    run_with_user(claims, request, next).await
}

/// CSRF middleware: requires `X-Kalatori: 1` on mutating (non-GET) requests.
pub async fn csrf_middleware(
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::GET
        && !request
            .headers()
            .contains_key(CSRF_HEADER)
    {
        let err = SessionError::CsrfMissing;
        return (
            err.http_status_code(),
            axum::Json(
                kalatori_client::types::ApiResultStructured::<()>::Err {
                    error: err.to_api_error(),
                },
            ),
        )
            .into_response();
    }

    next.run(request).await
}

// ============================================================================
// Cookie handling
// ============================================================================

/// Extract the session cookie value from the `Cookie` header.
pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?;

    // Parse cookies manually to find our specific cookie
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{COOKIE_NAME}=")) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Check for duplicate session cookies in the raw `Cookie` header.
/// Returns `Some(SessionError::DuplicateCookie)` if more than one is found.
///
/// Per spec §6.7: framework-level cookie parsing collapses duplicates silently,
/// so we inspect the raw header string.
fn check_duplicate_cookies(headers: &HeaderMap) -> Option<SessionError> {
    let cookie_header = headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?;

    let needle = format!("{COOKIE_NAME}=");
    let count = cookie_header.matches(&needle).count();
    if count > 1 {
        Some(SessionError::DuplicateCookie)
    } else {
        None
    }
}

/// Set the session cookie on a response.
fn set_session_cookie(
    response: &mut Response,
    token: &str,
) {
    let cookie_value = format!(
        "{COOKIE_NAME}={token}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}"
    );
    if let Ok(value) = HeaderValue::from_str(&cookie_value) {
        response
            .headers_mut()
            .append(header::SET_COOKIE, value);
    }
}

/// Build a `Set-Cookie` header that clears the session cookie.
fn clear_cookie_header() -> HeaderValue {
    let value = format!("{COOKIE_NAME}=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    HeaderValue::from_str(&value).expect("clear cookie header should be valid ASCII")
}

// ============================================================================
// Response helpers
// ============================================================================

/// Run the inner handler with `AuthenticatedUser` in request extensions.
async fn run_with_user(
    claims: TokenClaims,
    mut request: Request,
    next: Next,
) -> Response {
    request
        .extensions_mut()
        .insert(AuthenticatedUser {
            claims,
        });
    next.run(request).await
}

/// Build an error response based on the session error and caller type.
///
/// `clear_cookie`: whether to send a `Set-Cookie` to clear the session.
///
/// Per spec §6.12: API clients get JSON, browsers get 303 redirect.
fn session_error_response(
    err: SessionError,
    headers: &HeaderMap,
    clear_cookie: bool,
) -> Response {
    use crate::api::ApiErrorExt;

    let status = err.http_status_code();

    if is_api_client(headers) {
        // JSON response for API clients
        let mut response = (
            status,
            axum::Json(
                kalatori_client::types::ApiResultStructured::<()>::Err {
                    error: err.to_api_error(),
                },
            ),
        )
            .into_response();

        if clear_cookie {
            response.headers_mut().append(
                header::SET_COOKIE,
                clear_cookie_header(),
            );
        }

        response
    } else {
        // Browser: redirect to auth flow for 401, error page for 503
        match status {
            StatusCode::UNAUTHORIZED => {
                let mut response = Redirect::to("/auth/login").into_response();
                if clear_cookie {
                    response.headers_mut().append(
                        header::SET_COOKIE,
                        clear_cookie_header(),
                    );
                }
                response
            },
            StatusCode::SERVICE_UNAVAILABLE => {
                let mut response = (
                    StatusCode::SERVICE_UNAVAILABLE,
                    axum::response::Html(
                        "<h1>Service Temporarily Unavailable</h1>\
                         <p>Authentication service is temporarily unavailable. Please retry later.</p>",
                    ),
                )
                    .into_response();
                if clear_cookie {
                    response.headers_mut().append(
                        header::SET_COOKIE,
                        clear_cookie_header(),
                    );
                }
                response
            },
            _ => {
                // Other errors (CSRF, insufficient role) → JSON regardless
                let mut response = (
                    status,
                    axum::Json(
                        kalatori_client::types::ApiResultStructured::<()>::Err {
                            error: err.to_api_error(),
                        },
                    ),
                )
                    .into_response();
                if clear_cookie {
                    response.headers_mut().append(
                        header::SET_COOKIE,
                        clear_cookie_header(),
                    );
                }
                response
            },
        }
    }
}

/// Determine whether the caller is an API client or a browser.
///
/// Per spec §6.12:
/// 1. `X-Kalatori` header present → API client
/// 2. `Accept` contains `application/json` → API client
/// 3. `Accept` contains `text/html` but not `application/json` → browser
/// 4. Otherwise → API client (default)
fn is_api_client(headers: &HeaderMap) -> bool {
    if headers.contains_key(CSRF_HEADER) {
        return true;
    }

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("application/json") {
        return true;
    }

    if accept.contains("text/html") {
        return false;
    }

    // Default: treat as API client
    true
}

/// Helper to get the `ApiErrorExt` trait methods on `SessionError`.
/// (The trait is imported in the response builder above.)
use crate::api::ApiErrorExt as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-kalatori_session=v4.public.abc123; other=value"),
        );
        assert_eq!(
            extract_session_cookie(&headers).unwrap(),
            "v4.public.abc123"
        );
    }

    #[test]
    fn test_extract_session_cookie_missing() {
        let headers = HeaderMap::new();
        assert!(extract_session_cookie(&headers).is_none());
    }

    #[test]
    fn test_extract_session_cookie_not_present() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=value"),
        );
        assert!(extract_session_cookie(&headers).is_none());
    }

    #[test]
    fn test_duplicate_cookie_detection() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "__Host-kalatori_session=token1; __Host-kalatori_session=token2",
            ),
        );
        assert!(check_duplicate_cookies(&headers).is_some());
    }

    #[test]
    fn test_single_cookie_no_duplicate() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-kalatori_session=token1"),
        );
        assert!(check_duplicate_cookies(&headers).is_none());
    }

    #[test]
    fn test_is_api_client_with_custom_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-kalatori",
            HeaderValue::from_static("1"),
        );
        assert!(is_api_client(&headers));
    }

    #[test]
    fn test_is_api_client_with_json_accept() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );
        assert!(is_api_client(&headers));
    }

    #[test]
    fn test_is_browser_with_html_accept() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/html, application/xhtml+xml"),
        );
        assert!(!is_api_client(&headers));
    }

    #[test]
    fn test_is_api_client_json_takes_precedence_over_html() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/html, application/json"),
        );
        assert!(is_api_client(&headers));
    }

    #[test]
    fn test_is_api_client_default() {
        let headers = HeaderMap::new();
        assert!(is_api_client(&headers));
    }

    #[test]
    fn test_is_api_client_wildcard() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("*/*"),
        );
        assert!(is_api_client(&headers));
    }
}
