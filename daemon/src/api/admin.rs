//! Admin namespace — protected by session middleware + CSRF middleware when
//! auth is enabled.

use axum::extract::{
    Request,
    State,
};
use axum::http::StatusCode;
use axum::response::{
    IntoResponse,
    Response,
};
use axum::routing::get;
use axum::{
    Extension,
    Router,
    middleware,
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

use crate::api::utils::ErrorWrapper;
use crate::auth::errors::SessionError;
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
    ShopPlatform,
};

use super::ApiState;
use super::utils::{
    ApiResult,
    AppQuery,
    SuccessWrapper,
};

const INTEGRATION_SETTINGS_PATH: &str = "/integration-settings";
const GET_PLUGIN_PATH: &str = "/get-plugin";

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

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct ShopPlatformParam {
    shop_platform: ShopPlatform,
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
    picture: Option<String>,
    role: Role,
    sub: String,
    exp: String,
}

async fn whoami_handler(Extension(user): Extension<AuthenticatedUser>) -> Response {
    let response = WhoamiResponse {
        email: user.claims.email,
        picture: user.claims.picture,
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
    state
        .get_kalatori_integration_settings()
        .into()
}

async fn require_owner(
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
    next: middleware::Next,
) -> Result<Response, ErrorWrapper<SessionError>> {
    if user.claims.role != Role::Owner {
        return Err(SessionError::InsufficientRole.into());
    }

    Ok(next.run(request).await)
}

#[tracing::instrument(skip_all)]
async fn get_plugin_handler(
    State(state): State<ApiState>,
    AppQuery(param): AppQuery<ShopPlatformParam>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> Response {
    let platform = param.shop_platform;
    let result = state.get_shop_plugin(platform).await;

    match result {
        Ok(plugin_bytes) => {
            let filename = platform.plugin_asset_name();
            let content_length = plugin_bytes.len().to_string();
            (
                StatusCode::OK,
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        "application/zip".to_owned(),
                    ),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        format!(r#"attachment; filename="{filename}""#),
                    ),
                    (
                        axum::http::header::CONTENT_LENGTH,
                        content_length,
                    ),
                ],
                plugin_bytes,
            )
                .into_response()
        },
        Err(error) => ErrorWrapper::from(error).into_response(),
    }
}

/// Admin routes.
pub fn routes() -> Router<ApiState> {
    let owner_routes = Router::new()
        .route(
            INTEGRATION_SETTINGS_PATH,
            get(kalatori_integration_settings_handler),
        )
        .route(GET_PLUGIN_PATH, get(get_plugin_handler))
        .route_layer(middleware::from_fn(require_owner));

    let api_routes = Router::new()
        .route("/whoami", get(whoami_handler))
        .route(
            "/invoice/list",
            get(list_invoices_handler),
        )
        .route("/invoice/get", get(get_invoice_handler))
        .route(
            "/payout/list",
            get(list_payouts_handler),
        )
        .route("/payout/get", get(get_payout_handler))
        // Payout initiation is intentionally out of service: it uses a hardcoded
        // amount, and the correct amount semantics are deferred to follow-up work.
        .route(
            "/transaction/list",
            get(list_transactions_handler),
        )
        .route(
            "/transaction/get",
            get(get_transaction_handler),
        )
        .route("/swap/list", get(list_swaps_handler))
        .route("/swap/get", get(get_swap_handler))
        .route(
            "/settings",
            get(kalatori_settings_handler),
        )
        .merge(owner_routes)
        // A nested router inherits the outer fallback unless it sets its own,
        // so without this an unknown `/admin/api/*` path — `payout/initiate`
        // included — would be answered with the admin SPA's index.html and a
        // 200. An API path that does not exist must say so.
        .fallback(|| async { StatusCode::NOT_FOUND });

    Router::new()
        .nest("/api", api_routes)
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

#[cfg(test)]
mod tests {
    use axum::body::{
        Body,
        to_bytes,
    };
    use axum::http::Request;
    use chrono::{
        Duration,
        Utc,
    };
    use tower::ServiceExt;

    use crate::auth::token::TokenClaims;

    use super::*;

    const VIEWER_PATHS: &[&str] = &[
        "/whoami",
        "/invoice/list",
        "/invoice/get",
        "/payout/list",
        "/payout/get",
        "/transaction/list",
        "/transaction/get",
        "/swap/list",
        "/swap/get",
        "/settings",
    ];

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "fixed one-hour offset from `Utc::now()`, which is ~250_000 years from `DateTime`'s range limit"
    )]
    fn user(role: Role) -> AuthenticatedUser {
        let now = Utc::now();
        AuthenticatedUser {
            claims: TokenClaims {
                iss: "https://auth.example.com".to_string(),
                sub: "user".to_string(),
                email: "user@example.com".to_string(),
                picture: None,
                aud: "kalatori".to_string(),
                role,
                iat: now,
                exp: now + Duration::hours(1),
                raw_token: "token".to_string(),
            },
        }
    }

    fn authorization_test_router() -> Router {
        let owner_routes = Router::new()
            .route(
                INTEGRATION_SETTINGS_PATH,
                get(|| async { StatusCode::OK }),
            )
            .route(
                GET_PLUGIN_PATH,
                get(|| async { StatusCode::OK }),
            )
            .route_layer(middleware::from_fn(require_owner));

        // Built the way production builds it — viewer routes first, then
        // `.merge(owner_routes)` — rather than folding them onto the
        // already-layered owner router. Otherwise the harness would depend on
        // `route_layer` not reaching routes added after it, and could start
        // gating viewer paths, or stop gating owner ones, on an axum upgrade.
        let api_routes = VIEWER_PATHS
            .iter()
            .fold(Router::new(), |router, path| {
                router.route(path, get(|| async { StatusCode::OK }))
            })
            .merge(owner_routes)
            .fallback(|| async { StatusCode::NOT_FOUND });

        // Stands in for the SPA fallback the real router carries, but with a
        // status the API never returns. `ServeFile` would be useless here:
        // `static/admin/` is a build artefact absent from a source checkout,
        // so it 404s too and the assertion below could not tell a real 404
        // from the fallback swallowing the request.
        Router::new()
            .nest("/api", api_routes)
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
    }

    async fn request(
        path: &str,
        role: Role,
    ) -> Response {
        authorization_test_router()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .extension(user(role))
                    .body(Body::empty())
                    .expect("request uses a valid URI"),
            )
            .await
            .expect("router always responds")
    }

    #[tokio::test]
    async fn secret_routes_are_owner_only() {
        for path in [INTEGRATION_SETTINGS_PATH, GET_PLUGIN_PATH] {
            let path = format!("/api{path}");
            assert_eq!(
                request(&path, Role::Owner)
                    .await
                    .status(),
                StatusCode::OK
            );

            for role in [Role::Operator, Role::Viewer, Role::Support] {
                let response = request(&path, role).await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                let body = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("response body collects");
                let json: serde_json::Value =
                    serde_json::from_slice(&body).expect("error response is JSON");
                assert_eq!(
                    json["error"]["code"],
                    "INSUFFICIENT_ROLE"
                );
            }
        }
    }

    #[tokio::test]
    async fn every_authenticated_role_can_reach_non_gated_admin_api_routes() {
        for path in VIEWER_PATHS {
            let path = format!("/api{path}");
            for role in [Role::Owner, Role::Operator, Role::Viewer, Role::Support] {
                assert_eq!(
                    request(&path, role).await.status(),
                    StatusCode::OK,
                    "{role:?} should retain access to {path}"
                );
            }
        }
    }

    #[tokio::test]
    async fn payout_initiate_route_is_absent() {
        assert_eq!(
            request("/api/payout/initiate", Role::Owner)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}
