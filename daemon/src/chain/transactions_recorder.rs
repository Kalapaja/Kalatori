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
    InvoiceEventType,
    InvoiceStatus,
    InvoiceWithReceivedAmount,
    KalatoriEventExt,
    Payout,
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

        let invoice_id = transaction.invoice_id;
        let chain = transaction.transfer_info.chain;

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
            let payout = Payout::from_invoice(
                invoice,
                self.config
                    .recipient
                    .get(&chain)
                    // unwrap should be safe cause on program startup we check
                    // that recipient is set for all required chains
                    .unwrap()
                    .clone(),
            );

            dao_transaction
                .create_payout(payout)
                .await
                .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

            let event = public_invoice
                .build_event(InvoiceEventType::Paid)
                .into();

            dao_transaction
                .create_webhook_event(event)
                .await
                .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;
        } else if invoice_status == InvoiceStatus::PartiallyPaid {
            let event = public_invoice
                .build_event(InvoiceEventType::PartiallyPaid)
                .into();

            dao_transaction
                .create_webhook_event(event)
                .await
                .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;
        }

        // TODO: handle overpayments

        dao_transaction
            .commit()
            .await
            .map_err(|_e| TransactionsRecorderError::DaoTransactionError)?;

        Ok(())
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
        let &mut InvoiceWithReceivedAmount {
            ref mut invoice,
            ref mut total_received_amount,
        } = invoice;

        let updated_received_amount = *total_received_amount + transaction.transfer_info.amount;

        let underpayment_tolerance = self
            .config
            .get_asset_underpayment_tolerance(invoice.chain, &invoice.asset_id);
        let min_paid_amount = invoice.amount - underpayment_tolerance;

        // TODO: handle overpayments
        let updated_status = if updated_received_amount >= min_paid_amount {
            InvoiceStatus::Paid
        } else {
            InvoiceStatus::PartiallyPaid
        };

        match self
            .store_transaction(
                transaction,
                updated_status,
                updated_received_amount,
            )
            .await
        {
            Ok(()) if updated_status == InvoiceStatus::Paid => {
                tracing::info!(
                    invoice_id = %invoice.id,
                    filled_amount = %updated_received_amount,
                    min_fill_amount = %min_paid_amount,
                    "Invoice has been paid, removing from registry, stop monitoring"
                );

                self.registry
                    .remove_invoice(&invoice.id)
                    .await;

                invoice.status = updated_status;
                *total_received_amount = updated_received_amount;
            },
            Ok(()) if updated_status == InvoiceStatus::PartiallyPaid => {
                tracing::info!(
                    invoice_id = %invoice.id,
                    filled_amount = %updated_received_amount,
                    min_fill_amount = %min_paid_amount,
                    "Invoice has been partially paid, updating filled amount in registry"
                );

                self.registry
                    .update_filled_amount(&invoice.id, updated_received_amount)
                    .await;

                invoice.status = updated_status;
                *total_received_amount = updated_received_amount;
            },
            Ok(()) => unreachable!(),
            Err(TransactionsRecorderError::TransactionDuplication {
                chain,
                general_transaction_id,
            }) => {
                tracing::debug!(
                    invoice_id = %invoice.id,
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
                    invoice_id = %invoice.id,
                    "Error while storing transaction for invoice"
                );

                return Err(TransactionsRecorderError::DaoTransactionError);
            },
        }

        Ok(())
    }
}

#[cfg(test)]
mockall::mock! {
    pub TransactionsRecorder<D: 'static> {
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
    }

    impl<D> Clone for TransactionsRecorder<D> {
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
            default_chain: ChainType::PolkadotAssetHub,
            default_asset_id: HashMap::from([(
                ChainType::PolkadotAssetHub,
                1337.to_string(),
            )]),
            invoice_lifetime_millis: 600_000,
            recipient: HashMap::from([(
                ChainType::PolkadotAssetHub,
                "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
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
        //   - Invoice remains in registry with updated total received amount
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

        // Shared setup for test cases 4 and 5
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

        // Test case 4:
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

        // Test case 5:
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
}
