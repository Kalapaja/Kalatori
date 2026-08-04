use std::sync::Arc;

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
    SwapStatus,
};

use super::SwapsClients;

#[cfg_attr(test, mockall::automock)]
#[trait_variant::make(Send)]
pub trait SwapsClientsInterface: Send + Sync + 'static {
    async fn get_quote(
        &self,
        executor: SwapExecutorType,
        data: CreateSwapData,
    ) -> Result<crate::types::SwapQuote, SwapsClientError>;

    async fn sign_transaction(
        &self,
        keyring_client: &KeyringClient,
        swap: &Swap,
    ) -> Result<String, SwapsClientError>;

    fn validate_signature(
        &self,
        executor: SwapExecutorType,
        details: &crate::types::SwapDetails,
        signature: &str,
    ) -> Result<(), SwapsClientError>;

    async fn submit_transaction(
        &self,
        executor: SwapExecutorType,
        data: &crate::types::SwapDetails,
    ) -> Result<String, SwapsClientError>;
}

impl SwapsClientsInterface for SwapsClients {
    async fn get_quote(
        &self,
        executor: SwapExecutorType,
        data: CreateSwapData,
    ) -> Result<crate::types::SwapQuote, SwapsClientError> {
        self.get_quote(executor, data).await
    }

    async fn sign_transaction(
        &self,
        keyring_client: &KeyringClient,
        swap: &Swap,
    ) -> Result<String, SwapsClientError> {
        self.sign_transaction(keyring_client, swap)
            .await
    }

    fn validate_signature(
        &self,
        executor: SwapExecutorType,
        details: &crate::types::SwapDetails,
        signature: &str,
    ) -> Result<(), SwapsClientError> {
        self.validate_signature(executor, details, signature)
    }

    async fn submit_transaction(
        &self,
        executor: SwapExecutorType,
        data: &crate::types::SwapDetails,
    ) -> Result<String, SwapsClientError> {
        self.submit_transaction(executor, data)
            .await
    }
}

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
    #[error("Swap submission was already claimed while in status {current_status}")]
    SwapAlreadyClaimed { current_status: SwapStatus },
    #[error("Invoice {invoice_id} not found")]
    InvoiceNotFound { invoice_id: Uuid },
    /// The submitted signature does not match the shape the stored quote
    /// requires. The submitter can fix this by re-signing, so it is a 4xx and
    /// the swap row is left untouched.
    #[error("Submitted signature does not match the stored quote")]
    InvalidSignature,
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
            // Caller-supplied data we refused to accept, not a failure of ours:
            // it has to reach the API as a 4xx so the submitter knows to
            // re-sign, and must never be reported as an internal error.
            SwapsClientError::InvalidSignaturePayload => SwapsExecutorError::InvalidSignature,
            // `UnusableQuote` deliberately stays internal: the provider gave us
            // a quote we cannot publish (an unrepresentable expiry timestamp).
            // Nothing the requester does differently would fix it.
            //
            // Everything else (transport failures, provider 5xx, malformed
            // responses) is an internal failure from the requester's view
            _ => SwapsExecutorError::QuoteRequestFailed,
        }
    }
}

#[derive(Clone)]
pub struct SwapsExecutor<D: DaoInterface + 'static = DAO, C: SwapsClientsInterface = SwapsClients> {
    dao: D,
    clients: Arc<C>,
}

#[cfg_attr(test, expect(dead_code))]
impl<D: DaoInterface + 'static, C: SwapsClientsInterface> SwapsExecutor<D, C> {
    pub fn new(
        dao: D,
        clients: C,
    ) -> Self {
        Self {
            dao,
            clients: Arc::new(clients),
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
        // The signature arrives from the unauthenticated public endpoint, so it
        // has to be checked against the stored quote *before* it is written:
        // a payload the submission path cannot parse must neither reach the
        // swap row nor be handed to the executor. The stored executor is the
        // authority here — it's the one that drives submission below — not the
        // executor the caller claims in the request.
        let stored = self
            .dao
            .get_swap_by_id(swap_signature.swap_id)
            .await
            .map_err(|_| SwapsExecutorError::DatabaseError)?
            .ok_or(SwapsExecutorError::SwapNotFound {
                swap_id: swap_signature.swap_id,
            })?;

        // Only payer-signed executors accept a submitted signature at all. This
        // is checked against the stored executor rather than the one named in
        // the request: the request's is caller-controlled, and answering it
        // would let a caller pick an executor that validates nothing.
        if !matches!(
            stored.request.swap_executor,
            SwapExecutorType::Bungee | SwapExecutorType::ZeroEx | SwapExecutorType::ZeroExGasless
        ) {
            tracing::warn!(
                swap_executor = %stored.request.swap_executor,
                "Got submit-with-signature request for a swap that is not payer-signed"
            );

            return Err(SwapsExecutorError::InvalidSignature)
        }

        self.clients.validate_signature(
            stored.request.swap_executor,
            &stored.swap_details,
            &swap_signature.signature,
        )?;

        // Atomically persist the signature and the submission attempt BEFORE
        // handing the swap to the external executor. Only the request that
        // advances `Created` to `Submitted` may reach the provider; if the
        // daemon crashes or a later update fails, the attempt is already
        // visible to the tracker instead of silently staying `Created` while
        // funds are in flight.
        let submitted_swap = self
            .dao
            .claim_swap_for_submission(
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
                DaoSwapError::AlreadyClaimed {
                    current_status,
                } => SwapsExecutorError::SwapAlreadyClaimed {
                    current_status,
                },
                _ => SwapsExecutorError::DatabaseError,
            })?;

        // TODO: In case of error need to check an error thoroughly.
        // If it's problem with signature, we can mark it as failed.
        // If it's some kind of network error, we can retry it.
        // In any way we have to understand if it was received by bungee
        // and is being processed to avoid double-payments or just missing
        // the transaction.
        let transaction_hash = match self
            .clients
            .submit_transaction(
                submitted_swap.request.swap_executor,
                &submitted_swap.swap_details,
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

    /// Mark swap as `Submitted` in database and update it's related transaction
    /// hash. Use this method for swaps which has been executed on
    /// front-end. Backend-submitted swaps use `claim_swap_for_submission`.
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
    use crate::clients::{
        RawSwapDetails,
        default_zero_ex_gasless_raw_transaction,
    };
    use crate::dao::MockDaoInterface;
    use crate::types::{
        SwapChainType,
        SwapSignatureParams,
        default_swap,
    };

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

    /// A payload the submitter can fix has to reach the API as a 4xx, so it
    /// must not collapse into the internal-failure bucket above.
    #[test]
    fn an_invalid_signature_payload_is_the_submitters_fault() {
        assert!(matches!(
            SwapsExecutorError::from(SwapsClientError::InvalidSignaturePayload),
            SwapsExecutorError::InvalidSignature
        ));
    }

    fn validating_swaps_clients() -> MockSwapsClientsInterface {
        let mut clients = MockSwapsClientsInterface::new();
        clients
            .expect_validate_signature()
            .returning(|_, _, _| Ok(()));
        clients
    }

    fn gasless_swap_needing_two_signatures() -> Swap {
        let mut swap = default_swap(Uuid::new_v4());
        let mut raw_transaction = default_zero_ex_gasless_raw_transaction();
        raw_transaction.approval = Some(raw_transaction.raw_trade.clone());

        swap.request.swap_executor = SwapExecutorType::ZeroExGasless;
        swap.swap_details.raw_transaction = RawSwapDetails::ZeroExGasless(raw_transaction);

        swap
    }

    #[tokio::test]
    async fn an_already_claimed_swap_never_reaches_the_provider() {
        let swap = gasless_swap_needing_two_signatures();
        let swap_id = swap.id;

        let mut dao = MockDaoInterface::new();
        dao.expect_get_swap_by_id()
            .once()
            .returning(move |_| Ok(Some(swap.clone())));
        dao.expect_claim_swap_for_submission()
            .once()
            .returning(|_, _| {
                Err(DaoSwapError::AlreadyClaimed {
                    current_status: SwapStatus::Submitted,
                })
            });
        dao.expect_update_swap_submitted()
            .never();

        let mut clients = validating_swaps_clients();
        clients
            .expect_submit_transaction()
            .never();

        let executor = SwapsExecutor::new(dao, clients);
        let result = executor
            .submit_with_signature(SwapSignatureParams {
                swap_id,
                swap_executor: SwapExecutorType::ZeroExGasless,
                signature: "0xdeadbeef|0xcafebabe".to_string(),
            })
            .await;

        assert!(matches!(
            result,
            Err(SwapsExecutorError::SwapAlreadyClaimed {
                current_status: SwapStatus::Submitted,
            })
        ));
    }

    #[tokio::test]
    async fn a_valid_submission_claims_once_before_calling_the_provider() {
        let swap = gasless_swap_needing_two_signatures();
        let swap_id = swap.id;
        let signature = "0xdeadbeef|0xcafebabe".to_string();

        let mut claimed = swap.clone();
        claimed.status = SwapStatus::Submitted;
        claimed.swap_details.signature = Some(signature.clone());

        let mut completed = claimed.clone();
        completed.swap_details.transaction_hash = Some("transaction-hash".to_string());

        let mut dao = MockDaoInterface::new();
        dao.expect_get_swap_by_id()
            .once()
            .return_once(move |_| Ok(Some(swap)));

        let expected_signature = signature.clone();
        dao.expect_claim_swap_for_submission()
            .withf(move |id, submitted_signature| {
                *id == swap_id && submitted_signature == &expected_signature
            })
            .once()
            .return_once(move |_, _| Ok(claimed));
        dao.expect_update_swap_submitted()
            .never();
        dao.expect_update_swap_transaction_hash()
            .withf(move |id, transaction_hash| {
                *id == swap_id && transaction_hash == "transaction-hash"
            })
            .once()
            .return_once(move |_, _| Ok(completed));

        let mut clients = validating_swaps_clients();
        clients
            .expect_submit_transaction()
            .withf(|executor, details| {
                *executor == SwapExecutorType::ZeroExGasless
                    && details.signature.as_deref() == Some("0xdeadbeef|0xcafebabe")
            })
            .once()
            .returning(|_, _| Ok("transaction-hash".to_string()));

        let executor = SwapsExecutor::new(dao, clients);
        let result = executor
            .submit_with_signature(SwapSignatureParams {
                swap_id,
                swap_executor: SwapExecutorType::ZeroExGasless,
                signature,
            })
            .await
            .unwrap();

        assert_eq!(
            result
                .swap_details
                .transaction_hash
                .as_deref(),
            Some("transaction-hash")
        );
    }

    /// The reported bug persisted the payer's unsplittable signature and only
    /// then panicked, leaving a swap row that could never be submitted. The
    /// order matters as much as the rejection: `expect_...().never()` is what
    /// pins the row as untouched.
    #[tokio::test]
    async fn a_malformed_signature_never_reaches_the_swap_row() {
        let swap = gasless_swap_needing_two_signatures();
        let swap_id = swap.id;

        let mut dao = MockDaoInterface::new();
        dao.expect_get_swap_by_id()
            .returning(move |_| Ok(Some(swap.clone())));
        dao.expect_claim_swap_for_submission()
            .never();

        let mut clients = MockSwapsClientsInterface::new();
        clients
            .expect_validate_signature()
            .withf(|executor, _, signature| {
                *executor == SwapExecutorType::ZeroExGasless && signature == "0xdeadbeef"
            })
            .once()
            .returning(|_, _, _| Err(SwapsClientError::InvalidSignaturePayload));
        clients
            .expect_submit_transaction()
            .never();

        let executor = SwapsExecutor::new(dao, clients);

        let result = executor
            .submit_with_signature(SwapSignatureParams {
                swap_id,
                swap_executor: SwapExecutorType::ZeroExGasless,
                // One signature for a quote that needs the
                // `"<trade>|<approval>"` pair.
                signature: "0xdeadbeef".to_string(),
            })
            .await;

        assert!(
            matches!(
                result,
                Err(SwapsExecutorError::InvalidSignature)
            ),
            "expected InvalidSignature, got {result:?}"
        );
    }

    /// The caller names an executor in the request body, but the stored quote
    /// is what submission actually uses — so validation has to follow the
    /// stored one, or a caller could pick an executor that validates nothing.
    #[tokio::test]
    async fn validation_follows_the_stored_executor_not_the_requested_one() {
        let swap = gasless_swap_needing_two_signatures();
        let swap_id = swap.id;

        let mut dao = MockDaoInterface::new();
        dao.expect_get_swap_by_id()
            .returning(move |_| Ok(Some(swap.clone())));
        dao.expect_claim_swap_for_submission()
            .never();

        let mut clients = MockSwapsClientsInterface::new();
        clients
            .expect_validate_signature()
            .withf(|executor, _, signature| {
                *executor == SwapExecutorType::ZeroExGasless && signature == "0xdeadbeef"
            })
            .once()
            .returning(|_, _, _| Err(SwapsClientError::InvalidSignaturePayload));
        clients
            .expect_submit_transaction()
            .never();

        let executor = SwapsExecutor::new(dao, clients);

        let result = executor
            .submit_with_signature(SwapSignatureParams {
                swap_id,
                // Bungee performs no signature validation of its own.
                swap_executor: SwapExecutorType::Bungee,
                signature: "0xdeadbeef".to_string(),
            })
            .await;

        assert!(
            matches!(
                result,
                Err(SwapsExecutorError::InvalidSignature)
            ),
            "expected InvalidSignature, got {result:?}"
        );
    }
}
