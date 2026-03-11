//! Admin namespace — protected by session middleware + CSRF middleware when
//! auth is enabled.

use axum::http::StatusCode;
use axum::response::{
    IntoResponse,
    Response,
};
use axum::routing::get;
use axum::Extension;
use axum::Router;
use serde::Serialize;

use kalatori_client::types::ApiResultStructured;

use crate::auth::session::AuthenticatedUser;
use crate::auth::token::Role;

use super::ApiState;

/// Admin routes.
pub fn routes() -> Router<ApiState> {
    Router::new().route("/whoami", get(whoami_handler))
}

// ============================================================================
// GET /admin/whoami
// ============================================================================

#[derive(Serialize)]
struct WhoamiResponse {
    email: String,
    role: Role,
    sub: String,
    exp: String,
}

async fn whoami_handler(Extension(user): Extension<AuthenticatedUser>) -> Response {
    let response = WhoamiResponse {
        email: user.claims.email,
        role: user.claims.role,
        sub: user.claims.sub,
        exp: user.claims.exp.to_rfc3339(),
    };

    (
        StatusCode::OK,
        axum::Json(ApiResultStructured::Ok {
            result: response,
        }),
    )
        .into_response()
}
