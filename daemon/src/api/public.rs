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
use serde::Deserialize;
use tower_http::services::ServeDir;
use uuid::Uuid;

use crate::configs::ShopMetaConfig;
use crate::dao::DaoSwapError;
use crate::types::CreateFrontEndSwapParams;

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
    let shop_meta = state.inner.get_shop_meta();

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
            "%VITE_REWON_PROJECT_ID%",
            &shop_meta.reown_project_id,
        )
        .replace(
            "%VITE_PAYMENT_PAGE_TITLE%",
            &format!(
                "{} Payment | Kalatori",
                &shop_meta.shop_name
            ),
        );

    Html(html)
}

async fn invoice(
    ExtractState(state): ExtractState<ApiState>,
    Query(payload): Query<Params>,
) -> Response {
    let invoice = state
        .inner
        .get_invoice(payload.invoice_id)
        .await;

    match invoice {
        // If the invoice exists and is active, return it
        Ok(Some(invoice)) if invoice.invoice.status.is_active() => {
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
    state.inner.get_shop_meta().into()
}

async fn create_front_end_swap(
    ExtractState(state): ExtractState<ApiState>,
    AppJson(data): AppJson<CreateFrontEndSwapParams>,
) -> ApiResult<CreateFrontEndSwapParams, DaoSwapError> {
    let result = state
        .inner
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

pub fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/", axum::routing::get(index))
        .route("/invoice", axum::routing::get(invoice))
        .route("/info", axum::routing::get(shop_meta))
        .route(
            "/swap/register",
            axum::routing::post(create_front_end_swap),
        )
        .nest_service(
            "/assets",
            ServeDir::new("static/assets"),
        )
}
