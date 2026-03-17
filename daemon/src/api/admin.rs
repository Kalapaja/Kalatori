//! Admin namespace — protected by session middleware + CSRF middleware when
//! auth is enabled.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{
    IntoResponse,
    Response,
};
use axum::routing::get;
use axum::{
    Extension,
    Router,
};
use serde::Serialize;

use kalatori_client::types::ApiResultStructured;

use crate::auth::session::AuthenticatedUser;
use crate::auth::token::Role;
use crate::dao::{
    DaoInvoiceError,
    DaoPayoutError,
    DaoSwapError,
    DaoTransactionError,
};
use crate::types::{
    ListInvoicesParams,
    ListPayoutsParams,
    ListSwapsParams,
    ListTransactionsParams,
    PaginatedResponse,
    Payout,
    PublicInvoice,
    PublicSwap,
    PublicTransaction,
};

use super::ApiState;
use super::utils::{
    ApiResult,
    AppQuery,
};

/// Admin routes.
pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/whoami", get(whoami_handler))
        .route("/invoices", get(list_invoices_handler))
        .route("/payouts", get(list_payouts_handler))
        .route(
            "/transactions",
            get(list_transactions_handler),
        )
        .route("/swaps", get(list_swaps_handler))
}

// ============================================================================
// GET /admin/invoices
// ============================================================================

#[tracing::instrument(skip_all)]
async fn list_invoices_handler(
    State(state): State<ApiState>,
    AppQuery(params): AppQuery<ListInvoicesParams>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PaginatedResponse<PublicInvoice>, DaoInvoiceError> {
    let result = state.list_invoices(&params).await?;
    Ok(result.into())
}

// ============================================================================
// GET /admin/payouts
// ============================================================================

#[tracing::instrument(skip_all)]
async fn list_payouts_handler(
    State(state): State<ApiState>,
    AppQuery(params): AppQuery<ListPayoutsParams>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PaginatedResponse<Payout>, DaoPayoutError> {
    let result = state.list_payouts(&params).await?;
    Ok(result.into())
}

// ============================================================================
// GET /admin/transactions
// ============================================================================

#[tracing::instrument(skip_all)]
async fn list_transactions_handler(
    State(state): State<ApiState>,
    AppQuery(params): AppQuery<ListTransactionsParams>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PaginatedResponse<PublicTransaction>, DaoTransactionError> {
    let result = state.list_transactions(&params).await?;
    Ok(result.into())
}

// ============================================================================
// GET /admin/swaps
// ============================================================================

#[tracing::instrument(skip_all)]
async fn list_swaps_handler(
    State(state): State<ApiState>,
    AppQuery(params): AppQuery<ListSwapsParams>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PaginatedResponse<PublicSwap>, DaoSwapError> {
    let result = state.list_swaps(&params).await?;
    Ok(result.into())
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
