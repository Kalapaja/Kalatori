use axum::Json;
use axum::extract::{
    Query,
    State as ExtractState,
};
use axum::http::StatusCode;
use axum::response::{
    Html,
    IntoResponse,
    Response,
};
use chrono::{
    TimeDelta,
    Utc,
};
use serde::Deserialize;
use tower_http::services::ServeDir;
use uuid::Uuid;

use crate::configs::ShopMetaConfig;
use crate::dao::DaoSwapError;
use crate::state::SwapRequestError;
use crate::types::{
    CreateFrontEndSwapParams,
    CreateSwapParams,
    PublicSwap,
    SubmittedSwapParams,
    SwapSignatureParams,
};

use super::ApiState;
use super::utils::{
    ApiResult,
    AppJson,
    SuccessWrapper,
};

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Params {
    invoice_id: Uuid,
}

async fn index(ExtractState(state): ExtractState<ApiState>) -> Html<String> {
    let raw_html = include_str!("../../../static/index.html");
    let shop_meta = state.get_shop_meta();

    let html = raw_html
        .replace(
            "%VITE_MERCHANT_NAME%",
            &shop_meta.shop_name,
        )
        .replace(
            "%VITE_MERCHANT_LOGO_URL%",
            &shop_meta.logo_url.unwrap_or_default(),
        )
        .replace(
            "%VITE_REOWN_PROJECT_ID%",
            &shop_meta.reown_project_id,
        )
        .replace(
            "%VITE_PAYMENT_PAGE_TITLE%",
            &format!(
                "{} Payment | Kalatori",
                shop_meta.shop_name
            ),
        )
        .replace(
            "%VITE_ANKR_API_TOKEN%",
            &shop_meta
                .ankr_api_token
                .unwrap_or_default(),
        );

    Html(html)
}

async fn invoice(
    ExtractState(state): ExtractState<ApiState>,
    Query(payload): Query<Params>,
) -> Response {
    let invoice = state
        .get_invoice(payload.invoice_id)
        .await;

    // TODO: rename var, move value to const
    let response_if = Utc::now() - TimeDelta::days(30);

    match invoice {
        // If the invoice exists and is active, return it
        Ok(Some(invoice))
            if invoice.invoice.status.is_active() || invoice.invoice.updated_at >= response_if =>
        {
            (StatusCode::OK, Json(invoice)).into_response()
        },
        // TODO: update errors
        // If the invoice does not exist or is not active, return 404
        Ok(Some(_) | None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Invoice not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Internal server error: {}", e)})),
        )
            .into_response(),
    }
}

async fn shop_meta(ExtractState(state): ExtractState<ApiState>) -> SuccessWrapper<ShopMetaConfig> {
    state.get_shop_meta().into()
}

async fn create_front_end_swap(
    ExtractState(state): ExtractState<ApiState>,
    AppJson(data): AppJson<CreateFrontEndSwapParams>,
) -> ApiResult<CreateFrontEndSwapParams, DaoSwapError> {
    let result = state
        .create_front_end_swap(data)
        .await?;

    let response = CreateFrontEndSwapParams {
        invoice_id: result.invoice_id,
        from_amount_units: result.from_amount_units,
        from_chain_id: result.from_chain_id,
        from_asset_id: result.from_asset_id,
        transaction_hash: result.transaction_hash,
    };

    Ok(response.into())
}

async fn create_swap(
    ExtractState(state): ExtractState<ApiState>,
    AppJson(data): AppJson<CreateSwapParams>,
) -> ApiResult<PublicSwap, SwapRequestError> {
    let result = state
        .create_swap(data)
        .await?
        .into_public();

    Ok(result.into())
}

async fn update_swap_submitted(
    ExtractState(state): ExtractState<ApiState>,
    AppJson(data): AppJson<SubmittedSwapParams>,
) -> ApiResult<PublicSwap, SwapRequestError> {
    let result = state
        .update_swap_submitted(data)
        .await?
        .into_public();

    Ok(result.into())
}

async fn submit_with_signature(
    ExtractState(state): ExtractState<ApiState>,
    AppJson(data): AppJson<SwapSignatureParams>,
) -> ApiResult<PublicSwap, SwapRequestError> {
    let result = state
        .submit_swap_with_signature(data)
        .await?
        .into_public();

    Ok(result.into())
}

pub fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/", axum::routing::get(index))
        .route("/invoice", axum::routing::get(invoice))
        .route("/info", axum::routing::get(shop_meta))
        .route(
            "/swap/register",
            axum::routing::post(create_front_end_swap),
        )
        .route(
            "/swap/create",
            axum::routing::post(create_swap),
        )
        .route(
            "/swap/submitted",
            axum::routing::post(update_swap_submitted),
        )
        .route(
            "/swap/signature",
            axum::routing::post(submit_with_signature),
        )
        .nest_service(
            "/assets",
            ServeDir::new("static/assets"),
        )
}
