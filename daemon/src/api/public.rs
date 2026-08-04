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

use std::sync::Arc;

use crate::dao::DaoInterface;
use crate::state::AppState;

use super::utils::{
    ApiResult,
    AppJson,
    SuccessWrapper,
};

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Params {
    invoice_id: Uuid,
}

async fn index<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>
) -> Html<String> {
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

async fn invoice<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>,
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

async fn shop_meta<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>
) -> SuccessWrapper<ShopMetaConfig> {
    state.get_shop_meta().into()
}

async fn create_front_end_swap<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>,
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

async fn create_swap<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>,
    AppJson(data): AppJson<CreateSwapParams>,
) -> ApiResult<PublicSwap, SwapRequestError> {
    let result = state
        .create_swap(data)
        .await?
        .into_public();

    Ok(result.into())
}

async fn update_swap_submitted<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>,
    AppJson(data): AppJson<SubmittedSwapParams>,
) -> ApiResult<PublicSwap, SwapRequestError> {
    let result = state
        .update_swap_submitted(data)
        .await?
        .into_public();

    Ok(result.into())
}

async fn submit_with_signature<D: DaoInterface + 'static>(
    ExtractState(state): ExtractState<Arc<AppState<D>>>,
    AppJson(data): AppJson<SwapSignatureParams>,
) -> ApiResult<PublicSwap, SwapRequestError> {
    let result = state
        .submit_swap_with_signature(data)
        .await?
        .into_public();

    Ok(result.into())
}

pub fn routes<D: DaoInterface + 'static>() -> axum::Router<Arc<AppState<D>>> {
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::Body;
    use axum::http::{
        Request,
        StatusCode,
    };
    use secrecy::SecretString;
    use tower::ServiceExt as _;

    use crate::chain::InvoiceRegistry;
    use crate::chain_client::KeyringClient;
    use crate::configs::{
        PaymentsConfig,
        ShopConfig,
        ShopMetaConfig,
    };
    use crate::dao::MockDaoInterface;
    use crate::swaps::{
        SwapsExecutor,
        SwapsExecutorError,
    };
    use crate::types::{
        ChainType,
        DetectedShopPlatform,
    };

    use super::*;

    const POLYGON_USDC: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";

    fn app_state(
        swaps_executor: SwapsExecutor<MockDaoInterface>
    ) -> Arc<AppState<MockDaoInterface>> {
        let payments_config = PaymentsConfig {
            default_chain: ChainType::Polygon,
            default_asset_id: HashMap::from([(
                ChainType::Polygon,
                POLYGON_USDC.to_string(),
            )]),
            invoice_lifetime_millis: 600_000,
            recipient: HashMap::from([(
                ChainType::Polygon,
                "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7".to_string(),
            )]),
            payment_url_base: "https://payments.example.com".to_string(),
            slippage_params: HashMap::new(),
        };

        let shop_config = ShopConfig {
            invoices_webhook_url: None,
            signature_max_age_secs: 300,
            private_api_base_url: None,
            meta: ShopMetaConfig {
                shop_name: "Mega shop".to_string(),
                shop_url: "mega.shop".to_string(),
                logo_url: None,
                reown_project_id: "test".to_string(),
                ankr_api_token: None,
            },
            shop_platform: DetectedShopPlatform::Unknown,
        };

        Arc::new(AppState::new(
            KeyringClient::default(),
            MockDaoInterface::default(),
            InvoiceRegistry::new(),
            swaps_executor,
            HashMap::new(),
            HashMap::new(),
            payments_config,
            shop_config,
            SecretString::from("secret"),
        ))
    }

    /// Drives the real unauthenticated route, not just the layer beneath it.
    ///
    /// [#349](https://github.com/Kalapaja/Kalatori/issues/349) was a payer-supplied
    /// signature reaching `split_once("|").unwrap()` on this endpoint, which
    /// the panic hook turns into a daemon shutdown. The response has to be
    /// a 4xx carrying a code the caller can act on — and getting *any*
    /// response at all is the property under test, since the failure mode
    /// was no response ever again, from any endpoint.
    #[tokio::test]
    async fn a_malformed_signature_gets_a_4xx_from_the_public_route() {
        let mut swaps_executor = SwapsExecutor::default();
        swaps_executor
            .expect_submit_with_signature()
            .once()
            .returning(|_| Err(SwapsExecutorError::InvalidSignature));

        let response = routes()
            .with_state(app_state(swaps_executor))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/swap/signature")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "swap_id": "550e8400-e29b-41d4-a716-446655440000",
                            "swap_executor": "ZeroExGasless",
                            // One signature for a quote that needs two: the
                            // exact payload from the report.
                            "signature": "0xdeadbeef",
                        })
                        .to_string(),
                    ))
                    .expect("request builds from static parts"),
            )
            .await
            .expect("the router always answers");

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body collects");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("the error body is JSON");

        assert_eq!(
            json["error"]["code"], "INVALID_SWAP_SIGNATURE",
            "the caller needs a code it can act on, got {json}"
        );
    }

    /// The loser of a concurrent submission gets a 409, not a 400: it is not
    /// the caller's payload that is wrong, the swap is simply already in
    /// flight.
    #[tokio::test]
    async fn an_already_claimed_swap_gets_a_409_from_the_public_route() {
        let mut swaps_executor = SwapsExecutor::default();
        swaps_executor
            .expect_submit_with_signature()
            .once()
            .returning(|_| {
                Err(SwapsExecutorError::SwapAlreadyClaimed {
                    current_status: crate::types::SwapStatus::Submitted,
                })
            });

        let response = routes()
            .with_state(app_state(swaps_executor))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/swap/signature")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "swap_id": "550e8400-e29b-41d4-a716-446655440000",
                            "swap_executor": "ZeroExGasless",
                            "signature": "0xdead|0xbeef",
                        })
                        .to_string(),
                    ))
                    .expect("request builds from static parts"),
            )
            .await
            .expect("the router always answers");

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
}
