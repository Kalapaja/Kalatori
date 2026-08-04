use rust_decimal::Decimal;

use crate::configs::PaymentsConfig;
use crate::dao::{
    DaoInterface,
    DaoTransactionError,
    DaoTransactionInterface,
};
use crate::types::{
    ChainType,
    GeneralTransactionId,
    IncomingTransaction,
    Invoice,
    InvoiceEventType,
    InvoiceStatus,
    InvoiceWithReceivedAmount,
    KalatoriEventExt,
    Payout,
    PublicInvoice,
    Refund,
    SwapChainType,
    TransferDestinationParams,
};

use super::InvoiceRegistry;

#[derive(Debug, thiserror::Error)]
pub enum TransactionsRecorderError {
    #[error("Database transaction failed")]
    DaoTransactionError,
    #[error("Transaction already exists")]
    TransactionDuplication {
        chain: ChainType,
        general_transaction_id: GeneralTransactionId,
    },
}

#[derive(Clone)]
pub struct TransactionsRecorder<D: DaoInterface + 'static> {
    dao: D,
    registry: InvoiceRegistry,
    config: PaymentsConfig,
}

impl<D: DaoInterface + 'static> TransactionsRecorder<D> {
    pub fn new(
        dao: D,
        registry: InvoiceRegistry,
        config: PaymentsConfig,
    ) -> Self {
        Self {
            dao,
            registry,
            config,
        }
    }

    async fn add_webhook_to_dao_transaction(
        &self,
        dao_transaction: &D::Transaction,
        public_invoice: PublicInvoice,
        event_type: InvoiceEventType,
    ) -> Result<(), TransactionsRecorderError> {
        let event = public_invoice
            .build_event(event_type)
            .into();

        dao_transaction
            .create_webhook_event(event)
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

        Ok(())
    }

    /// Schedule the payout for a paid invoice.
    ///
    /// Failing to build a payout destination must **not** abort the surrounding
    /// database transaction — that transaction is what records the incoming
    /// payment. Rolling it back loses the payment entirely, and the tracker
    /// rediscovers and re-fails the same transfer forever. So the two
    /// destination problems below are logged and skipped: the payment, the
    /// invoice status and the `Paid` webhook still commit, and the operator
    /// settles the payout by hand. Database failures stay fatal.
    async fn add_payout_to_dao_transaction(
        &self,
        dao_transaction: &D::Transaction,
        invoice: Invoice,
        amount: Decimal,
    ) -> Result<(), TransactionsRecorderError> {
        let chain = invoice.chain;

        // `validate_recipients` only checks the chains it is handed at startup,
        // so this lookup is not guaranteed to hit.
        let Some(payout_address) = self
            .config
            .recipient
            .get(&chain)
            .cloned()
        else {
            tracing::error!(
                %chain,
                invoice_id = %invoice.id,
                "No recipient configured for this chain, payment recorded but payout not scheduled, manual settlement required"
            );

            return Ok(())
        };

        // Asset Hub has no `SwapChainType` equivalent, so a payout destination
        // cannot be expressed for it.
        let Ok(destination_chain) = SwapChainType::try_from(chain) else {
            tracing::error!(
                %chain,
                invoice_id = %invoice.id,
                "Cannot build a payout destination for this chain, payment recorded but payout not scheduled, manual settlement required"
            );

            return Ok(())
        };

        let destination_params = TransferDestinationParams {
            destination_address: payout_address,
            destination_chain,
            destination_asset_id: invoice.asset_id.clone(),
        };

        let payout = Payout::from_invoice(invoice, destination_params, amount);

        dao_transaction
            .create_payout(payout)
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

        Ok(())
    }

    async fn store_transaction_in(
        &self,
        dao_transaction: &D::Transaction,
        transaction: IncomingTransaction,
        invoice_status: InvoiceStatus,
        total_received_amount: Decimal,
    ) -> Result<(), TransactionsRecorderError> {
        let invoice_id = transaction.invoice_id;

        dao_transaction
            .create_transaction(transaction.into())
            .await
            .map_err(|e| match e {
                DaoTransactionError::DuplicateTransaction {
                    chain,
                    general_transaction_id,
                } => TransactionsRecorderError::TransactionDuplication {
                    chain,
                    general_transaction_id,
                },
                _ => TransactionsRecorderError::DaoTransactionError,
            })?;

        let invoice = dao_transaction
            .update_invoice_status(invoice_id, invoice_status)
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

        let public_invoice = invoice
            .clone()
            .with_amount(total_received_amount)
            .into_public_invoice(&self.config.payment_url_base);

        if invoice_status == InvoiceStatus::Paid {
            // In case when invoice is just "Paid" without refund required,
            // put here total received amount which might be slightly higher or lower then
            // invoice amount
            self.add_payout_to_dao_transaction(
                dao_transaction,
                invoice,
                total_received_amount,
            )
            .await?;

            self.add_webhook_to_dao_transaction(
                dao_transaction,
                public_invoice,
                InvoiceEventType::Paid,
            )
            .await?;
        } else if invoice_status == InvoiceStatus::PartiallyPaid {
            self.add_webhook_to_dao_transaction(
                dao_transaction,
                public_invoice,
                InvoiceEventType::PartiallyPaid,
            )
            .await?;
        } else if invoice_status == InvoiceStatus::OverPaid {
            // In case when invoice is overpaid and refund is required, we schedule payout
            // with original invoice amount and refund with the rest amount
            let payout_amount = invoice.amount;
            let refund_amount = total_received_amount - payout_amount;

            self.add_payout_to_dao_transaction(
                dao_transaction,
                invoice.clone(),
                payout_amount,
            )
            .await?;

            let refund = Refund::from_invoice(invoice, refund_amount);

            dao_transaction
                .create_refund(refund)
                .await
                .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

            self.add_webhook_to_dao_transaction(
                dao_transaction,
                public_invoice,
                InvoiceEventType::Paid,
            )
            .await?;
        }

        Ok(())
    }

    #[cfg(test)]
    async fn store_transaction(
        &self,
        transaction: IncomingTransaction,
        invoice_status: InvoiceStatus,
        total_received_amount: Decimal,
    ) -> Result<(), TransactionsRecorderError> {
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

        self.store_transaction_in(
            &dao_transaction,
            transaction,
            invoice_status,
            total_received_amount,
        )
        .await?;

        dao_transaction
            .commit()
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)
    }

    pub(crate) async fn process_invoice_transaction_in(
        &self,
        dao_transaction: &D::Transaction,
        invoice: &InvoiceWithReceivedAmount,
        transaction: IncomingTransaction,
    ) -> Result<(InvoiceWithReceivedAmount, Decimal), TransactionsRecorderError> {
        let updated_received_amount =
            invoice.total_received_amount + transaction.transfer_info.amount;

        let underpayment_tolerance = self
            .config
            .get_asset_underpayment_tolerance(
                invoice.invoice.chain,
                &invoice.invoice.asset_id,
            );
        let min_paid_amount = invoice.invoice.amount - underpayment_tolerance;

        let overpayment_tolerance = self
            .config
            .get_asset_overpayment_tolerance(
                invoice.invoice.chain,
                &invoice.invoice.asset_id,
            );
        let max_paid_amount = invoice.invoice.amount + overpayment_tolerance;

        let is_underpaid = updated_received_amount < min_paid_amount;
        let is_overpaid = updated_received_amount > max_paid_amount;

        let updated_status = if !is_underpaid && !is_overpaid {
            InvoiceStatus::Paid
        } else if is_underpaid {
            InvoiceStatus::PartiallyPaid
        } else {
            InvoiceStatus::OverPaid
        };

        self.store_transaction_in(
            dao_transaction,
            transaction,
            updated_status,
            updated_received_amount,
        )
        .await?;

        let mut updated_invoice = invoice.clone();
        updated_invoice.invoice.status = updated_status;
        updated_invoice.total_received_amount = updated_received_amount;

        Ok((updated_invoice, min_paid_amount))
    }

    pub(crate) async fn apply_recorded_invoice_update(
        &self,
        invoice: &mut InvoiceWithReceivedAmount,
        updated_invoice: InvoiceWithReceivedAmount,
        min_paid_amount: Decimal,
    ) {
        let updated_status = updated_invoice.invoice.status;
        let updated_received_amount = updated_invoice.total_received_amount;

        match updated_status {
            InvoiceStatus::Paid | InvoiceStatus::OverPaid => {
                tracing::info!(
                    invoice_id = %updated_invoice.invoice.id,
                    filled_amount = %updated_received_amount,
                    min_fill_amount = %min_paid_amount,
                    "Invoice has been paid, removing from registry, stop monitoring"
                );

                self.registry
                    .remove_invoice(&updated_invoice.invoice.id)
                    .await;
            },
            InvoiceStatus::PartiallyPaid => {
                tracing::info!(
                    invoice_id = %updated_invoice.invoice.id,
                    filled_amount = %updated_received_amount,
                    min_fill_amount = %min_paid_amount,
                    "Invoice has been partially paid, updating filled amount in registry"
                );

                self.registry
                    .update_filled_amount(
                        &updated_invoice.invoice.id,
                        updated_received_amount,
                        updated_status,
                    )
                    .await;
            },
            _ => {
                tracing::error!(
                    invoice_id = %updated_invoice.invoice.id,
                    invoice_status = %updated_status,
                    "Recorded invoice transaction produced an unexpected status"
                );
                return;
            },
        }

        *invoice = updated_invoice;
    }

    #[tracing::instrument(skip_all)]
    pub async fn process_invoice_transaction(
        &self,
        invoice: &mut InvoiceWithReceivedAmount,
        transaction: IncomingTransaction,
    ) -> Result<(), TransactionsRecorderError> {
        // TODO: we'll need to handle case when invoice has been already paid (and not
        // monitored anymore) but the user accidently sent money to this
        // address. We'll be able to init balance and transactions refetch and
        // will need to create only refund but not payout. So we'll need to respect the
        // invoice status and probably allow transition `Paid` -> `Overpaid`.
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

        let (updated_invoice, min_paid_amount) = match self
            .process_invoice_transaction_in(&dao_transaction, invoice, transaction)
            .await
        {
            Ok(update) => update,
            Err(TransactionsRecorderError::TransactionDuplication {
                chain,
                general_transaction_id,
            }) => {
                tracing::debug!(
                    invoice_id = %invoice.invoice.id,
                    ?chain,
                    transaction_id = ?general_transaction_id,
                    "Transaction is already presented in database, skip it"
                );

                return Err(
                    TransactionsRecorderError::TransactionDuplication {
                        chain,
                        general_transaction_id,
                    },
                );
            },
            Err(TransactionsRecorderError::DaoTransactionError) => {
                tracing::error!(
                    invoice_id = %invoice.invoice.id,
                    "Error while storing transaction for invoice"
                );

                return Err(TransactionsRecorderError::DaoTransactionError);
            },
        };

        if dao_transaction.commit().await.is_err() {
            tracing::error!(
                invoice_id = %invoice.invoice.id,
                "Error while committing transaction for invoice"
            );
            return Err(TransactionsRecorderError::DaoTransactionError);
        }

        self.apply_recorded_invoice_update(
            invoice,
            updated_invoice,
            min_paid_amount,
        )
        .await;

        Ok(())
    }
}

#[cfg(test)]
mockall::mock! {
    pub TransactionsRecorder<D: DaoInterface + 'static> {
        pub fn new(
            dao: D,
            registry: InvoiceRegistry,
            config: PaymentsConfig,
        ) -> Self;

        pub async fn process_invoice_transaction(
            &self,
            invoice: &mut InvoiceWithReceivedAmount,
            transaction: IncomingTransaction,
        ) -> Result<(), TransactionsRecorderError>;

        pub async fn process_invoice_transaction_in(
            &self,
            dao_transaction: &D::Transaction,
            invoice: &InvoiceWithReceivedAmount,
            transaction: IncomingTransaction,
        ) -> Result<(InvoiceWithReceivedAmount, Decimal), TransactionsRecorderError>;

        pub async fn apply_recorded_invoice_update(
            &self,
            invoice: &mut InvoiceWithReceivedAmount,
            updated_invoice: InvoiceWithReceivedAmount,
            min_paid_amount: Decimal,
        );
    }

    impl<D: DaoInterface + 'static> Clone for TransactionsRecorder<D> {
        fn clone(&self) -> Self;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use kalatori_client::types::KalatoriEvent;
    use mockall::predicate::eq;

    use crate::configs::SlippageParams;
    use crate::dao::{
        MockDaoInterface,
        MockDaoTransactionInterface,
    };
    use crate::types::{
        Invoice,
        default_incoming_transaction,
        default_invoice,
    };

    use super::*;

    fn default_payments_config() -> PaymentsConfig {
        PaymentsConfig {
            default_chain: ChainType::Polygon,
            default_asset_id: HashMap::from([(
                ChainType::Polygon,
                "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(),
            )]),
            invoice_lifetime_millis: 600_000,
            recipient: HashMap::from([(
                ChainType::Polygon,
                "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9".to_string(),
            )]),
            payment_url_base: "https://payments.example.com".to_string(),
            slippage_params: HashMap::new(),
        }
    }

    fn partially_paid_dao_transaction_mock(
        invoice: &Invoice,
        amount: Decimal,
    ) -> MockDaoTransactionInterface {
        let invoice_id = invoice.id;
        let status = InvoiceStatus::PartiallyPaid;
        let expected_amount = amount;
        let expected_event_type = InvoiceEventType::PartiallyPaid;

        let returning_invoice = Invoice {
            status,
            ..invoice.clone()
        };

        let mut dao_transaction = MockDaoTransactionInterface::default();

        dao_transaction
            .expect_create_transaction()
            .once()
            .returning(Ok);

        dao_transaction
            .expect_update_invoice_status()
            .once()
            .with(eq(invoice_id), eq(status))
            .returning(move |_, _| Ok(returning_invoice.clone()));

        dao_transaction
            .expect_create_webhook_event()
            .once()
            .withf(move |event| {
                let generic_event: KalatoriEvent =
                    serde_json::from_value(event.payload.clone()).unwrap();

                #[expect(irrefutable_let_patterns)]
                let KalatoriEvent::Invoice(invoice_event) = generic_event else {
                    return false;
                };

                invoice_event.event_type == expected_event_type
                    && event.entity_id == invoice_id
                    && invoice_event
                        .payload
                        .total_received_amount
                        == expected_amount
            })
            .returning(Ok);

        dao_transaction
            .expect_commit()
            .once()
            .returning(|| Ok(()));

        dao_transaction
    }

    fn paid_dao_transaction_mock(
        invoice: &Invoice,
        amount: Decimal,
    ) -> MockDaoTransactionInterface {
        let invoice_id = invoice.id;
        let status = InvoiceStatus::Paid;
        let expected_amount = amount;
        let expected_event_type = InvoiceEventType::Paid;

        let returning_invoice = Invoice {
            status,
            ..invoice.clone()
        };

        let mut dao_transaction = MockDaoTransactionInterface::default();

        dao_transaction
            .expect_create_transaction()
            .once()
            .returning(Ok);

        dao_transaction
            .expect_update_invoice_status()
            .once()
            .with(eq(invoice_id), eq(status))
            .returning(move |_, _| Ok(returning_invoice.clone()));

        dao_transaction
            .expect_create_payout()
            .once()
            .withf(move |p| p.amount == amount)
            .returning(Ok);

        dao_transaction
            .expect_create_webhook_event()
            .once()
            .withf(move |event| {
                let generic_event: KalatoriEvent =
                    serde_json::from_value(event.payload.clone()).unwrap();
                #[expect(irrefutable_let_patterns)]
                let KalatoriEvent::Invoice(invoice_event) = generic_event else {
                    return false
                };

                invoice_event.event_type == expected_event_type
                    && event.entity_id == invoice_id
                    && invoice_event
                        .payload
                        .total_received_amount
                        == expected_amount
            })
            .returning(Ok);

        dao_transaction
            .expect_commit()
            .once()
            .returning(|| Ok(()));

        dao_transaction
    }

    fn overpaid_dao_transaction_mock(
        invoice: &Invoice,
        amount: Decimal,
        payout_amount: Decimal,
        refund_amount: Decimal,
    ) -> MockDaoTransactionInterface {
        let invoice_id = invoice.id;
        let status = InvoiceStatus::OverPaid;
        let expected_amount = amount;
        let expected_event_type = InvoiceEventType::Paid;

        let returning_invoice = Invoice {
            status,
            ..invoice.clone()
        };

        let mut dao_transaction = MockDaoTransactionInterface::default();

        dao_transaction
            .expect_create_transaction()
            .once()
            .returning(Ok);

        dao_transaction
            .expect_update_invoice_status()
            .once()
            .with(eq(invoice_id), eq(status))
            .returning(move |_, _| Ok(returning_invoice.clone()));

        dao_transaction
            .expect_create_payout()
            .once()
            .withf(move |p| p.amount == payout_amount)
            .returning(Ok);

        dao_transaction
            .expect_create_refund()
            .withf(move |r| r.amount == refund_amount)
            .once()
            .returning(Ok);

        dao_transaction
            .expect_create_webhook_event()
            .once()
            .withf(move |event| {
                let generic_event: KalatoriEvent =
                    serde_json::from_value(event.payload.clone()).unwrap();
                #[expect(irrefutable_let_patterns)]
                let KalatoriEvent::Invoice(invoice_event) = generic_event else {
                    return false
                };

                invoice_event.event_type == expected_event_type
                    && event.entity_id == invoice_id
                    && invoice_event
                        .payload
                        .total_received_amount
                        == expected_amount
            })
            .returning(Ok);

        dao_transaction
            .expect_commit()
            .once()
            .returning(|| Ok(()));

        dao_transaction
    }

    #[tokio::test]
    async fn test_store_transaction() {
        let config = default_payments_config();
        let dao = MockDaoInterface::default();

        let invoice = default_invoice();
        let invoice_id = invoice.id;
        let invoice_with_amount = invoice
            .clone()
            .with_amount(Decimal::ZERO);

        let registry = InvoiceRegistry::new();
        registry
            .add_invoice(invoice_with_amount)
            .await;

        let mut recorder = TransactionsRecorder::new(dao, registry, config);

        // Test case 1:
        // - Successful flow
        // - PartiallyPaid status
        // - Expectations:
        //   - Transaction created
        //   - Invoice status updated
        //   - Webhook event created
        {
            // Setup test
            let status = InvoiceStatus::PartiallyPaid;
            let transaction = default_incoming_transaction(invoice_id);
            // in this method it should only be included into event
            // the method doesn't check it in any way so we can put any value here
            let amount = Decimal::ONE_HUNDRED;

            let dao_transaction = partially_paid_dao_transaction_mock(&invoice, amount);

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .store_transaction(transaction, status, amount)
                .await;
            // We need to ensure that we received successful result only, the rest checks
            // are made in dao mocks
            assert!(result.is_ok());
        }

        // Test case 2:
        // - Successful flow
        // - Paid status
        // - Expectations:
        //   - Transaction created
        //   - Invoice status updated
        //   - Payout created
        //   - Webhook event created
        {
            // Setup test
            let status = InvoiceStatus::Paid;
            let transaction = default_incoming_transaction(invoice_id);
            let amount = Decimal::ONE_THOUSAND;

            let dao_transaction = paid_dao_transaction_mock(&invoice, amount);

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .store_transaction(transaction, status, amount)
                .await;
            // We need to ensure that we received successful result only, the rest checks
            // are made in dao mocks
            assert!(result.is_ok());
        }

        // Test case 3
        // - Unsuccessful flow
        // - Duplicated transaction error
        // - Expectations:
        //   - Error on transaction creation
        //   - No other dao/dao_transaction methods called
        {
            // Setup
            let status = InvoiceStatus::Paid;
            let transaction = default_incoming_transaction(invoice_id);
            let amount = Decimal::ONE_THOUSAND;

            let mut dao_transaction = MockDaoTransactionInterface::default();

            dao_transaction
                .expect_create_transaction()
                .once()
                .returning(|trans| {
                    Err(
                        DaoTransactionError::DuplicateTransaction {
                            chain: trans.transfer_info.chain,
                            general_transaction_id: trans.transaction_id,
                        },
                    )
                });

            // No need to setup additional checks that any methods wasn't called
            // If they will be called after some code updates, mockall will raise an error
            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .store_transaction(transaction.clone(), status, amount)
                .await;
            // We need to ensure that we received successful result only, the rest checks
            // are made in dao mocks
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                TransactionsRecorderError::TransactionDuplication {
                    chain,
                    general_transaction_id
                } if chain == transaction.transfer_info.chain && general_transaction_id == transaction.transaction_id
            ));
        }

        // Test case 4:
        // - Unsuccessful flow
        // - Database error
        // - Expectations:
        //   - Error on transaction creation
        //   - No other dao/dao_transaction methods called
        {
            // Setup test
            let status = InvoiceStatus::Paid;
            let transaction = default_incoming_transaction(invoice_id);
            let amount = Decimal::ONE_THOUSAND;

            let mut dao_transaction = MockDaoTransactionInterface::default();

            dao_transaction
                .expect_create_transaction()
                .once()
                .returning(|_| Err(DaoTransactionError::DatabaseError));

            // No need to setup additional checks that any methods wasn't called
            // If they will be called after some code updates, mockall will raise an error
            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .store_transaction(transaction.clone(), status, amount)
                .await;
            // We need to ensure that we received successful result only, the rest checks
            // are made in dao mocks
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                TransactionsRecorderError::DaoTransactionError
            ));
        }
    }

    #[tokio::test]
    async fn test_process_invoice_transaction() {
        let config = default_payments_config();
        let dao = MockDaoInterface::default();

        let registry = InvoiceRegistry::new();
        let mut recorder = TransactionsRecorder::new(dao, registry.clone(), config);

        // Test case 1:
        // - Successful flow
        // - Partially paid
        // - Expectations:
        //   - Invoice status updated
        //   - Invoice total received amount updated
        //   - Respective database calls
        //   - Invoice remains in registry with updated total received amount and status
        {
            // Setup test
            let invoice = Invoice {
                amount: Decimal::ONE_THOUSAND,
                ..default_invoice()
            };

            let invoice_id = invoice.id;
            let mut invoice_with_amount = invoice
                .clone()
                .with_amount(Decimal::ZERO);

            registry
                .add_invoice(invoice_with_amount.clone())
                .await;

            let mut transaction = default_incoming_transaction(invoice_id);
            transaction.transfer_info.amount = Decimal::ONE_HUNDRED;
            let amount = transaction.transfer_info.amount;

            let dao_transaction = partially_paid_dao_transaction_mock(&invoice, amount);

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(&mut invoice_with_amount, transaction)
                .await;
            assert!(result.is_ok());

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::PartiallyPaid
            );
            assert_eq!(
                invoice_with_amount.total_received_amount,
                amount
            );
            let invoice_in_registry = registry
                .get_invoice(&invoice_id)
                .await
                .unwrap();
            assert_eq!(
                invoice_in_registry.total_received_amount,
                amount
            );
            assert_eq!(
                invoice_in_registry.invoice.status,
                InvoiceStatus::PartiallyPaid
            );
        }

        // Test case 2:
        // - Successful flow
        // - Paid
        // - Expectations:
        //   - Invoice status updated
        //   - Invoice total received amount updated
        //   - Respective database calls
        //   - Invoice is removed from registry
        {
            // Setup test
            let invoice = Invoice {
                amount: Decimal::ONE_THOUSAND,
                ..default_invoice()
            };

            let invoice_id = invoice.id;
            let mut invoice_with_amount = invoice
                .clone()
                .with_amount(Decimal::ONE_HUNDRED);

            registry
                .add_invoice(invoice_with_amount.clone())
                .await;

            let mut transaction = default_incoming_transaction(invoice_id);
            transaction.transfer_info.amount = Decimal::ONE_HUNDRED * Decimal::new(9, 0);
            // A hundred from previous already existing amount + 900 from current one
            let expected_amount = Decimal::ONE_THOUSAND;

            let dao_transaction = paid_dao_transaction_mock(&invoice, expected_amount);

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(&mut invoice_with_amount, transaction)
                .await;
            assert!(result.is_ok());

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::Paid
            );
            assert_eq!(
                invoice_with_amount.total_received_amount,
                expected_amount
            );
            let invoice_in_registry = registry.get_invoice(&invoice_id).await;
            assert!(invoice_in_registry.is_none());
        }

        // Test case 3:
        // - Successful flow
        // - Check underpayment tolerance
        // - Paid
        // - Expectations:
        //   - Invoice status updated
        //   - Invoice total received amount updated
        //   - Respective database calls
        //   - Invoice is removed from registry
        {
            // Setup test

            let invoice = Invoice {
                amount: Decimal::ONE_THOUSAND,
                ..default_invoice()
            };
            let invoice_id = invoice.id;

            recorder.config.slippage_params.insert(
                invoice.chain,
                HashMap::from([(
                    invoice.asset_id.clone(),
                    SlippageParams {
                        underpayment_tolerance: Decimal::ONE_HUNDRED,
                        overpayment_tolerance: Decimal::ZERO,
                    },
                )]),
            );

            let mut invoice_with_amount = invoice
                .clone()
                .with_amount(Decimal::ZERO);

            let mut transaction = default_incoming_transaction(invoice_id);
            transaction.transfer_info.amount = Decimal::ONE_HUNDRED * Decimal::new(9, 0);
            let expected_amount = transaction.transfer_info.amount;

            let dao_transaction = paid_dao_transaction_mock(&invoice, expected_amount);

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(&mut invoice_with_amount, transaction)
                .await;
            assert!(result.is_ok());

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::Paid
            );
            assert_eq!(
                invoice_with_amount.total_received_amount,
                expected_amount
            );
            let invoice_in_registry = registry.get_invoice(&invoice_id).await;
            assert!(invoice_in_registry.is_none());
        }

        // Test case 4:
        // - Successful flow
        // - Check overpayment tolerance
        // - Paid
        // - Expectations:
        //   - Invoice status updated
        //   - Invoice total received amount updated
        //   - Respective database calls
        //   - Invoice is removed from registry
        {
            // Setup test

            let invoice = Invoice {
                amount: Decimal::ONE_THOUSAND,
                ..default_invoice()
            };
            let invoice_id = invoice.id;

            recorder.config.slippage_params.insert(
                invoice.chain,
                HashMap::from([(
                    invoice.asset_id.clone(),
                    SlippageParams {
                        underpayment_tolerance: Decimal::ZERO,
                        overpayment_tolerance: Decimal::ONE_HUNDRED,
                    },
                )]),
            );

            let mut invoice_with_amount = invoice
                .clone()
                .with_amount(Decimal::ZERO);

            let mut transaction = default_incoming_transaction(invoice_id);
            transaction.transfer_info.amount = Decimal::ONE_HUNDRED * Decimal::new(11, 0);
            let expected_amount = transaction.transfer_info.amount;

            let dao_transaction = paid_dao_transaction_mock(&invoice, expected_amount);

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(&mut invoice_with_amount, transaction)
                .await;
            assert!(result.is_ok());

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::Paid
            );
            assert_eq!(
                invoice_with_amount.total_received_amount,
                expected_amount
            );
            let invoice_in_registry = registry.get_invoice(&invoice_id).await;
            assert!(invoice_in_registry.is_none());
        }

        // Test case 5:
        // - Successful flow
        // - OverPaid
        // - Expectations:
        //   - Invoice status updated
        //   - Invoice total received amount updated
        //   - Refund created
        //   - Respective database calls
        //   - Invoice is removed from registry
        {
            // Setup test

            let invoice = Invoice {
                amount: Decimal::ONE_HUNDRED,
                ..default_invoice()
            };
            let invoice_id = invoice.id;

            recorder.config.slippage_params.insert(
                invoice.chain,
                HashMap::from([(
                    invoice.asset_id.clone(),
                    SlippageParams {
                        underpayment_tolerance: Decimal::ZERO,
                        overpayment_tolerance: Decimal::ZERO,
                    },
                )]),
            );

            let mut invoice_with_amount = invoice
                .clone()
                .with_amount(Decimal::ZERO);

            let mut transaction = default_incoming_transaction(invoice_id);
            transaction.transfer_info.amount = Decimal::ONE_HUNDRED * Decimal::new(3, 0);
            let expected_amount = transaction.transfer_info.amount;

            let dao_transaction = overpaid_dao_transaction_mock(
                &invoice,
                expected_amount,
                Decimal::ONE_HUNDRED,
                Decimal::ONE_HUNDRED * Decimal::new(2, 0),
            );

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(&mut invoice_with_amount, transaction)
                .await;
            assert!(result.is_ok());

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::OverPaid
            );
            assert_eq!(
                invoice_with_amount.total_received_amount,
                expected_amount
            );
            let invoice_in_registry = registry.get_invoice(&invoice_id).await;
            assert!(invoice_in_registry.is_none());
        }

        // Shared setup for test cases 6 and 7
        let invoice = Invoice {
            amount: Decimal::ONE_THOUSAND,
            ..default_invoice()
        };
        let invoice_id = invoice.id;
        let mut invoice_with_amount = invoice
            .clone()
            .with_amount(Decimal::ZERO);

        registry
            .add_invoice(invoice_with_amount.clone())
            .await;

        let mut transaction = default_incoming_transaction(invoice_id);
        transaction.transfer_info.amount = Decimal::ONE_HUNDRED;

        // Test case 6:
        // - Unsuccessful flow
        // - Database error
        // - Expectations:
        //   - Invoice status not updated
        //   - Invoice total received amount not updated
        //   - Invoice remains in registry
        {
            // Setup test
            let mut dao_transaction = MockDaoTransactionInterface::default();

            dao_transaction
                .expect_create_transaction()
                .once()
                .returning(|_| Err(DaoTransactionError::DatabaseError));

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(
                    &mut invoice_with_amount,
                    transaction.clone(),
                )
                .await;
            assert!(result.is_err());

            assert!(matches!(
                result.unwrap_err(),
                TransactionsRecorderError::DaoTransactionError
            ));

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::Waiting
            );
            assert!(
                invoice_with_amount
                    .total_received_amount
                    .is_zero()
            );
            let invoice_in_registry = registry
                .get_invoice(&invoice_id)
                .await
                .unwrap();
            assert!(
                invoice_in_registry
                    .total_received_amount
                    .is_zero()
            );
        }

        // Test case 7:
        // - Unsuccessful flow
        // - Transaction duplicate error
        // - Expectations:
        //   - Invoice status not updated
        //   - Invoice total received amount not updated
        //   - Invoice remains in registry
        {
            // Setup test
            let mut dao_transaction = MockDaoTransactionInterface::default();

            dao_transaction
                .expect_create_transaction()
                .once()
                .returning(|trans| {
                    Err(
                        DaoTransactionError::DuplicateTransaction {
                            chain: trans.transfer_info.chain,
                            general_transaction_id: trans.transaction_id,
                        },
                    )
                });

            recorder
                .dao
                .expect_begin_transaction()
                .once()
                .return_once(move || Ok(dao_transaction));

            // Test and assert
            let result = recorder
                .process_invoice_transaction(
                    &mut invoice_with_amount,
                    transaction.clone(),
                )
                .await;
            assert!(result.is_err());

            assert!(matches!(
                result.unwrap_err(),
                TransactionsRecorderError::TransactionDuplication {
                    chain,
                    general_transaction_id,
                } if chain == transaction.transfer_info.chain && general_transaction_id == transaction.transaction_id
            ));

            assert_eq!(
                invoice_with_amount.invoice.status,
                InvoiceStatus::Waiting
            );
            assert!(
                invoice_with_amount
                    .total_received_amount
                    .is_zero()
            );
            let invoice_in_registry = registry
                .get_invoice(&invoice_id)
                .await
                .unwrap();
            assert!(
                invoice_in_registry
                    .total_received_amount
                    .is_zero()
            );
        }
    }

    #[tokio::test]
    async fn transaction_scoped_adjustment_records_the_shortfall_once() {
        let invoice = Invoice {
            amount: Decimal::ONE_HUNDRED,
            chain: ChainType::PolkadotAssetHub,
            ..default_invoice()
        };
        let invoice_id = invoice.id;
        let recorded_amount = Decimal::new(3, 0);
        let adjustment = Decimal::new(7, 0);
        let expected_total = Decimal::TEN;
        let invoice_with_amount = invoice
            .clone()
            .with_amount(recorded_amount);

        let mut dao_transaction = MockDaoTransactionInterface::default();
        dao_transaction
            .expect_create_transaction()
            .once()
            .returning(Ok);

        let updated_invoice = Invoice {
            status: InvoiceStatus::PartiallyPaid,
            ..invoice
        };
        dao_transaction
            .expect_update_invoice_status()
            .once()
            .with(
                eq(invoice_id),
                eq(InvoiceStatus::PartiallyPaid),
            )
            .return_once(move |_, _| Ok(updated_invoice));
        dao_transaction
            .expect_create_webhook_event()
            .once()
            .returning(Ok);

        // The caller owns this transaction so the scoped recorder must neither
        // begin another one nor commit before reconciliation has finished.
        let recorder = TransactionsRecorder::new(
            MockDaoInterface::default(),
            InvoiceRegistry::new(),
            default_payments_config(),
        );

        let mut transaction = default_incoming_transaction(invoice_id);
        transaction.transfer_info.chain = ChainType::PolkadotAssetHub;
        transaction.transfer_info.amount = adjustment;
        transaction.transaction_id = GeneralTransactionId {
            block_number: None,
            position_in_block: None,
            tx_hash: None,
        };

        let (result, _) = recorder
            .process_invoice_transaction_in(
                &dao_transaction,
                &invoice_with_amount,
                transaction,
            )
            .await
            .unwrap();

        assert_eq!(
            result.total_received_amount,
            expected_total
        );
        assert_eq!(
            result.invoice.status,
            InvoiceStatus::PartiallyPaid
        );
    }

    /// A payout that cannot be built must not roll back the payment.
    ///
    /// Asset Hub has no `SwapChainType` equivalent, so no payout destination
    /// can be expressed for it. When that failure propagated out of
    /// `add_payout_to_dao_transaction` it aborted the surrounding transaction
    /// before `commit`, so the incoming transaction, the invoice status and the
    /// `Paid` webhook were all discarded — and the caller only logs, so the
    /// tracker rediscovered and re-failed the same transfer forever. The
    /// payment has to commit; only the payout is skipped.
    #[tokio::test]
    async fn a_payout_that_cannot_be_built_still_commits_the_payment() {
        let mut config = default_payments_config();
        config.default_chain = ChainType::PolkadotAssetHub;
        config.recipient.insert(
            ChainType::PolkadotAssetHub,
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        );

        let invoice = Invoice {
            chain: ChainType::PolkadotAssetHub,
            ..default_invoice()
        };
        let invoice_id = invoice.id;
        let amount = Decimal::ONE_THOUSAND;

        let returning_invoice = Invoice {
            status: InvoiceStatus::Paid,
            ..invoice.clone()
        };

        let mut dao_transaction = MockDaoTransactionInterface::default();

        dao_transaction
            .expect_create_transaction()
            .once()
            .returning(Ok);

        dao_transaction
            .expect_update_invoice_status()
            .once()
            .with(eq(invoice_id), eq(InvoiceStatus::Paid))
            .returning(move |_, _| Ok(returning_invoice.clone()));

        // The payout is the part that cannot be built.
        dao_transaction
            .expect_create_payout()
            .never();

        // The merchant still has to be told the invoice was paid...
        dao_transaction
            .expect_create_webhook_event()
            .once()
            .returning(Ok);

        // ...and above all, the work must actually be committed.
        dao_transaction
            .expect_commit()
            .once()
            .returning(|| Ok(()));

        let registry = InvoiceRegistry::new();
        let mut recorder = TransactionsRecorder::new(
            MockDaoInterface::default(),
            registry,
            config,
        );

        recorder
            .dao
            .expect_begin_transaction()
            .once()
            .return_once(move || Ok(dao_transaction));

        let result = recorder
            .store_transaction(
                default_incoming_transaction(invoice_id),
                InvoiceStatus::Paid,
                amount,
            )
            .await;

        assert!(
            result.is_ok(),
            "recording the payment must succeed even with no payout destination, got {result:?}"
        );
    }
}
