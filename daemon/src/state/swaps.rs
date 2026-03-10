use rust_decimal::prelude::*;
use uuid::Uuid;

use crate::api::ApiErrorExt;
use crate::dao::DaoInterface;
use crate::types::{
    CreateSwapData,
    CreateSwapParams,
    SubmittedSwapParams,
    Swap,
    SwapChainType,
    SwapDirection,
    SwapExecutorType,
    SwapSignatureParams,
};

use super::AppState;

#[derive(Debug, thiserror::Error)]
pub enum SwapRequestError {
    #[error("Invalid chain id: {chain_id}")]
    InvalidChainId { chain_id: u64 },
    #[error("Invoice not found: {invoice_id}")]
    InvoiceNotFound { invoice_id: Uuid },
    #[error("Swap direction from {from_chain_id} to {to_chain_id} is not supported")]
    DirectionIsUnsupported {
        from_chain_id: u64,
        to_chain_id: u64,
    },
    #[error("Failed to get quotes for swap")]
    QuoteRequestFailed,
    #[error("Database error")]
    DatabaseError,
}

impl ApiErrorExt for SwapRequestError {
    fn category(&self) -> &str {
        "SWAP_ERROR"
    }

    fn code(&self) -> &str {
        "SWAP_ERROR"
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    }

    fn message(&self) -> &str {
        "Swap error"
    }
}

impl<D: DaoInterface> AppState<D> {
    pub async fn create_swap(
        &self,
        params: CreateSwapParams,
    ) -> Result<Swap, SwapRequestError> {
        let direction = SwapDirection::Incoming;
        let invoice_id = params.invoice_id;
        let default_chain = self.payments_config.default_chain;
        let to_token_address = self
            .payments_config
            .default_asset_id
            .get(&default_chain)
            .unwrap()
            .clone();

        let from_chain = SwapChainType::try_from(params.from_chain_id).map_err(|chain_id| {
            SwapRequestError::InvalidChainId {
                chain_id,
            }
        })?;

        let to_chain = default_chain.into();

        let swap_executor =
            SwapExecutorType::detect(from_chain, to_chain, direction).ok_or_else(|| {
                SwapRequestError::DirectionIsUnsupported {
                    from_chain_id: from_chain.chain_id(),
                    to_chain_id: to_chain.chain_id(),
                }
            })?;

        let invoice = self
            .get_invoice(invoice_id)
            .await
            // TODO: update error handling
            .map_err(|_| SwapRequestError::DatabaseError)?
            .ok_or(SwapRequestError::InvoiceNotFound {
                invoice_id,
            })?;

        if invoice.invoice.status.is_final() {
            return Err(SwapRequestError::InvoiceNotFound {
                invoice_id,
            })
        }

        // get from params if provided, otherwise calculate from invoice's unfilled
        // amount
        let expected_to_amount_units = if let Some(units) = params.expected_to_amount_units {
            units
        } else {
            // TODO: get real decimals for the asset
            (invoice.unfilled_amount() / Decimal::new(1, 6))
                .to_u128()
                // TODO: change error
                .ok_or(SwapRequestError::DatabaseError)?
        };

        let data = CreateSwapData {
            invoice_id,
            swap_executor,
            from_chain,
            to_chain,
            from_token_address: params.from_asset_id,
            to_token_address,
            from_amount_units: params.from_amount_units,
            from_address: params.from_address,
            to_address: invoice.invoice.payment_address,
            expected_to_amount_units,
            direction,
        };

        let swap = self
            .swaps_executor
            .create_swap(data)
            .await
            .map_err(|_e| {
                // TODO: check errors
                SwapRequestError::QuoteRequestFailed
            })?;

        Ok(swap)
    }

    pub async fn update_swap_submitted(
        &self,
        params: SubmittedSwapParams,
    ) -> Result<Swap, SwapRequestError> {
        self.swaps_executor
            .update_swap_submitted_on_front_end(params)
            .await
            // TODO: check and handle different errors
            .map_err(|_| SwapRequestError::DatabaseError)
    }

    pub async fn submit_swap_with_signature(
        &self,
        params: SwapSignatureParams,
    ) -> Result<Swap, SwapRequestError> {
        self.swaps_executor
            .submit_with_signature(params)
            .await
            // TODO: check and handle different errors
            .map_err(|_| SwapRequestError::DatabaseError)
    }
}
