//! Admin namespace — protected by session middleware + CSRF middleware when
//! auth is enabled.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{
    IntoResponse,
    Response,
};
use axum::routing::{
    get,
    post,
};
use axum::{
    Extension,
    Router,
};
use serde::{
    Deserialize,
    Serialize,
};
use tower_http::services::{
    ServeDir,
    ServeFile,
};
use uuid::Uuid;

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
    KalatoriIntegrationSettings,
    KalatoriSettings,
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
    AppJson,
    AppQuery,
    SuccessWrapper,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InvoiceIdParam {
    invoice_id: Uuid,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct PayoutIdParam {
    payout_id: Uuid,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct TransactionIdParam {
    transaction_id: Uuid,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct SwapIdParam {
    swap_id: Uuid,
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
// GET /admin/invoices/{id}
// ============================================================================

#[tracing::instrument(skip_all)]
async fn get_invoice_handler(
    State(state): State<ApiState>,
    AppQuery(param): AppQuery<InvoiceIdParam>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PublicInvoice, DaoInvoiceError> {
    let invoice_id = param.invoice_id;

    let invoice = state
        .get_invoice(invoice_id)
        .await?
        .ok_or(DaoInvoiceError::NotFound {
            invoice_id,
        })?;

    let result = state.invoice_to_public_invoice(invoice);
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
// GET /admin/payouts/{id}
// ============================================================================

#[tracing::instrument(skip_all)]
async fn get_payout_handler(
    State(state): State<ApiState>,
    AppQuery(param): AppQuery<PayoutIdParam>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<Payout, DaoPayoutError> {
    let payout_id = param.payout_id;

    let payout = state
        .get_payout(payout_id)
        .await?
        .ok_or(DaoPayoutError::NotFound {
            payout_id,
        })?;

    Ok(payout.into())
}

// ============================================================================
// POST /admin/payouts/initiate
// ============================================================================

#[tracing::instrument(skip_all)]
async fn initiate_payout_handler(
    State(state): State<ApiState>,
    Extension(_user): Extension<AuthenticatedUser>,
    AppJson(param): AppJson<InvoiceIdParam>,
) -> ApiResult<Payout, DaoInvoiceError> {
    let invoice_id = param.invoice_id;

    let payout = state
        .initiate_payout(invoice_id)
        .await?;

    Ok(payout.into())
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
// GET /admin/transactions/{id}
// ============================================================================

#[tracing::instrument(skip_all)]
async fn get_transaction_handler(
    State(state): State<ApiState>,
    AppQuery(param): AppQuery<TransactionIdParam>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PublicTransaction, DaoTransactionError> {
    let transaction_id = param.transaction_id;

    let transaction = state
        .get_transaction(transaction_id)
        .await?
        .ok_or(DaoTransactionError::NotFound {
            transaction_id,
        })?;

    Ok(PublicTransaction::from(transaction).into())
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
// GET /admin/swaps/{id}
// ============================================================================

#[tracing::instrument(skip_all)]
async fn get_swap_handler(
    State(state): State<ApiState>,
    AppQuery(param): AppQuery<SwapIdParam>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> ApiResult<PublicSwap, DaoSwapError> {
    let swap_id = param.swap_id;

    let swap = state
        .get_swap(swap_id)
        .await?
        .ok_or(DaoSwapError::NotFound {
            swap_id,
        })?;

    Ok(PublicSwap::from(swap).into())
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

async fn kalatori_settings_handler(
    State(state): State<ApiState>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> SuccessWrapper<KalatoriSettings> {
    state.get_kalatori_settings().into()
}

async fn kalatori_integration_settings_handler(
    State(state): State<ApiState>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> SuccessWrapper<KalatoriIntegrationSettings> {
    // TODO: restrict visibility by user's role
    state
        .get_kalatori_integration_settings()
        .into()
}

/// Admin routes.
pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/whoami", get(whoami_handler))
        .route(
            "/api/invoice/list",
            get(list_invoices_handler),
        )
        .route(
            "/api/invoice/get",
            get(get_invoice_handler),
        )
        .route(
            "/api/payout/list",
            get(list_payouts_handler),
        )
        .route(
            "/api/payout/get",
            get(get_payout_handler),
        )
        .route(
            "/api/payout/initiate",
            post(initiate_payout_handler),
        )
        .route(
            "/api/transaction/list",
            get(list_transactions_handler),
        )
        .route(
            "/api/transaction/get",
            get(get_transaction_handler),
        )
        .route(
            "/api/swap/list",
            get(list_swaps_handler),
        )
        .route("/api/swap/get", get(get_swap_handler))
        .route(
            "/api/settings",
            get(kalatori_settings_handler),
        )
        .route(
            "/api/integration-settings",
            get(kalatori_integration_settings_handler),
        )
        .route_service(
            "/",
            ServeFile::new("static/admin/index.html"),
        )
        .fallback_service(ServeFile::new(
            "static/admin/index.html",
        ))
        .nest_service(
            "/assets",
            ServeDir::new("static/admin/assets"),
        )
}
