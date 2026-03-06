use axum::extract::State;
use axum::routing::{
    get,
    post,
};

use kalatori_client::middleware::axum_hmac_validator;
use kalatori_client::types::{
    CancelInvoiceParams,
    CreateInvoiceParams,
    GetInvoiceParams,
    UpdateInvoiceParams,
};
use kalatori_client::utils::HmacConfig;

use crate::dao::DaoInvoiceError;
use crate::types::{
    InvoiceStatus,
    PublicInvoice,
    TransactionType,
};

use super::ApiState;
use super::utils::{
    ApiResult,
    AppJson,
    AppQuery,
    fallback_handler,
    method_not_allowed_fallback_handler,
};

#[tracing::instrument(skip_all)]
async fn create_invoice(
    State(api_state): State<ApiState>,
    AppJson(params): AppJson<CreateInvoiceParams>,
) -> ApiResult<PublicInvoice, DaoInvoiceError> {
    api_state
        .validator
        .validate_create_invoice_params(&params)
        .await?;

    let invoice = api_state
        .inner
        .create_invoice(params)
        .await?;

    let result = api_state
        .inner
        .invoice_to_public_invoice(invoice);
    Ok(result.into())
}

#[tracing::instrument(skip_all)]
async fn get_invoice(
    State(state): State<ApiState>,
    AppQuery(params): AppQuery<GetInvoiceParams>,
) -> ApiResult<PublicInvoice, DaoInvoiceError> {
    let invoice = state
        .inner
        .get_invoice(params.invoice_id)
        .await?
        .ok_or(DaoInvoiceError::NotFound {
            invoice_id: params.invoice_id,
        })?;

    let mut result = state
        .inner
        .invoice_to_public_invoice(invoice);

    // TODO: filter it on database query level
    if params.include_transactions && result.status == InvoiceStatus::Waiting {
        let transactions = state
            .inner
            .get_invoice_transactions(params.invoice_id)
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?
            .into_iter()
            .filter(|trans| trans.transaction_type == TransactionType::Incoming)
            .map(From::from)
            .collect();

        result.transactions = transactions;
    }

    Ok(result.into())
}

#[tracing::instrument(skip_all)]
async fn update_invoice(
    State(api_state): State<ApiState>,
    AppJson(params): AppJson<UpdateInvoiceParams>,
) -> ApiResult<PublicInvoice, DaoInvoiceError> {
    api_state
        .validator
        .validate_update_invoice_params(&params)
        .await?;

    let invoice = api_state
        .inner
        .update_invoice(params)
        .await?;

    let result = api_state
        .inner
        .invoice_to_public_invoice(invoice);
    Ok(result.into())
}

#[tracing::instrument(skip_all)]
async fn cancel_invoice(
    State(state): State<ApiState>,
    AppJson(params): AppJson<CancelInvoiceParams>,
) -> ApiResult<PublicInvoice, DaoInvoiceError> {
    let invoice = state
        .inner
        .cancel_invoice_admin(params.invoice_id)
        .await?;

    let mut result = state
        .inner
        .invoice_to_public_invoice(invoice);

    // TODO: filter it on database query level
    if params.include_transactions && result.status == InvoiceStatus::Waiting {
        let transactions = state
            .inner
            .get_invoice_transactions(params.invoice_id)
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?
            .into_iter()
            .filter(|trans| trans.transaction_type == TransactionType::Incoming)
            .map(From::from)
            .collect();

        result.transactions = transactions;
    }

    Ok(result.into())
}

pub fn routes(hmac_config: HmacConfig) -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/v3/invoice/create",
            post(create_invoice),
        )
        .route("/v3/invoice/get", get(get_invoice))
        .route(
            "/v3/invoice/update",
            post(update_invoice),
        )
        .route(
            "/v3/invoice/cancel",
            post(cancel_invoice),
        )
        .fallback(fallback_handler)
        .method_not_allowed_fallback(method_not_allowed_fallback_handler)
        .layer(axum::middleware::from_fn_with_state(
            hmac_config,
            axum_hmac_validator,
        ))
}
