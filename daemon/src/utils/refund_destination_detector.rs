use crate::types::{Refund, Swap, Transaction, TransferDestinationParams};
use crate::dao::{DaoInterface, DAO};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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
            .map(|swap| swap.request.from_address.to_lowercase())
            .collect();

        transactions
            .retain(|trans| !swaps_addresses.contains(&trans.transfer_info.source_address.to_lowercase()));
    }

    #[tracing::instrument(skip_all)]
    async fn find_refund_destination(
        &self,
        refund: &Refund,
    ) -> Result<TransferDestinationParams, RefundDestinationDetectorError> {
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
            .map_err(|error| {
                tracing::warn!(
                    %error,
                    "Failed to fetch invoice incomnig swaps to detect refund address"
                );

                RefundDestinationDetectorError::DatabaseError
            })?;

        if let Some(params) = self.find_destination_in_swaps(&swaps, true) {
            return Ok(params)
        }

        // TODO: add search for cross-chain swap when it will be supported

        let mut transactions = self.dao
            .get_completed_transactions_by_invoice(refund.invoice_id)
            .await
            .map_err(|error| {
                tracing::warn!(
                    %error,
                    "Failed to fetch invoice incomnig transactions to detect refund address"
                );

                RefundDestinationDetectorError::DatabaseError
            })?;

        self.filter_out_swap_transactions(&mut transactions, &swaps);

        // TODO: now we just get the first one, later we'll have to also check them using arkhm API
        if let Some(trans) = transactions.first() {
            let params = TransferDestinationParams {
                destination_asset_id: trans.transfer_info.asset_id.clone(),
                destination_address: trans.transfer_info.source_address.clone(),
                destination_chain: trans.transfer_info.chain.into(),
            };

            return Ok(params)
        }

        Err(RefundDestinationDetectorError::NoAvailableDestination)
    }

    #[tracing::instrument(
        skip_all,
        fields(
            refund_id = %refund.id,
            invoice_id = %refund.invoice_id,
            source_address = %refund.source_address,
            destination_address = ?refund.destination_params.as_ref().map(|dp| &dp.destination_address),
        )
    )]
    async fn find_and_update_refund_destination(
        &self,
        refund: &Refund,
        with_destination: &mut Vec<Refund>,
    ) -> Result<(), RefundDestinationDetectorError> {
        match self.find_refund_destination(&refund).await {
            Ok(params) => {
                let refund = self.dao
                    .update_refund_destination_params(refund.id, params.clone())
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            %error,
                            destination_params = ?params,
                            "Failed to update refund destination params"
                        );
                        RefundDestinationDetectorError::DatabaseError
                    })?;

                with_destination.push(refund);
            },
            Err(error) => {
                let mut retry_meta = refund.retry_meta.clone();
                retry_meta.increment_retry(error.to_string());

                let _refund = self.dao
                    .update_refund_retry(refund.id, retry_meta, error.is_retriable())
                    .await
                    .map_err(|error| {
                        tracing::error!(
                            %error,
                            refund_id = %refund.id,
                            "Failed to update refund retry error. It might stuck in InProgress stataus"
                        );

                        RefundDestinationDetectorError::DatabaseError
                    })?;
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_refunds_with_destination(&self, limit: u32) -> Vec<Refund> {
        let refunds = match self.dao
            .get_pending_refunds(limit)
            .await
        {
            Ok(refunds) => refunds,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Failed to get pending refunds from database. Return an empty vector"
                );
                return vec![]
            }
        };

        let mut with_destination = Vec::with_capacity(refunds.len());

        for refund in refunds {
            if refund.destination_params.is_some() {
                with_destination.push(refund)
            } else {
                if let Err(error) = self.find_and_update_refund_destination(&refund, &mut with_destination).await {
                    tracing::warn!(
                        %error,
                        "Failed to find destination params for refund, it will be skipped"
                    )
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

#[cfg(test)]
mod tests  {
    use uuid::Uuid;
    use mockall::predicate::eq;

    use crate::dao::{MockDaoInterface, DaoSwapError, DaoTransactionError};
    use crate::types::{SwapChainType, default_swap, default_transaction, default_refund, ChainType};

    use super::*;

    #[test]
    fn test_find_destination_in_swaps() {
        let dao = MockDaoInterface::default();
        let detector = RefundDestinationDetector::new(dao);

        let mut swap_1 = default_swap(Uuid::new_v4());
        swap_1.request.from_chain = SwapChainType::Polygon;
        swap_1.request.to_chain = SwapChainType::Polygon;
        swap_1.request.from_token_address = "swap1_address".to_string();

        let swap_1_destination = TransferDestinationParams {
            destination_address: swap_1.request.from_address.clone(),
            destination_chain: swap_1.request.from_chain,
            destination_asset_id: swap_1.request.from_token_address.clone(),
        };

        let mut swap_2 = default_swap(Uuid::new_v4());
        swap_2.request.from_chain = SwapChainType::Polygon;
        swap_2.request.to_chain = SwapChainType::Polygon;
        swap_2.request.from_token_address = "swap2_address".to_string();

        let swap_2_destination = TransferDestinationParams {
            destination_address: swap_2.request.from_address.clone(),
            destination_chain: swap_2.request.from_chain,
            destination_asset_id: swap_2.request.from_token_address.clone(),
        };

        let mut swap_3 = default_swap(Uuid::new_v4());
        swap_3.request.from_chain = SwapChainType::Polygon;
        swap_3.request.to_chain = SwapChainType::Base;
        swap_3.request.from_token_address = "swap3_address".to_string();

        let swap_3_destination = TransferDestinationParams {
            destination_address: swap_3.request.from_address.clone(),
            destination_chain: swap_3.request.from_chain,
            destination_asset_id: swap_3.request.from_token_address.clone(),
        };

        let mut swaps = vec![swap_1, swap_2, swap_3];

        let result = detector.find_destination_in_swaps(&swaps, true);
        assert_eq!(result, Some(swap_1_destination));

        let result = detector.find_destination_in_swaps(&swaps, false);
        assert_eq!(result, Some(swap_3_destination));

        swaps.remove(0);

        let result = detector.find_destination_in_swaps(&swaps, true);
        assert_eq!(result, Some(swap_2_destination));

        swaps.remove(1);

        let result = detector.find_destination_in_swaps(&swaps, false);
        assert!(result.is_none());
    }

    #[test]
    fn test_filter_out_swap_transactions() {
        let dao = MockDaoInterface::default();
        let detector = RefundDestinationDetector::new(dao);

        let mut swap_1 = default_swap(Uuid::new_v4());
        swap_1.request.from_address = "swap1_address".to_string();

        let mut swap_2 = default_swap(Uuid::new_v4());
        swap_2.request.from_address = "swap2_address".to_string();

        let mut swap_3 = default_swap(Uuid::new_v4());
        swap_3.request.from_address = "swap3_address".to_string();

        let swaps = vec![swap_1, swap_2, swap_3];

        let mut transaction_1 = default_transaction(Uuid::new_v4());
        transaction_1.transfer_info.source_address = "swap1_address".to_string();

        let mut transaction_2 = default_transaction(Uuid::new_v4());
        transaction_2.transfer_info.source_address = "SWAP2_address".to_string();

        let transaction_3 = default_transaction(Uuid::new_v4());

        let mut transactions = vec![transaction_1, transaction_2, transaction_3.clone()];

        detector.filter_out_swap_transactions(&mut transactions, &swaps);

        assert_eq!(transactions.len(), 1);
        assert!(transactions.contains(&transaction_3));
    }

    #[tokio::test]
    async fn test_find_refund_destination() {
        let dao = MockDaoInterface::default();
        let mut detector = RefundDestinationDetector::new(dao);
        let invoice_id = Uuid::new_v4();
        let refund = default_refund(invoice_id);

        // Test case 1:
        // - Successful flow
        // - Destination found in swaps
        // Expectations:
        // - Single dao call, get swaps by invoice
        // - First returned swap return params
        {
            let mut returned_swap = default_swap(invoice_id);
            returned_swap.request.to_chain = returned_swap.request.from_chain;

            let expected_destination_params = TransferDestinationParams {
                destination_address: returned_swap.request.from_address.clone(),
                destination_chain: returned_swap.request.to_chain,
                destination_asset_id: returned_swap.request.from_token_address.clone(),
            };

            detector.dao
                .expect_get_completed_incoming_swaps_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| {
                    Ok(vec![returned_swap.clone()])
                });

            let result = detector
                .find_refund_destination(&refund)
                .await
                .unwrap();

            assert_eq!(result, expected_destination_params);
        }

        // Test case 2:
        // - Successful flow
        // - Destination found in transactions
        // Expectations:
        // - Get swaps by invoice dao call returns an empty vec
        // - Get transactions by invoice dao call returns single transaction
        // - Transaction params returned
        {
            let mut transaction = default_transaction(invoice_id);
            transaction.transfer_info.chain = ChainType::Polygon;
            transaction.transfer_info.source_address = "TEST".to_string();

            let expected_destination_params = TransferDestinationParams {
                destination_address: transaction.transfer_info.source_address.clone(),
                destination_chain: transaction.transfer_info.chain.into(),
                destination_asset_id: transaction.transfer_info.asset_id.clone(),
            };

            detector.dao
                .expect_get_completed_incoming_swaps_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Ok(vec![]));

            detector.dao
                .expect_get_completed_transactions_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Ok(vec![transaction.clone()]));

            let result = detector
                .find_refund_destination(&refund)
                .await
                .unwrap();

            assert_eq!(result, expected_destination_params);
        }

        // Test case 3:
        // - Unsuccessful flow
        // - Destination not found
        // Expectations:
        // - Get swaps by invoice dao call returns an empty vec
        // - Get transactions by invoice dao call returns an empty vec
        // - No available destination error
        {
            detector.dao
                .expect_get_completed_incoming_swaps_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Ok(vec![]));

            detector.dao
                .expect_get_completed_transactions_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Ok(vec![]));

            let result = detector
                .find_refund_destination(&refund)
                .await
                .unwrap_err();

            assert_eq!(result, RefundDestinationDetectorError::NoAvailableDestination);
        }

        // Test case 4:
        // - Unsuccessful flow
        // - Database error while request swaps
        // Expectations:
        // - Get swaps by invoice dao call returns an error
        // - No other dao calls
        // - Database error
        {
            detector.dao
                .expect_get_completed_incoming_swaps_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Err(DaoSwapError::DatabaseError));

            let result = detector
                .find_refund_destination(&refund)
                .await
                .unwrap_err();

            assert_eq!(result, RefundDestinationDetectorError::DatabaseError);
        }

        // Test case 5:
        // - Unsuccessful flow
        // - Database error while request transactions
        // Expectations:
        // - Get swaps by invoice dao call returns an empty vec
        // - Get transactions by invoice dao call returns an error
        // - Database error
        {
            detector.dao
                .expect_get_completed_incoming_swaps_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Ok(vec![]));

            detector.dao
                .expect_get_completed_transactions_by_invoice()
                .once()
                .with(eq(invoice_id))
                .returning(move |_| Err(DaoTransactionError::DatabaseError));

            let result = detector
                .find_refund_destination(&refund)
                .await
                .unwrap_err();

            assert_eq!(result, RefundDestinationDetectorError::DatabaseError);
        }
    }
}
