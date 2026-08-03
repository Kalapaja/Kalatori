use uuid::Uuid;

use crate::chain_client::KeyringClient;
use crate::clients::SwapsClientError;
use crate::dao::{
    DAO,
    DaoInterface,
    DaoSwapError,
};
use crate::types::{
    CreateSwapData,
    SubmittedSwapParams,
    Swap,
    SwapExecutorType,
    SwapSignatureParams,
};

use super::SwapsClients;

#[derive(Debug, thiserror::Error)]
pub enum SwapsExecutorError {
    // TODO: refactor
    #[error("Failed to request swap quote")]
    QuoteRequestFailed,
    /// The swap provider refused the request itself (unsupported route,
    /// amount below bridge minimum, insufficient liquidity, etc.).
    #[error("Swap provider rejected the request: {message}")]
    ProviderRejected { message: String },
    #[error("Swap {swap_id} not found")]
    SwapNotFound { swap_id: Uuid },
    #[error("Invoice {invoice_id} not found")]
    InvoiceNotFound { invoice_id: Uuid },
    #[error("Internal database error")]
    DatabaseError,
}

impl From<SwapsClientError> for SwapsExecutorError {
    fn from(value: SwapsClientError) -> Self {
        match value {
            SwapsClientError::ProviderRejected {
                message,
            } => SwapsExecutorError::ProviderRejected {
                message,
            },
            // The provider answered successfully and told us it will not serve
            // this trade. That is the same class as an explicit rejection — the
            // requester can act on it (different amount, different token) — so
            // it must not be reported as an internal failure. These arms carry
            // no provider text, so supply our own; it is surfaced to the
            // payment UI verbatim.
            SwapsClientError::NoRouteAvailable => SwapsExecutorError::ProviderRejected {
                message: "No swap route is available for this pair right now.".to_string(),
            },
            SwapsClientError::NoLiquidity => SwapsExecutorError::ProviderRejected {
                message: "There is not enough liquidity for this trade right now.".to_string(),
            },
            // `UnusableQuote` deliberately stays internal: the provider gave us
            // a quote we cannot publish (missing gas parameters, unrepresentable
            // expiry). Nothing the requester does differently would fix it.
            //
            // Everything else (transport failures, provider 5xx, malformed
            // responses) is an internal failure from the requester's view
            _ => SwapsExecutorError::QuoteRequestFailed,
        }
    }
}

#[derive(Clone)]
pub struct SwapsExecutor<D: DaoInterface + 'static = DAO> {
    dao: D,
    clients: SwapsClients,
}

#[cfg_attr(test, expect(dead_code))]
impl<D: DaoInterface + 'static> SwapsExecutor<D> {
    pub fn new(
        dao: D,
        clients: SwapsClients,
    ) -> Self {
        Self {
            dao,
            clients,
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn create_swap(
        &self,
        data: CreateSwapData,
    ) -> Result<Swap, SwapsExecutorError> {
        let quote_request_data = data.clone();

        let quote = self
            .clients
            .get_quote(data.swap_executor, quote_request_data)
            .await?;

        let swap = Swap::new(data, quote);

        let created = self
            .dao
            .create_swap(swap)
            .await
            .map_err(|e| match e {
                DaoSwapError::InvoiceNotFound {
                    invoice_id,
                } => SwapsExecutorError::InvoiceNotFound {
                    invoice_id,
                },
                _ => SwapsExecutorError::DatabaseError,
            })?;

        tracing::trace!(
            swap_id = %created.id,
            "Swap created"
        );

        Ok(created)
    }

    #[tracing::instrument(skip_all)]
    pub async fn sign_transaction(
        &self,
        keyring_client: &KeyringClient,
        swap: &Swap,
        // TODO: return SwapsExecutorError for consistency?
    ) -> Result<String, SwapsClientError> {
        self.clients
            .sign_transaction(keyring_client, swap)
            .await
    }

    #[tracing::instrument(
        skip_all,
        fields(swap_id = %swap_signature.swap_id)
    )]
    pub async fn submit_with_signature(
        &self,
        swap_signature: SwapSignatureParams,
    ) -> Result<Swap, SwapsExecutorError> {
        if !matches!(
            swap_signature.swap_executor,
            SwapExecutorType::Bungee | SwapExecutorType::ZeroEx | SwapExecutorType::ZeroExGasless
        ) {
            // TODO: other error, perhaps also check executor on DB level
            tracing::warn!(
                swap_executor = %swap_signature.swap_executor,
                "Got submit with signature request for wrong swap executor"
            );
            return Err(SwapsExecutorError::DatabaseError);
        }

        let swap = self
            .dao
            .update_swap_set_signature(
                swap_signature.swap_id,
                swap_signature.signature,
            )
            .await
            .map_err(|e| match e {
                DaoSwapError::NotFound {
                    swap_id,
                } => SwapsExecutorError::SwapNotFound {
                    swap_id,
                },
                _ => SwapsExecutorError::DatabaseError,
            })?;

        // Persist the submission attempt BEFORE handing the swap to the
        // external executor: if the daemon crashes or the later updates fail,
        // the swap is already `Submitted` and visible to the tracker instead
        // of silently staying `Created` while funds are in flight.
        let submitted_swap = self
            .update_swap_submitted_internally(swap_signature.swap_id)
            .await?;

        // TODO: In case of error need to check an error thoroughly.
        // If it's problem with signature, we can mark it as failed.
        // If it's some kind of network error, we can retry it.
        // In any way we have to understand if it was received by bungee
        // and is being processed to avoid double-payments or just missing
        // the transaction.
        let transaction_hash = match self
            .clients
            .submit_transaction(
                swap.request.swap_executor,
                &swap.swap_details,
            )
            .await
        {
            Ok(transaction_hash) => transaction_hash,
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "External executor rejected swap submission, marking swap as failed"
                );

                // The submission may still have been accepted (e.g. on a
                // network timeout); actually arriving funds are detected by
                // the chain transfer subscription regardless, and the Failed
                // record keeps the attempt visible for reconciliation.
                if let Err(db_error) = self
                    .dao
                    .update_swap_failed(swap_signature.swap_id, e.to_string())
                    .await
                {
                    tracing::error!(
                        error = ?db_error,
                        "Failed to mark swap as failed after a submission error, swap stays visible to the tracker"
                    );
                }

                return Err(e.into());
            },
        };

        tracing::Span::current().record("transaction_hash", &transaction_hash);

        // The tracker polls the database with a short interval and may have
        // already moved the swap from `Submitted` to `Pending`, so only the
        // transaction hash is written here — no status transition.
        match self
            .dao
            .update_swap_transaction_hash(swap_signature.swap_id, transaction_hash)
            .await
        {
            Ok(swap) => {
                tracing::info!("Swap has been submitted successfully");
                Ok(swap)
            },
            Err(e) => {
                // The submission itself succeeded — don't report a failure to
                // the caller (a retry could double-submit). The swap stays
                // Submitted/Pending without a hash and needs manual
                // reconciliation, which this error record points at.
                //
                // Return the post-`Submitted` row, never the pre-submission
                // `swap`: that one still reads `Created`, which would tell the
                // caller nothing was sent while funds are already in flight.
                tracing::error!(
                    error = ?e,
                    "Swap was submitted but recording its transaction hash failed, manual reconciliation required"
                );
                Ok(submitted_swap)
            },
        }
    }

    /// Mark swap as `Submitted` in database. Use this method for swaps which
    /// has been executed inside this service by either sending some API
    /// requests to executor or sent to blockhain directly. For swaps which has
    /// been sent on front-end use `update_swap_submitted_on_front_end`
    /// method.
    async fn update_swap_submitted_internally(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, SwapsExecutorError> {
        self.dao
            .update_swap_submitted(swap_id)
            .await
            .map_err(|e| match e {
                DaoSwapError::NotFound {
                    swap_id,
                } => SwapsExecutorError::SwapNotFound {
                    swap_id,
                },
                _ => SwapsExecutorError::DatabaseError,
            })
    }

    /// Mark swap as `Submitted` in database and update it's related transaction
    /// hash. Use this method for swaps which has been executed on
    /// front-end. For swaps which has been executed inside this service use
    /// `update_swap_submitted_internally` method.
    pub async fn update_swap_submitted_on_front_end(
        &self,
        submitted_swap: SubmittedSwapParams,
    ) -> Result<Swap, SwapsExecutorError> {
        // TODO: either use separate dao methods for different executors or move
        // executor to the dao method too
        let SubmittedSwapParams {
            swap_id,
            swap_executor,
            transaction_hash,
        } = submitted_swap;

        self.dao
            .update_swap_submitted_with_hash(swap_id, transaction_hash.clone())
            .await
            .map_err(|e| {
                // TODO: check more different errors, at least status constraints
                match e {
                    DaoSwapError::NotFound {
                        swap_id,
                    } => SwapsExecutorError::SwapNotFound {
                        swap_id,
                    },
                    _ => SwapsExecutorError::DatabaseError,
                }
            })
            .inspect(|_| {
                tracing::info!(
                    %swap_id,
                    %swap_executor,
                    %transaction_hash,
                    "Swap has been successfully marked as submitted by front-end"
                )
            })
    }

    // pub async fn abandon_swap(&self) -> Result<Swap, SwapsExecutorError> {

    // }
}

#[cfg(test)]
mockall::mock! {
    pub SwapsExecutor<D: DaoInterface + 'static = DAO> {
        pub fn new(
            dao: D,
            clients: SwapsClients,
        ) -> Self;

        pub async fn create_swap(
            &self,
            data: CreateSwapData,
        ) -> Result<Swap, SwapsExecutorError>;

        pub async fn sign_transaction(
            &self,
            keyring_client: &KeyringClient,
            swap: &Swap,
        ) -> Result<String, SwapsClientError>;

        pub async fn submit_with_signature(
            &self,
            swap_signature: SwapSignatureParams,
        ) -> Result<Swap, SwapsExecutorError>;

        pub async fn update_swap_submitted_on_front_end(
            &self,
            submitted_swap: SubmittedSwapParams,
        ) -> Result<Swap, SwapsExecutorError>;
    }

    impl<D: DaoInterface + 'static> Clone for SwapsExecutor<D> {
        fn clone(&self) -> Self;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SwapChainType;

    /// The provider's own explanation is the only thing safe to surface to the
    /// payment UI, so it has to survive the hop to the executor verbatim.
    #[test]
    fn provider_rejection_keeps_the_provider_message() {
        let error = SwapsExecutorError::from(SwapsClientError::ProviderRejected {
            message: "Amount is below the bridge minimum".to_string(),
        });

        assert!(matches!(
            &error,
            SwapsExecutorError::ProviderRejected { message }
                if message == "Amount is below the bridge minimum"
        ));
    }

    /// A provider that answers successfully and declines to serve the trade is
    /// making a rejection, not failing. These arms carry no provider text of
    /// their own, so the conversion supplies a message that is safe to show the
    /// payer — the point is that they reach the API as a 422 and not a 500.
    #[test]
    fn quote_unavailable_is_a_rejection_not_an_internal_failure() {
        for error in [
            SwapsClientError::NoRouteAvailable,
            SwapsClientError::NoLiquidity,
        ] {
            let converted = SwapsExecutorError::from(error.clone());

            let SwapsExecutorError::ProviderRejected {
                message,
            } = converted
            else {
                panic!("{error:?} must map to ProviderRejected, got {converted:?}")
            };

            assert!(
                !message.is_empty(),
                "{error:?} must carry a message for the payment UI"
            );
        }
    }

    /// Transport failures, provider 5xx and malformed responses all reach this
    /// conversion as something other than `ProviderRejected`. None of them are
    /// the requester's fault, so none may surface as a 422 — they have to
    /// collapse to `QuoteRequestFailed`, which the API renders as a 500.
    #[test]
    fn every_other_client_error_stays_an_internal_failure() {
        let internal = [
            SwapsClientError::UnknownApiError,
            SwapsClientError::SignatureIsNotSet,
            SwapsClientError::WrongRawTransaction,
            SwapsClientError::TransactionHashIsNotSet,
            SwapsClientError::OperationIsNotAllowed,
            SwapsClientError::FailedToSignTransaction,
            SwapsClientError::ChainIsNotSupported {
                chain: SwapChainType::Polygon,
            },
            SwapsClientError::DirectionIsNotSupported {
                from_chain: SwapChainType::Polygon,
                to_chain: SwapChainType::Ethereum,
            },
            // The provider answered, but with a quote we cannot publish. The
            // requester cannot act on that, so it is internal.
            SwapsClientError::UnusableQuote,
        ];

        for error in internal {
            let converted = SwapsExecutorError::from(error.clone());

            assert!(
                matches!(
                    converted,
                    SwapsExecutorError::QuoteRequestFailed
                ),
                "{error:?} must map to QuoteRequestFailed, got {converted:?}"
            );
        }
    }
}
