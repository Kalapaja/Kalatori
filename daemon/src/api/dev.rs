//! Development and testing endpoints.
//!
//! Feature-gated behind `dev_api`. These endpoints expose sensitive internals
//! and must never be used in production.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{
    IntoResponse,
    Response,
};
use axum::routing::post;
use chrono::Utc;
use serde::{
    Deserialize,
    Serialize,
};

use kalatori_client::types::ApiResultStructured;

use crate::auth::session::COOKIE_NAME;
use crate::auth::token::{
    Role,
    TokenClaims,
    sign_token,
};

use super::ApiState;
use super::utils::{
    fallback_handler,
    method_not_allowed_fallback_handler,
};

/// Shared dev state holding the ephemeral signing key.
pub struct DevAuthState {
    pub secret_key: pasetors::keys::AsymmetricSecretKey<pasetors::version4::V4>,
    pub issuer: String,
    pub audience: String,
}

pub fn routes(dev_auth: Option<Arc<DevAuthState>>) -> axum::Router<ApiState> {
    let mut router = axum::Router::new();

    if let Some(dev_auth) = dev_auth {
        let mint_routes = axum::Router::new()
            .route("/auth/mint-token", post(mint_token_handler))
            .with_state(dev_auth);

        router = router.merge(mint_routes);
    }

    router
        .fallback(fallback_handler)
        .method_not_allowed_fallback(method_not_allowed_fallback_handler)
}

// ============================================================================
// POST /dev/auth/mint-token
// ============================================================================

fn default_role() -> Role {
    Role::Owner
}

fn default_email() -> String {
    "dev@localhost".to_string()
}

fn default_sub() -> String {
    "dev-user".to_string()
}

fn default_exp_minutes() -> u64 {
    60
}

#[derive(Deserialize)]
struct MintTokenRequest {
    #[serde(default = "default_role")]
    role: Role,
    #[serde(default = "default_email")]
    email: String,
    #[serde(default = "default_sub")]
    sub: String,
    #[serde(default = "default_exp_minutes")]
    exp_minutes: u64,
}

#[derive(Serialize)]
struct MintTokenResponse {
    token: String,
    cookie_header: String,
    claims: MintedClaims,
}

#[derive(Serialize)]
struct MintedClaims {
    email: String,
    role: Role,
    sub: String,
    iss: String,
    aud: String,
    exp: String,
}

async fn mint_token_handler(
    State(dev_auth): State<Arc<DevAuthState>>,
    body: Option<axum::Json<MintTokenRequest>>,
) -> Response {
    let req = body.map_or_else(
        || MintTokenRequest {
            role: default_role(),
            email: default_email(),
            sub: default_sub(),
            exp_minutes: default_exp_minutes(),
        },
        |axum::Json(r)| r,
    );

    let now = Utc::now();
    let exp = now + chrono::Duration::minutes(i64::try_from(req.exp_minutes).unwrap_or(60));

    let claims = TokenClaims {
        iss: dev_auth.issuer.clone(),
        sub: req.sub.clone(),
        email: req.email.clone(),
        aud: dev_auth.audience.clone(),
        role: req.role,
        iat: now,
        exp,
        raw_token: String::new(),
    };

    let token = sign_token(&dev_auth.secret_key, &claims);

    let response = MintTokenResponse {
        cookie_header: format!("{COOKIE_NAME}={token}"),
        claims: MintedClaims {
            email: req.email,
            role: req.role,
            sub: req.sub,
            iss: dev_auth.issuer.clone(),
            aud: dev_auth.audience.clone(),
            exp: exp.to_rfc3339(),
        },
        token,
    };

    (
        axum::http::StatusCode::OK,
        axum::Json(ApiResultStructured::Ok {
            result: response,
        }),
    )
        .into_response()
}
