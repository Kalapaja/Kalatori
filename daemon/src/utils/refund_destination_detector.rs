use crate::types::{Refund, Swap, Transaction, TransferDestinationParams};
use crate::dao::{DaoInterface, DAO};

#[derive(Debug, thiserror::Error)]
pub enum RefundDestinationDetectorError {
    #[error("Database Error")]
    DatabaseError,
    #[error("No available destination found")]
    NoAvailableDestination,
}

impl RefundDestinationDetectorError {
    fn is_retriable(&self) -> bool {
        match self {
            RefundDestinationDetectorError::DatabaseError => true,
            RefundDestinationDetectorError::NoAvailableDestination => false,
        }
    }
}

#[derive(Clone)]
pub struct RefundDestinationDetector<D: DaoInterface + 'static = DAO> {
    dao: D,
}

impl<D: DaoInterface + 'static> RefundDestinationDetector<D> {
    pub fn new(dao: D) -> Self {
        Self {
            dao,
        }
    }

    fn find_destination_in_swaps(
        &self,
        swaps: &[Swap],
        same_chain: bool,
    ) -> Option<TransferDestinationParams> {
        for swap in swaps {
            if (swap.request.from_chain == swap.request.to_chain) == same_chain {
                let destination_params = TransferDestinationParams {
                    destination_address: swap.request.from_address.clone(),
                    destination_asset_id: swap.request.from_token_address.clone(),
                    destination_chain: swap.request.from_chain,
                };

                return Some(destination_params)
            }
        }

        None
    }

    fn filter_out_swap_transactions(
        &self,
        transactions: &mut Vec<Transaction>,
        swaps: &[Swap],
    ) {
        let swaps_addresses: Vec<_> = swaps
            .iter()
            .map(|swap| &swap.request.to_address)
            .collect();

        transactions
            .retain(|trans| !swaps_addresses.contains(&&trans.transfer_info.source_address));
    }

    async fn find_refund_destination(
        &self,
        refund: &Refund,
    ) -> Result<Refund, RefundDestinationDetectorError> {
        // At this moment we only support same chain refunds.
        //
        // Also some transfers might be sent not from user's wallet directly
        // but from CEX hot wallet, swap pool etc. If we'll send refund to such
        // address, money might be lost. Also we suppose that user can make
        // multiple payment transaction, from different sources.
        // The most reliable way to detect money source is our internal swap
        // records so prefer refunding to the swap source address if found.
        // Otherwise check incoming transactions using arkhm to detect if it's
        // user's wallet or something else.
        let swaps = self.dao
            .get_completed_incoming_swaps_by_invoice(refund.invoice_id)
            .await
            .map_err(|e| {
                // TODO: change error, add logs
                RefundDestinationDetectorError::DatabaseError
            })?;

        if let Some(params) = self.find_destination_in_swaps(&swaps, true) {
            return self.dao
                .update_refund_destination_params(refund.id, params)
                .await
                .map_err(|e| {
                    // TODO: add logs
                    RefundDestinationDetectorError::DatabaseError
                })
        }

        // TODO: add search for cross-chain swap when it will be supported

        let mut transactions = self.dao
            .get_completed_transactions_by_invoice(refund.invoice_id)
            .await
            .map_err(|e| {
                // TODO: change error, add logs
                RefundDestinationDetectorError::DatabaseError
            })?;

        self.filter_out_swap_transactions(&mut transactions, &swaps);

        if let Some(trans) = transactions.first() {
            let params = TransferDestinationParams {
                destination_asset_id: trans.transfer_info.asset_id.clone(),
                destination_address: trans.transfer_info.source_address.clone(),
                destination_chain: trans.transfer_info.chain.into(),
            };

            return self.dao
                .update_refund_destination_params(refund.id, params)
                .await
                .map_err(|e| {
                    // TODO: add logs
                    RefundDestinationDetectorError::DatabaseError
                })
        }

        Err(RefundDestinationDetectorError::NoAvailableDestination)
    }

    pub async fn get_refunds_with_destination(&self, limit: u32) -> Vec<Refund> {
        let refunds = match self.dao
            .get_pending_refunds(limit)
            .await
        {
            Ok(refunds) => refunds,
            Err(error) => {
                // TODO: add logs
                return vec![]
            }
        };

        let mut with_destination = Vec::with_capacity(refunds.len());

        for refund in refunds {
            if refund.destination_params.is_some() {
                with_destination.push(refund)
            } else {
                match self.find_refund_destination(&refund).await {
                    Ok(refund) => with_destination.push(refund),
                    Err(error) => {
                        // TODO: add logs
                        let mut retry_meta = refund.retry_meta.clone();
                        retry_meta.increment_retry(error.to_string());

                        if let Err(e) = self.dao
                            .update_refund_retry(refund.id, retry_meta, error.is_retriable())
                            .await
                        {
                            tracing::error!("");
                        }
                    }
                }
            }
        }

        with_destination
    }
}

#[cfg(test)]
mockall::mock!{
    pub RefundDestinationDetector<D: DaoInterface + 'static = DAO> {
        pub fn new(dao: D) -> Self;

        pub async fn get_refunds_with_destination(&self, limit: u32) -> Vec<Refund>;
    }

    impl<D: DaoInterface + 'static> Clone for RefundDestinationDetector<D> {
        fn clone(&self) -> Self;
    }
}
