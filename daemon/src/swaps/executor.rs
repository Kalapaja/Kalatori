use uuid::Uuid;

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

#[expect(dead_code)]
pub enum SwapsExecutorError {
    // TODO: refactor
    QuoteRequestFailed,
    SwapNotFound { swap_id: Uuid },
    InvoiceNotFound { invoice_id: Uuid },
    DatabaseError,
}

impl From<SwapsClientError> for SwapsExecutorError {
    fn from(_value: SwapsClientError) -> Self {
        // TODO: refactor
        SwapsExecutorError::QuoteRequestFailed
    }
}

#[derive(Clone)]
pub struct SwapsExecutor<D: DaoInterface + 'static = DAO> {
    dao: D,
    clients: SwapsClients,
}

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

        Ok(created)
    }

    pub async fn submit_with_signature(
        &self,
        swap_signature: SwapSignatureParams,
    ) -> Result<Swap, SwapsExecutorError> {
        if !matches!(
            swap_signature.swap_executor,
            SwapExecutorType::Bungee | SwapExecutorType::ZeroEx
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

        // TODO: In case of error need to check an error thoroughly.
        // If it's problem with signature, we can mark it as failed.
        // If it's some kind of network error, we can retry it.
        // In any way we have to understand if it was received by bungee
        // and is being processed to avoid double-payments or just missing
        // the transaction.
        let transaction_hash = self
            .clients
            .submit_transaction(
                swap.request.swap_executor,
                &swap.swap_details,
            )
            .await?;

        self.dao
            .update_swap_submitted_with_hash(swap_signature.swap_id, transaction_hash)
            .await
            .map_err(|e| match e {
                DaoSwapError::NotFound {
                    swap_id,
                } => SwapsExecutorError::SwapNotFound {
                    swap_id,
                },
                _ => SwapsExecutorError::DatabaseError,
            })?;

        Ok(swap)
    }

    /// Mark swap as `Submitted` in database. Use this method for swaps which
    /// has been executed inside this service by either sending some API
    /// requests to executor or sent to blockhain directly. For swaps which has
    /// been sent on front-end use `update_swap_submitted_on_front_end`
    /// method.
    #[expect(dead_code)]
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
