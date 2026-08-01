use rust_decimal::prelude::*;
use uuid::Uuid;

use crate::api::ApiErrorExt;
use crate::dao::DaoInterface;
use crate::swaps::SwapsExecutorError;
use crate::types::{
    CreateSwapData,
    CreateSwapParams,
    SubmittedSwapParams,
    Swap,
    SwapChainType,
    SwapDirection,
    SwapExecutorType,
    SwapSignatureParams,
    SwapStatus,
};

use super::AppState;

#[derive(Debug, thiserror::Error)]
pub enum SwapRequestError {
    #[error("Invalid chain id: {chain_id}")]
    InvalidChainId { chain_id: u64 },
    #[error("Invoice not found: {invoice_id}")]
    InvoiceNotFound { invoice_id: Uuid },
    #[error("Swap not found: {swap_id}")]
    SwapNotFound { swap_id: Uuid },
    #[error("Swap submission was already claimed while in status {current_status}")]
    SwapAlreadyClaimed { current_status: SwapStatus },
    #[error("Swap direction from {from_chain_id} to {to_chain_id} is not supported")]
    DirectionIsUnsupported {
        from_chain_id: u64,
        to_chain_id: u64,
    },
    /// The swap provider refused the request itself (unsupported route,
    /// amount below bridge minimum, insufficient liquidity, etc.).
    /// `message` is the provider's explanation, surfaced to the payment UI.
    #[error("Swap provider rejected the request: {message}")]
    ProviderRejected { message: String },
    #[error("Failed to get quotes for swap")]
    QuoteRequestFailed,
    /// This deployment settles on a chain that cannot be a swap destination
    /// (Asset Hub), so no swap can be prepared here at all. Nothing the caller
    /// sends changes that.
    #[error("Swaps are not available for this shop's settlement chain")]
    SwapDestinationUnavailable,
    /// The submitted signature does not match the shape the stored quote
    /// requires. Fixable by the submitter, so it is a 400 and nothing is
    /// persisted.
    #[error("Submitted signature does not match the stored quote")]
    InvalidSignature,
    #[error("Asset metadata unavailable for asset {asset_id}")]
    AssetMetadataUnavailable { asset_id: String },
    #[error("Database error")]
    DatabaseError,
    /// Scaling the invoice's unfilled amount into integer token base units
    /// overflowed. Kept distinct from `DatabaseError` because it is a bad
    /// amount, not a storage fault, and is not worth retrying.
    #[error("Amount {amount} cannot be converted to token base units")]
    AmountConversion { amount: Decimal },
}

impl From<SwapsExecutorError> for SwapRequestError {
    fn from(value: SwapsExecutorError) -> Self {
        match value {
            SwapsExecutorError::ProviderRejected {
                message,
            } => SwapRequestError::ProviderRejected {
                message,
            },
            SwapsExecutorError::QuoteRequestFailed => SwapRequestError::QuoteRequestFailed,
            SwapsExecutorError::SwapNotFound {
                swap_id,
            } => SwapRequestError::SwapNotFound {
                swap_id,
            },
            SwapsExecutorError::SwapAlreadyClaimed {
                current_status,
            } => SwapRequestError::SwapAlreadyClaimed {
                current_status,
            },
            SwapsExecutorError::InvoiceNotFound {
                invoice_id,
            } => SwapRequestError::InvoiceNotFound {
                invoice_id,
            },
            SwapsExecutorError::InvalidSignature => SwapRequestError::InvalidSignature,
            SwapsExecutorError::DatabaseError => SwapRequestError::DatabaseError,
        }
    }
}

impl ApiErrorExt for SwapRequestError {
    fn category(&self) -> &str {
        match self {
            SwapRequestError::InvalidChainId {
                ..
            }
            | SwapRequestError::DirectionIsUnsupported {
                ..
            }
            | SwapRequestError::InvalidSignature
            | SwapRequestError::SwapAlreadyClaimed {
                ..
            } => "INVALID_REQUEST",
            SwapRequestError::InvoiceNotFound {
                ..
            }
            | SwapRequestError::SwapNotFound {
                ..
            } => "ENTITY_NOT_FOUND",
            SwapRequestError::ProviderRejected {
                ..
            }
            | SwapRequestError::QuoteRequestFailed => "SWAP_ERROR",
            SwapRequestError::SwapDestinationUnavailable => "SERVICE_UNAVAILABLE",
            SwapRequestError::AssetMetadataUnavailable {
                ..
            }
            | SwapRequestError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn code(&self) -> &str {
        match self {
            SwapRequestError::InvalidChainId {
                ..
            } => "INVALID_CHAIN_ID",
            SwapRequestError::InvoiceNotFound {
                ..
            } => "INVOICE_NOT_FOUND",
            SwapRequestError::SwapNotFound {
                ..
            } => "SWAP_NOT_FOUND",
            SwapRequestError::DirectionIsUnsupported {
                ..
            } => "SWAP_DIRECTION_UNSUPPORTED",
            SwapRequestError::InvalidSignature => "INVALID_SWAP_SIGNATURE",
            SwapRequestError::SwapAlreadyClaimed {
                ..
            } => "SWAP_ALREADY_SUBMITTED",
            SwapRequestError::ProviderRejected {
                ..
            } => "SWAP_PROVIDER_REJECTED",
            SwapRequestError::QuoteRequestFailed => "QUOTE_REQUEST_FAILED",
            SwapRequestError::SwapDestinationUnavailable => "SWAP_DESTINATION_UNAVAILABLE",
            SwapRequestError::AssetMetadataUnavailable {
                ..
            }
            | SwapRequestError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            SwapRequestError::InvalidChainId {
                ..
            }
            | SwapRequestError::DirectionIsUnsupported {
                ..
            }
            | SwapRequestError::InvalidSignature => reqwest::StatusCode::BAD_REQUEST,
            SwapRequestError::SwapAlreadyClaimed {
                ..
            } => reqwest::StatusCode::CONFLICT,
            SwapRequestError::InvoiceNotFound {
                ..
            }
            | SwapRequestError::SwapNotFound {
                ..
            } => reqwest::StatusCode::NOT_FOUND,
            SwapRequestError::ProviderRejected {
                ..
            } => reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            // The caller cannot make this succeed by sending anything else —
            // this shop simply cannot do swaps — so it is not a 4xx.
            SwapRequestError::SwapDestinationUnavailable => {
                reqwest::StatusCode::SERVICE_UNAVAILABLE
            },
            SwapRequestError::QuoteRequestFailed
            | SwapRequestError::AssetMetadataUnavailable {
                ..
            }
            | SwapRequestError::DatabaseError => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn message(&self) -> &str {
        match self {
            SwapRequestError::InvalidChainId {
                ..
            } => "The provided chain id is not supported.",
            SwapRequestError::InvoiceNotFound {
                ..
            } => "The related invoice was not found.",
            SwapRequestError::SwapNotFound {
                ..
            } => "The requested swap was not found.",
            SwapRequestError::DirectionIsUnsupported {
                ..
            } => "This swap direction is not supported.",
            SwapRequestError::InvalidSignature => {
                "The submitted signature does not match the prepared swap quote."
            },
            SwapRequestError::SwapAlreadyClaimed {
                ..
            } => "The swap has already been submitted.",
            SwapRequestError::ProviderRejected {
                message,
            } => message,
            SwapRequestError::QuoteRequestFailed => "Failed to get a quote from the swap provider.",
            SwapRequestError::SwapDestinationUnavailable => {
                "Swaps are not available for this shop."
            },
            // The asset id is deliberately not surfaced: it's our own
            // configuration/chain-metadata state, not something the caller can act on.
            SwapRequestError::AssetMetadataUnavailable {
                ..
            } => "The swap could not be prepared: asset metadata is unavailable.",
            SwapRequestError::DatabaseError => "A database error occurred.",
        }
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
        // Startup validation requires a default asset for every chain, so this
        // should always hit — but `/public/swap/create` is unauthenticated, and
        // no configuration mistake may be reachable as a remote daemon kill.
        let to_token_address = self
            .payments_config
            .default_asset_id
            .get(&default_chain)
            .ok_or_else(
                || SwapRequestError::AssetMetadataUnavailable {
                    asset_id: default_chain.to_string(),
                },
            )?
            .clone();

        let from_chain = SwapChainType::try_from(params.from_chain_id).map_err(|chain_id| {
            SwapRequestError::InvalidChainId {
                chain_id,
            }
        })?;

        // A deployment can legitimately settle on a chain that is not a swap
        // endpoint (Asset Hub). That must answer this unauthenticated route
        // with a rejection, not take the daemon down.
        let to_chain = SwapChainType::try_from(default_chain).map_err(|chain| {
            tracing::warn!(
                %chain,
                "Swap requested but the configured default chain cannot be a swap destination"
            );

            SwapRequestError::SwapDestinationUnavailable
        })?;

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
            .map_err(|e| {
                tracing::warn!(
                    %invoice_id,
                    error.source = ?e,
                    "Failed to load invoice while creating a swap"
                );
                SwapRequestError::DatabaseError
            })?
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
            // Convert the unfilled amount into the destination token's
            // smallest units using its real on-chain decimals; assuming 6
            // underpays or overpays for any other precision.
            let decimals = self
                .asset_decimals_map
                .get(&default_chain)
                .and_then(|assets| assets.get(&to_token_address))
                .copied()
                .ok_or_else(|| {
                    tracing::error!(
                        chain = %default_chain,
                        asset_id = %to_token_address,
                        "Destination asset decimals are not known, can't calculate swap target amount"
                    );
                    SwapRequestError::AssetMetadataUnavailable {
                        asset_id: to_token_address.clone(),
                    }
                })?;

            let one_unit = Decimal::try_new(1, decimals.into()).map_err(|_| {
                tracing::error!(
                    chain = %default_chain,
                    asset_id = %to_token_address,
                    decimals,
                    "Destination asset decimals exceed the supported range"
                );
                SwapRequestError::AssetMetadataUnavailable {
                    asset_id: to_token_address.clone(),
                }
            })?;

            // `/ one_unit` is a multiplication by 10^decimals and panics on
            // `Decimal` overflow; `to_u128` rejects anything that does not fit.
            let amount = invoice.unfilled_amount();
            amount
                .checked_div(one_unit)
                .and_then(|scaled| scaled.to_u128())
                .ok_or(SwapRequestError::AmountConversion {
                    amount,
                })?
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
            origin: Default::default(),
        };

        let from_amount_units = data.from_amount_units;

        let swap = self
            .swaps_executor
            .create_swap(data)
            .await
            .map_err(|e| {
                tracing::warn!(
                    %invoice_id,
                    %swap_executor,
                    %from_chain,
                    %to_chain,
                    from_amount_units,
                    error.source = ?e,
                    "Failed to create swap"
                );
                SwapRequestError::from(e)
            })?;

        Ok(swap)
    }

    pub async fn update_swap_submitted(
        &self,
        params: SubmittedSwapParams,
    ) -> Result<Swap, SwapRequestError> {
        let swap_id = params.swap_id;

        self.swaps_executor
            .update_swap_submitted_on_front_end(params)
            .await
            .map_err(|e| {
                tracing::warn!(
                    %swap_id,
                    error.source = ?e,
                    "Failed to mark swap as submitted"
                );
                SwapRequestError::from(e)
            })
    }

    pub async fn submit_swap_with_signature(
        &self,
        params: SwapSignatureParams,
    ) -> Result<Swap, SwapRequestError> {
        let swap_id = params.swap_id;

        self.swaps_executor
            .submit_with_signature(params)
            .await
            .map_err(|e| {
                tracing::warn!(
                    %swap_id,
                    error.source = ?e,
                    "Failed to submit swap with signature"
                );
                SwapRequestError::from(e)
            })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use secrecy::SecretString;

    use crate::chain::InvoiceRegistry;
    use crate::chain_client::KeyringClient;
    use crate::configs::{
        PaymentsConfig,
        ShopConfig,
        ShopMetaConfig,
    };
    use crate::dao::MockDaoInterface;
    use crate::swaps::SwapsExecutor;
    use crate::types::{
        ChainType,
        DetectedShopPlatform,
        default_invoice,
        default_swap,
    };

    use super::*;

    #[test]
    fn provider_rejection_maps_to_422_with_provider_message() {
        let error = SwapRequestError::from(SwapsExecutorError::ProviderRejected {
            message: "Amount is below the bridge minimum".to_string(),
        });

        assert!(matches!(
            &error,
            SwapRequestError::ProviderRejected { message }
                if message == "Amount is below the bridge minimum"
        ));
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(error.code(), "SWAP_PROVIDER_REJECTED");
        assert_eq!(
            error.message(),
            "Amount is below the bridge minimum"
        );
    }

    #[test]
    fn executor_not_found_errors_map_to_404() {
        let swap_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        let error = SwapRequestError::from(SwapsExecutorError::SwapNotFound {
            swap_id,
        });
        assert!(matches!(
            error,
            SwapRequestError::SwapNotFound { swap_id: id } if id == swap_id
        ));
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::NOT_FOUND
        );

        let error = SwapRequestError::from(SwapsExecutorError::InvoiceNotFound {
            invoice_id,
        });
        assert!(matches!(
            error,
            SwapRequestError::InvoiceNotFound { invoice_id: id } if id == invoice_id
        ));
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn internal_failures_stay_500() {
        for error in [
            SwapRequestError::from(SwapsExecutorError::QuoteRequestFailed),
            SwapRequestError::from(SwapsExecutorError::DatabaseError),
        ] {
            assert_eq!(
                error.http_status_code(),
                reqwest::StatusCode::INTERNAL_SERVER_ERROR
            );
        }
    }

    #[test]
    fn request_validation_errors_map_to_400() {
        let error = SwapRequestError::InvalidChainId {
            chain_id: 999,
        };
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::BAD_REQUEST
        );

        let error = SwapRequestError::DirectionIsUnsupported {
            from_chain_id: 1,
            to_chain_id: 137,
        };
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::BAD_REQUEST
        );
    }

    /// A signature the submitter can re-sign is their problem to fix, so the
    /// unauthenticated endpoint has to answer 4xx. Answering 500 — or, as
    /// before, aborting the daemon — is the bug.
    #[test]
    fn a_rejected_signature_is_a_400_not_an_internal_error() {
        let error = SwapRequestError::from(SwapsExecutorError::InvalidSignature);

        assert!(matches!(
            error,
            SwapRequestError::InvalidSignature
        ));
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert_eq!(error.category(), "INVALID_REQUEST");
        assert_eq!(error.code(), "INVALID_SWAP_SIGNATURE");
    }

    #[test]
    fn an_already_claimed_swap_maps_to_conflict() {
        let error = SwapRequestError::from(SwapsExecutorError::SwapAlreadyClaimed {
            current_status: SwapStatus::Submitted,
        });

        assert!(matches!(
            &error,
            SwapRequestError::SwapAlreadyClaimed {
                current_status: SwapStatus::Submitted,
            }
        ));
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::CONFLICT
        );
        assert_eq!(error.category(), "INVALID_REQUEST");
        assert_eq!(error.code(), "SWAP_ALREADY_SUBMITTED");
    }

    const POLYGON_USDC: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";

    fn app_state_with_decimals(
        asset_decimals_map: HashMap<ChainType, HashMap<String, u8>>
    ) -> AppState<MockDaoInterface> {
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

        AppState::new(
            KeyringClient::default(),
            MockDaoInterface::default(),
            InvoiceRegistry::new(),
            SwapsExecutor::default(),
            HashMap::new(),
            asset_decimals_map,
            payments_config,
            shop_config,
            SecretString::from("secret"),
        )
    }

    fn create_swap_params(invoice_id: Uuid) -> CreateSwapParams {
        CreateSwapParams {
            invoice_id,
            // Base
            from_chain_id: 8453,
            from_asset_id: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(),
            from_address: "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9".to_string(),
            from_amount_units: 10_000_000,
            expected_to_amount_units: None,
        }
    }

    #[tokio::test]
    async fn test_create_swap_converts_amount_with_destination_decimals() {
        // Give the destination asset 2 decimals to make the conversion
        // visible: 100.00 unfilled → 10_000 smallest units (the old
        // hardcoded 6 would produce 100_000_000)
        let mut app_state = app_state_with_decimals(HashMap::from([(
            ChainType::Polygon,
            HashMap::from([(POLYGON_USDC.to_string(), 2)]),
        )]));

        let invoice = default_invoice();
        let invoice_id = invoice.id;

        app_state
            .dao
            .expect_get_invoice_with_received_amount_by_id()
            .returning(move |_| {
                Ok(Some(
                    invoice
                        .clone()
                        .with_amount(Decimal::ZERO),
                ))
            });

        app_state
            .swaps_executor
            .expect_create_swap()
            .withf(|data| data.expected_to_amount_units == 10_000)
            .returning(move |_| Ok(default_swap(invoice_id)));

        app_state
            .create_swap(create_swap_params(invoice_id))
            .await
            .expect("swap should be created with converted target amount");
    }

    #[tokio::test]
    async fn test_create_swap_rejects_unknown_destination_decimals() {
        let mut app_state = app_state_with_decimals(HashMap::new());

        let invoice = default_invoice();
        let invoice_id = invoice.id;

        app_state
            .dao
            .expect_get_invoice_with_received_amount_by_id()
            .returning(move |_| {
                Ok(Some(
                    invoice
                        .clone()
                        .with_amount(Decimal::ZERO),
                ))
            });

        let result = app_state
            .create_swap(create_swap_params(invoice_id))
            .await;

        assert!(matches!(
            result,
            Err(SwapRequestError::AssetMetadataUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn test_create_swap_rejects_a_missing_default_asset() {
        let mut app_state = app_state_with_decimals(HashMap::new());
        app_state
            .payments_config
            .default_asset_id
            .clear();

        let result = app_state
            .create_swap(create_swap_params(Uuid::new_v4()))
            .await;

        assert!(matches!(
            result,
            Err(SwapRequestError::AssetMetadataUnavailable { asset_id })
                if asset_id == ChainType::Polygon.to_string()
        ));
    }

    /// Asset Hub is a valid settlement chain but has no `SwapChainType`
    /// equivalent. Converting it used to `unimplemented!()`, so a deployment
    /// with `default_chain = PolkadotAssetHub` answered this unauthenticated
    /// route by shutting the daemon down.
    #[tokio::test]
    async fn test_create_swap_rejects_a_settlement_chain_that_cannot_swap() {
        let mut app_state = app_state_with_decimals(HashMap::new());
        app_state.payments_config.default_chain = ChainType::PolkadotAssetHub;
        app_state
            .payments_config
            .default_asset_id
            .insert(
                ChainType::PolkadotAssetHub,
                "1337".to_string(),
            );

        let result = app_state
            .create_swap(create_swap_params(Uuid::new_v4()))
            .await;

        let error = result.expect_err("Asset Hub cannot be a swap destination");
        assert!(matches!(
            error,
            SwapRequestError::SwapDestinationUnavailable
        ));
        assert_eq!(
            error.http_status_code(),
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(error.category(), "SERVICE_UNAVAILABLE");
        assert_eq!(
            error.code(),
            "SWAP_DESTINATION_UNAVAILABLE"
        );
        assert_eq!(
            error.message(),
            "Swaps are not available for this shop."
        );
    }
}
