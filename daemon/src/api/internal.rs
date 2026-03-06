use axum::extract::State;
use axum::routing::get;

use crate::dao::DaoChangesError;
use crate::types::{
    GetChangesParams,
    PublicChangesResponse,
};

use super::ApiState;
use super::utils::{
    ApiResult,
    AppQuery,
    fallback_handler,
    method_not_allowed_fallback_handler,
};

#[tracing::instrument(skip_all)]
async fn get_changes(
    State(api_state): State<ApiState>,
    AppQuery(params): AppQuery<GetChangesParams>,
) -> ApiResult<PublicChangesResponse, DaoChangesError> {
    let result = api_state
        .inner
        .get_invoice_changes(params.since)
        .await?;
    Ok(result.into())
}

pub fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/changes", get(get_changes))
        .fallback(fallback_handler)
        .method_not_allowed_fallback(method_not_allowed_fallback_handler)
}
