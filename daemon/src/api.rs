//! API server implementation
//!
//! API namespaces:
//! - `/public`: Publicly accessible endpoints that do not require
//!   authentication. Should return only sanitized data without sensitive
//!   information and details about the internal state.
//! - `/private`: Endpoints that require authentication and are intended for
//!   internal use. Should return only sanitized data without sensitive
//!   information and details about the internal state.
//! - `/admin`: Admin UI endpoints, protected by OAuth session middleware +
//!   CSRF. Only mounted when auth config is present.
//! - `/auth`: OAuth callback and session introspection endpoints. Only mounted
//!   when auth config is present.
//! - `/dev`: Development and testing endpoints. May include endpoints that are
//!   not intended for production use. Allowed to return raw data including
//!   sensitive information and internal state details for debugging purposes.
//!   Should not be exposed in production environments.
//!
//! Error handling principles:
//! - For invalid or malformed JSON, query parameters, or request structure,
//!   return structured JSON error response.
//! - For authentication errors, return structured JSON error response.
//! - For application-level errors (e.g., entity not found, validation errors),
//!   return structured JSON error response.
//! - For unexpected server errors, return structured JSON error response with a
//!   generic message.
//! - For invalid routes or methods under `/private` and `/dev` namespaces,
//!   return structured JSON error response, while `/public` namespace returns
//!   standard 404 HTML response.
mod admin;
pub mod auth_endpoints;
#[cfg(feature = "dev_api")]
mod dev;
mod internal;
mod private;
mod public;
mod utils;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{
    HeaderName,
    StatusCode,
};
use axum::routing::{
    get,
    post,
};
use axum::{
    ServiceExt,
    middleware,
};
use tokio::net::TcpListener;
use tower::{
    Layer,
    ServiceExt as _,
};
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::request_id::{
    MakeRequestUuid,
    PropagateRequestIdLayer,
    SetRequestIdLayer,
};
use tower_http::trace::TraceLayer;

use kalatori_client::types::ApiError;
use kalatori_client::utils::HmacConfig;

use crate::auth::oauth_client::OAuthClient;
use crate::auth::session::{
    AuthState,
    csrf_middleware,
    session_middleware,
};
use crate::configs::{
    OAuthConfig,
    WebServerConfig,
};
use crate::state::AppState;

pub type ApiState = Arc<AppState>;

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

pub trait ApiErrorExt: std::error::Error {
    fn category(&self) -> &str;
    fn code(&self) -> &str;
    fn message(&self) -> &str;
    fn http_status_code(&self) -> StatusCode;

    fn to_api_error(&self) -> ApiError {
        ApiError {
            category: self.category().to_string(),
            code: self.code().to_string(),
            message: self.message().to_string(),
            details: None,
        }
    }
}

#[cfg(not(feature = "dev_api"))]
mod dev {
    pub fn routes(_dev_auth: Option<std::sync::Arc<()>>) -> axum::Router<super::ApiState> {
        axum::Router::new()
    }
}

pub async fn api_server(
    config: WebServerConfig,
    hmac_config: HmacConfig,
    auth_config: Option<OAuthConfig>,
    state: AppState,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> impl std::future::Future<Output = ()> {
    let api_state = Arc::new(state);

    let host = SocketAddr::new(config.host, config.port);

    let listener = TcpListener::bind(host)
        .await
        .expect("Failed to bind to address");

    // Resolve auth config — in dev mode, auto-generate if absent
    #[cfg(feature = "dev_api")]
    let (auth_config, dev_auth_state) = resolve_dev_auth(auth_config);

    #[cfg(not(feature = "dev_api"))]
    let dev_auth_state: Option<Arc<()>> = None;

    let mut router = axum::Router::new()
        .nest("/dev", dev::routes(dev_auth_state))
        .nest("/internal", internal::routes())
        .nest("/private", private::routes(hmac_config))
        .nest("/public", public::routes());

    // Conditionally mount auth and admin routes when auth is configured
    if let Some(oauth_config) = auth_config {
        let auth_state = Arc::new(AuthState {
            oauth_client: OAuthClient::new(oauth_config),
        });

        // /auth/* routes — no session middleware (these ARE the login flow)
        let auth_routes = axum::Router::new()
            .route(
                "/login",
                get(auth_endpoints::login_handler),
            )
            .route(
                "/callback",
                post(auth_endpoints::callback_handler),
            )
            .route(
                "/session",
                get(auth_endpoints::session_handler),
            )
            .route(
                "/logout",
                post(auth_endpoints::logout_handler),
            )
            .with_state(Arc::clone(&auth_state));

        // /admin routes — protected by session + CSRF middleware
        // Session middleware runs first (outermost layer), then CSRF.
        let admin_routes = admin::routes()
            .layer(middleware::from_fn(csrf_middleware))
            .layer(middleware::from_fn_with_state(
                Arc::clone(&auth_state),
                session_middleware,
            ));

        router = router
            .nest("/auth", auth_routes)
            .nest("/admin", admin_routes);

        tracing::info!("OAuth authentication enabled — /auth and /admin routes mounted");
    } else {
        tracing::info!("OAuth authentication disabled — /auth and /admin routes not mounted");
    }

    let router = router
        .layer(
            tower::ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(
                    REQUEST_ID_HEADER,
                    MakeRequestUuid,
                ))
                .layer(
                    TraceLayer::new_for_http().make_span_with(
                        |request: &axum::http::Request<_>| {
                            let request_id = request
                                .headers()
                                .get(REQUEST_ID_HEADER)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("-");

                            tracing::info_span!(
                                "HTTP Request",
                                method = %request.method(),
                                path = %request.uri().path(),
                                request_id = %request_id,
                            )
                        },
                    ),
                )
                .layer(PropagateRequestIdLayer::new(
                    REQUEST_ID_HEADER,
                )),
        )
        .with_state(api_state);

    let app = NormalizePathLayer::trim_trailing_slash()
        .layer(router)
        .map_request(|req| req);

    async move {
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(cancellation_token.cancelled_owned())
            .await
            .unwrap();
    }
}

/// When `dev_api` is enabled and no auth config is provided, generate an
/// ephemeral keypair and construct a synthetic auth config. This enables local
/// development of admin endpoints without needing a real auth server.
#[cfg(feature = "dev_api")]
fn resolve_dev_auth(
    auth_config: Option<OAuthConfig>
) -> (
    Option<OAuthConfig>,
    Option<Arc<dev::DevAuthState>>,
) {
    use secrecy::SecretString;

    use crate::auth::token::generate_dev_keypair;

    if auth_config.is_some() {
        return (auth_config, None);
    }

    let (kp, paserk_public) = generate_dev_keypair();

    let issuer = "http://localhost:dev".to_string();
    let client_id = "dev".to_string();

    let synthetic_config = OAuthConfig {
        auth_server_url: issuer.clone(),
        client_id: client_id.clone(),
        client_secret: SecretString::from("dev-secret-not-used".to_string()),
        previous_client_secret: None,
        token_public_keys: vec![paserk_public.clone()],
        clock_tolerance: 30,
        base_url: "http://localhost:8080".to_string(),
    };

    let dev_auth = Arc::new(dev::DevAuthState {
        secret_key: kp.secret,
        issuer,
        audience: client_id,
    });

    tracing::info!(
        public_key = %paserk_public,
        "Dev mode: auto-generated ephemeral auth keypair. \
         Use POST /dev/auth/mint-token to create session tokens."
    );

    (Some(synthetic_config), Some(dev_auth))
}
