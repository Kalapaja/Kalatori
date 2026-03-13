use std::collections::HashMap;

use axum::extract::State;
use axum::routing::get;
use uuid::Uuid;

use crate::types::InvoiceWithReceivedAmount;

use super::ApiState;
use super::utils::{
    fallback_handler,
    method_not_allowed_fallback_handler,
    SuccessWrapper,
};

async fn get_invoices_registry_state(
    State(state): State<ApiState>,
) -> SuccessWrapper<HashMap<Uuid, InvoiceWithReceivedAmount>> {
    let result = state
        .get_invoices_registry_state()
        .await;

    result.into()
}

pub fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/invoices-registry", get(get_invoices_registry_state))
        .fallback(fallback_handler)
        .method_not_allowed_fallback(method_not_allowed_fallback_handler)
}
