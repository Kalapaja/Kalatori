//! Auth endpoints for the OAuth 2.0 callback and session introspection.
//!
//! - `POST /auth/callback` — receives `form_post` from the auth server,
//!   exchanges the authorization code, sets the session cookie, and redirects
//!   to the admin UI.
//! - `GET /auth/login` — initiates the OAuth flow by redirecting to the auth
//!   server's authorize endpoint.
//! - `GET /auth/session` — returns the current session info (email, role, exp)
//!   for API clients, or 401 if unauthenticated.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{
    HeaderMap,
    HeaderValue,
    StatusCode,
    header,
};
use axum::response::{
    IntoResponse,
    Redirect,
    Response,
};
use serde::{
    Deserialize,
    Serialize,
};

use kalatori_client::types::ApiResultStructured;

use crate::auth::errors::OAuthError;
use crate::auth::session::{
    AuthState,
    COOKIE_MAX_AGE_SECS,
    COOKIE_NAME,
    extract_session_cookie,
};
use crate::utils::logging::{
    category,
    operation,
};

// ============================================================================
// POST /auth/callback
// ============================================================================

/// Form body from the auth server's `response_mode=form_post` callback.
#[derive(Deserialize)]
pub struct CallbackForm {
    code: String,
    state: String,
}

/// Handle the OAuth callback: validate origin, exchange code, set cookie,
/// redirect to admin UI.
///
/// Per spec §6.4–6.5:
/// 1. Reject non-POST (405) — handled by axum routing
/// 2. Validate `Origin` / `Referer` header against `auth_server_url`
/// 3. Extract `code` and `state` from form body
/// 4. Exchange code for PASETO token (verifies state → PKCE verifier)
/// 5. Set session cookie
/// 6. 303 redirect to `/admin?_auth=1`
pub async fn callback_handler(
    State(auth): State<Arc<AuthState>>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<CallbackForm>,
) -> Response {
    // Step 1: Validate Origin/Referer (spec §6.4)
    if let Err(err) = validate_callback_origin(
        &headers,
        auth.oauth_client.auth_server_url(),
    ) {
        tracing::warn!(
            error.category = category::AUTH,
            error.operation = operation::CODE_EXCHANGE,
            "Callback origin validation failed"
        );
        return callback_error_response(err);
    }

    // Step 2: Exchange authorization code for token
    match auth
        .oauth_client
        .exchange_code(&form.code, &form.state)
        .await
    {
        Ok(claims) => {
            tracing::info!(
                error.category = category::AUTH,
                error.operation = operation::LOGIN,
                user.sub = %claims.sub,
                user.email = %claims.email,
                user.role = ?claims.role,
                "Login successful"
            );

            // Step 3: Set session cookie and redirect to admin UI
            let mut response = Redirect::to("/admin?_auth=1").into_response();
            set_session_cookie(&mut response, &claims.raw_token);
            response
        },
        Err(e) => {
            tracing::warn!(
                error.category = category::AUTH,
                error.operation = operation::CODE_EXCHANGE,
                error.source = ?e,
                "Code exchange failed during callback"
            );
            callback_error_response(e)
        },
    }
}

// ============================================================================
// GET /auth/login
// ============================================================================

/// Initiate the OAuth flow by redirecting to the auth server's authorize
/// endpoint.
pub async fn login_handler(State(auth): State<Arc<AuthState>>) -> Response {
    let (authorize_url, _pkce) = auth.oauth_client.start_auth_flow();

    tracing::debug!(
        error.category = category::AUTH,
        error.operation = operation::LOGIN,
        "Redirecting to auth server for login"
    );

    Redirect::to(&authorize_url).into_response()
}

// ============================================================================
// GET /auth/session
// ============================================================================

/// Session info returned by `GET /auth/session`.
#[derive(Serialize)]
pub struct SessionInfo {
    pub email: String,
    pub role: String,
    pub exp: String,
}

/// Return current session information for API clients.
///
/// This endpoint does its own lightweight token verification (no session
/// middleware required). It reads the session cookie, verifies the PASETO
/// signature, and returns `{email, role, exp}` or 401.
pub async fn session_handler(
    State(auth): State<Arc<AuthState>>,
    headers: HeaderMap,
) -> Response {
    // Extract and verify token directly (lightweight — no refresh logic)
    let token = match extract_session_cookie(&headers) {
        Some(t) => t,
        None => return session_error_json(),
    };

    match crate::auth::token::verify_token(
        &token,
        auth.oauth_client.public_keys(),
        auth.oauth_client.auth_server_url(),
        auth.oauth_client.client_id(),
    ) {
        Ok(claims) => {
            let info = SessionInfo {
                email: claims.email,
                role: serde_json::to_value(claims.role)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
                exp: claims.exp.to_rfc3339(),
            };
            (
                StatusCode::OK,
                axum::Json(ApiResultStructured::Ok {
                    result: info,
                }),
            )
                .into_response()
        },
        Err(_) => session_error_json(),
    }
}

/// JSON 401 response for unauthenticated session queries.
fn session_error_json() -> Response {
    let err = crate::auth::errors::SessionError::NoSession;
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(ApiResultStructured::<()>::Err {
            error: crate::api::ApiErrorExt::to_api_error(&err),
        }),
    )
        .into_response()
}

// ============================================================================
// Helpers
// ============================================================================

/// Validate that the `Origin` or `Referer` header matches the configured
/// `auth_server_url` origin (scheme + host + port).
///
/// Per spec §6.4: The auth server sends the callback as a POST with
/// `response_mode=form_post`, so the browser includes an `Origin` header.
/// Fall back to `Referer` for compatibility.
fn validate_callback_origin(
    headers: &HeaderMap,
    expected_auth_server_url: &str,
) -> Result<(), OAuthError> {
    let expected_origin = extract_origin(expected_auth_server_url);

    // Try Origin header first, then Referer
    let actual_origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|o| o.trim_end_matches('/').to_string())
        .or_else(|| {
            headers
                .get(header::REFERER)
                .and_then(|v| v.to_str().ok())
                .and_then(extract_origin_from_url)
        });

    match actual_origin {
        Some(origin) if origin == expected_origin => Ok(()),
        Some(_) => Err(OAuthError::InvalidOrigin),
        // No Origin or Referer header — reject per spec
        None => Err(OAuthError::InvalidOrigin),
    }
}

/// Extract origin (scheme + host + port) from a URL string.
///
/// e.g. `"https://app.kalatori.org/some/path"` → `"https://app.kalatori.org"`
fn extract_origin(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| {
            let scheme = u.scheme();
            let host = u.host_str().unwrap_or("");
            match u.port() {
                Some(port) => format!("{scheme}://{host}:{port}"),
                None => format!("{scheme}://{host}"),
            }
        })
        .unwrap_or_default()
}

/// Extract origin from a full URL (used for Referer header parsing).
fn extract_origin_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let scheme = parsed.scheme();
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Set the session cookie on a response.
fn set_session_cookie(
    response: &mut Response,
    token: &str,
) {
    let cookie_value = format!(
        "{COOKIE_NAME}={token}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}"
    );
    if let Ok(value) = HeaderValue::from_str(&cookie_value) {
        response
            .headers_mut()
            .append(header::SET_COOKIE, value);
    }
}

/// Build an error response for the callback endpoint.
///
/// Since the callback is initiated by a browser redirect, errors are shown
/// as simple HTML pages (not JSON).
fn callback_error_response(err: OAuthError) -> Response {
    match err {
        OAuthError::InvalidOrigin => (
            StatusCode::FORBIDDEN,
            axum::response::Html(
                "<h1>Forbidden</h1>\
                 <p>Origin validation failed. This request did not come from the expected auth server.</p>",
            ),
        )
            .into_response(),
        OAuthError::AuthServerUnreachable => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::response::Html(
                "<h1>Service Unavailable</h1>\
                 <p>The authentication server is temporarily unreachable. Please try again later.</p>",
            ),
        )
            .into_response(),
        _ => (
            StatusCode::UNAUTHORIZED,
            axum::response::Html(
                "<h1>Authentication Failed</h1>\
                 <p>Unable to complete login. Please <a href=\"/auth/login\">try again</a>.</p>",
            ),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_origin_https() {
        assert_eq!(
            extract_origin("https://app.kalatori.org/oauth/authorize?foo=bar"),
            "https://app.kalatori.org"
        );
    }

    #[test]
    fn test_extract_origin_with_port() {
        assert_eq!(
            extract_origin("https://localhost:8443/path"),
            "https://localhost:8443"
        );
    }

    #[test]
    fn test_extract_origin_http_default_port() {
        assert_eq!(
            extract_origin("http://example.com:80/path"),
            "http://example.com"
        );
    }

    #[test]
    fn test_validate_origin_matches() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.kalatori.org"),
        );
        assert!(validate_callback_origin(&headers, "https://app.kalatori.org").is_ok());
    }

    #[test]
    fn test_validate_origin_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example.com"),
        );
        assert!(validate_callback_origin(&headers, "https://app.kalatori.org").is_err());
    }

    #[test]
    fn test_validate_origin_missing() {
        let headers = HeaderMap::new();
        assert!(validate_callback_origin(&headers, "https://app.kalatori.org").is_err());
    }

    #[test]
    fn test_validate_origin_with_trailing_slash() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.kalatori.org/"),
        );
        assert!(validate_callback_origin(&headers, "https://app.kalatori.org").is_ok());
    }

    #[test]
    fn test_validate_referer_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://app.kalatori.org/oauth/authorize?state=abc"),
        );
        assert!(validate_callback_origin(&headers, "https://app.kalatori.org").is_ok());
    }

    #[test]
    fn test_validate_referer_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://evil.example.com/page"),
        );
        assert!(validate_callback_origin(&headers, "https://app.kalatori.org").is_err());
    }
}
