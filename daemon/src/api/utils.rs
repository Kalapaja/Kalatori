use axum::extract::FromRequest;
use axum::extract::rejection::{
    JsonRejection,
    QueryRejection,
};
use axum::http::StatusCode;
use axum::response::{
    IntoResponse,
    Json,
    Response,
};
use serde::Serialize;

use kalatori_client::types::{
    ApiError,
    ApiResultStructured,
};

use super::ApiErrorExt;
use crate::error::inputs_validation::ApiInputValidationError;

#[derive(thiserror::Error, Debug)]
pub(super) enum AppExtractorError {
    #[error(transparent)]
    Json(#[from] JsonRejection),
    #[error(transparent)]
    Query(#[from] QueryRejection),
}

impl IntoResponse for AppExtractorError {
    fn into_response(self) -> Response {
        let api_error = match self {
            AppExtractorError::Json(rejection) => ApiError {
                // TODO: improve error codes and messages based on rejection reason
                category: "INVALID_REQUEST".to_string(),
                code: "INVALID_JSON".to_string(),
                message: format!("JSON extraction error: {}", rejection),
                details: None,
            },
            AppExtractorError::Query(rejection) => ApiError {
                category: "INVALID_REQUEST".to_string(),
                code: "INVALID_QUERY_PARAMS".to_string(),
                message: format!("Query extraction error: {}", rejection),
                details: None,
            },
        };

        (
            StatusCode::BAD_REQUEST,
            Json(ApiResultStructured::<()>::Err {
                error: api_error,
            }),
        )
            .into_response()
    }
}

#[derive(FromRequest)]
#[from_request(via(axum::extract::Json), rejection(AppExtractorError))]
pub(super) struct AppJson<T>(pub T);

#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppExtractorError))]
pub(super) struct AppQuery<T>(pub T);

pub type ApiResult<T, E> = Result<SuccessWrapper<T>, HandlerError<E>>;

#[derive(Debug)]
pub(super) struct SuccessWrapper<T: Serialize>(T);

impl<T: Serialize> From<T> for SuccessWrapper<T> {
    fn from(value: T) -> Self {
        SuccessWrapper(value)
    }
}

impl<T: Serialize> IntoResponse for SuccessWrapper<T> {
    fn into_response(self) -> Response {
        (
            StatusCode::OK,
            Json(ApiResultStructured::Ok {
                result: self.0,
            }),
        )
            .into_response()
    }
}

/// Combines non domain API related errors with any downstream domain error `E`.
#[derive(Debug, thiserror::Error)]
pub(super) enum HandlerError<E: ApiErrorExt> {
    #[error(transparent)]
    Validation(#[from] ApiInputValidationError),

    #[error(transparent)]
    Domain(#[from] E),
}

impl<E: ApiErrorExt> IntoResponse for HandlerError<E> {
    fn into_response(self) -> Response {
        let status_code = self.http_status_code();
        let api_error = self.to_api_error();

        (
            status_code,
            Json(ApiResultStructured::<()>::Err {
                error: api_error,
            }),
        )
            .into_response()
    }
}

impl<E: ApiErrorExt> ApiErrorExt for HandlerError<E> {
    fn category(&self) -> &str {
        match self {
            Self::Validation(_) => "INVALID_PARAMETER",
            Self::Domain(e) => e.category(),
        }
    }

    fn code(&self) -> &str {
        match self {
            Self::Validation(e) => match e {
                ApiInputValidationError::InvalidRedirectUrl(_) => "INVALID_REDIRECT_URL",
                ApiInputValidationError::InvalidImageUrl(_) => "INVALID_IMAGE_URL",
                ApiInputValidationError::InvalidProductUrl(_) => "INVALID_PRODUCT_URL",
            },
            Self::Domain(e) => e.code(),
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Validation(e) => match e {
                ApiInputValidationError::InvalidRedirectUrl(_) => {
                    "The redirect URL failed validation."
                },
                ApiInputValidationError::InvalidImageUrl(_) => {
                    "A cart item image URL failed validation."
                },
                ApiInputValidationError::InvalidProductUrl(_) => {
                    "A cart item product URL failed validation."
                },
            },
            Self::Domain(e) => e.message(),
        }
    }

    fn http_status_code(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::Domain(e) => e.http_status_code(),
        }
    }
}

pub(super) async fn fallback_handler() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResultStructured::<()>::Err {
            error: ApiError {
                category: "INVALID_REQUEST".to_string(),
                code: "ROUTE_NOT_FOUND".to_string(),
                message: "The requested route was not found.".to_string(),
                details: None,
            },
        }),
    )
}

pub(super) async fn method_not_allowed_fallback_handler() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ApiResultStructured::<()>::Err {
            error: ApiError {
                category: "INVALID_REQUEST".to_string(),
                code: "METHOD_NOT_ALLOWED".to_string(),
                message: "Only GET and POST methods are allowed.".to_string(),
                details: None,
            },
        }),
    )
}
