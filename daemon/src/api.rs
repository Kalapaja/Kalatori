//! API server implementation
//!
//! API namespaces:
//! - `/public`: Publicly accessible endpoints that do not require
//!   authentication. Should return only sanitized data without sensitive
//!   information and details about the internal state.
//! - `/private`: Endpoints that require authentication and are intended for
//!   internal use. Should return only sanitized data without sensitive
//!   information and details about the internal state.
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
#[cfg(feature = "dev_api")]
mod dev;
mod internal;
mod private;
mod public;
mod utils;
mod validator;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{
    HeaderName,
    Method,
    StatusCode,
};
use tokio::net::TcpListener;
use tower_http::cors::{
    Any,
    CorsLayer,
};
use tower_http::request_id::{
    MakeRequestUuid,
    PropagateRequestIdLayer,
    SetRequestIdLayer,
};
use tower_http::trace::TraceLayer;

use kalatori_client::types::ApiError;
use kalatori_client::utils::HmacConfig;

use crate::configs::{
    ApiValidatorConfig,
    WebServerConfig,
};
use crate::state::AppState;

use validator::ApiParamsValidator;

/// State shared across all API handlers.
#[derive(Clone)]
pub struct ApiState {
    pub inner: Arc<AppState>,
    validator: Arc<ApiParamsValidator>,
}

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
    pub fn routes() -> axum::Router<super::ApiState> {
        axum::Router::new()
    }
}

pub async fn api_server(
    config: WebServerConfig,
    hmac_config: HmacConfig,
    state: AppState,
    validator_config: ApiValidatorConfig,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> impl std::future::Future<Output = ()> {
    let api_state = ApiState {
        inner: Arc::new(state),
        validator: Arc::new(ApiParamsValidator::new(
            validator_config,
        )),
    };

    let host = SocketAddr::new(config.host, config.port);

    let listener = TcpListener::bind(host)
        .await
        .expect("Failed to bind to address");

    let router = axum::Router::new()
        .nest("/dev", dev::routes())
        .nest("/internal", internal::routes())
        .nest("/private", private::routes(hmac_config))
        .nest("/public", public::routes())
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
                ))
                .layer(
                    CorsLayer::new()
                        .allow_methods([Method::GET, Method::POST])
                        .allow_origin(Any),
                ),
        )
        .with_state(api_state);

    async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(cancellation_token.cancelled_owned())
            .await
            .unwrap();
    }
}
