//! Validator for API request parameters.
//!
//! Private to the `api` module — do not re-export.

use futures::future;

use kalatori_client::types::{
    CreateInvoiceParams,
    InvoiceCart as PublicInvoiceCart,
    UpdateInvoiceParams,
};
use url::Url;

use crate::configs::ApiValidatorConfig;
use crate::error::inputs_validation::ApiInputValidationError;
use crate::utils::url_validation::{
    self,
    ALLOWED_IMAGE_EXTENSIONS,
    UrlValidationError,
};

pub(super) struct ApiParamsValidator {
    allowed_base_redirect_url: Url,
    allowed_base_image_urls: Vec<Url>,
    allow_insecure_urls: bool,
}

// Public api and domain related fns.
impl ApiParamsValidator {
    pub(super) fn new(config: ApiValidatorConfig) -> Self {
        Self {
            allowed_base_redirect_url: config.allowed_base_redirect_url,
            allowed_base_image_urls: config.allowed_base_image_urls,
            allow_insecure_urls: config.allow_insecure_urls,
        }
    }

    pub(super) async fn validate_create_invoice_params(
        &self,
        params: &CreateInvoiceParams,
    ) -> Result<(), ApiInputValidationError> {
        self.validate_redirect_url(&params.redirect_url)
            .await?;
        self.validate_cart_urls(&params.cart)
            .await?;

        Ok(())
    }

    pub(super) async fn validate_update_invoice_params(
        &self,
        params: &UpdateInvoiceParams,
    ) -> Result<(), ApiInputValidationError> {
        self.validate_cart_urls(&params.cart)
            .await
    }

    async fn validate_redirect_url(
        &self,
        url: &str,
    ) -> Result<(), ApiInputValidationError> {
        self.validate_url_with_allowed_base(
            url,
            &self.allowed_base_redirect_url,
            ApiInputValidationError::InvalidRedirectUrl,
        )
        .await
    }

    async fn validate_cart_urls(
        &self,
        PublicInvoiceCart {
            items,
        }: &PublicInvoiceCart,
    ) -> Result<(), ApiInputValidationError> {
        future::try_join_all(items.iter().map(|item| async {
            if let Some(url) = &item.image_url {
                self.validate_image_url(url).await?;
            }
            if let Some(url) = &item.product_url {
                self.validate_product_url(url).await?;
            }

            Ok::<(), ApiInputValidationError>(())
        }))
        .await
        .map(|_| ())
    }

    async fn validate_image_url(
        &self,
        url: &str,
    ) -> Result<(), ApiInputValidationError> {
        self.validate_url_with_allowed_base_many(
            url,
            &self.allowed_base_image_urls,
            Some(ALLOWED_IMAGE_EXTENSIONS),
            ApiInputValidationError::InvalidImageUrl,
        )
        .await
    }

    async fn validate_product_url(
        &self,
        url: &str,
    ) -> Result<(), ApiInputValidationError> {
        self.validate_url(
            url,
            ApiInputValidationError::InvalidProductUrl,
        )
        .await
    }
}

// Private helper fns for validation.
impl ApiParamsValidator {
    async fn validate_url(
        &self,
        url: &str,
        error_op: impl FnOnce(UrlValidationError) -> ApiInputValidationError,
    ) -> Result<(), ApiInputValidationError> {
        if !self.allow_insecure_urls {
            url_validation::validate(url)
                .await
                .map_err(error_op)?;
        }

        Ok(())
    }

    async fn validate_url_with_allowed_base(
        &self,
        url: &str,
        allowed_base: &Url,
        error_op: impl FnOnce(UrlValidationError) -> ApiInputValidationError,
    ) -> Result<(), ApiInputValidationError> {
        if !self.allow_insecure_urls {
            url_validation::validate_with_allowed_base(url, allowed_base)
                .await
                .map_err(error_op)?;
        }

        Ok(())
    }

    async fn validate_url_with_allowed_base_many(
        &self,
        url: &str,
        allowed_bases: &[Url],
        allowed_extensions: Option<&[&str]>,
        error_op: impl FnOnce(UrlValidationError) -> ApiInputValidationError,
    ) -> Result<(), ApiInputValidationError> {
        if !self.allow_insecure_urls {
            url_validation::validate_with_allowed_base_many(url, allowed_bases, allowed_extensions)
                .await
                .map_err(error_op)?;
        }

        Ok(())
    }
}
