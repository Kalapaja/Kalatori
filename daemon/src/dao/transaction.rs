use chrono::{
    DateTime,
    Utc,
};
use sqlx::types::{
    Json,
    Text,
};
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    ChainType,
    GeneralTransactionId,
    Transaction,
    TransactionRow,
    TransactionStatus,
};

use super::DaoExecutor;
use super::error_parsing::{
    StatusTransitionError,
    StatusTriggerError,
};

// ============================================================================
// Transaction Domain Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum DaoTransactionError {
    /// Transaction not found by ID
    #[error("Transaction not found: {transaction_id}")]
    NotFound { transaction_id: Uuid },

    /// Referenced invoice doesn't exist (foreign key violation)
    #[error("Invoice not found: {invoice_id}")]
    InvoiceNotFound { invoice_id: Uuid },

    /// Status transition not allowed
    #[error("Cannot transition from {current_status} to {attempted_status}")]
    StatusConstraintViolation {
        current_status: TransactionStatus,
        attempted_status: TransactionStatus,
    },

    /// Transaction with the same blockchain coordinates already exists
    #[error("Duplicate transaction")]
    DuplicateTransaction {
        chain: ChainType,
        general_transaction_id: GeneralTransactionId,
    },

    /// Database operation failed
    #[error("Database error during transaction operation")]
    DatabaseError,
}

impl From<sqlx::Error> for DaoTransactionError {
    fn from(_e: sqlx::Error) -> Self {
        DaoTransactionError::DatabaseError
    }
}

impl From<StatusTriggerError<TransactionStatus>> for DaoTransactionError {
    fn from(e: StatusTriggerError<TransactionStatus>) -> Self {
        DaoTransactionError::StatusConstraintViolation {
            current_status: e.old_status,
            attempted_status: e.new_status,
        }
    }
}

impl StatusTransitionError for TransactionStatus {
    type ErrorType = DaoTransactionError;

    const ERROR_TYPE_PREFIX: &'static str = "TRANSACTION_STATUS_TRANSITION|";
}

pub trait DaoTransactionMethods: DaoExecutor + 'static {
    async fn create_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<Transaction, DaoTransactionError> {
        let query = sqlx::query_as::<_, TransactionRow>(
        "INSERT INTO transactions (id, invoice_id, asset_id, asset_name, chain, amount, source_address, destination_address, block_number, position_in_block, tx_hash, origin, status, transaction_type, outgoing_meta, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
            .bind(transaction.id)
            .bind(transaction.invoice_id)
            .bind(transaction.transfer_info.asset_id)
            .bind(transaction.transfer_info.asset_name)
            .bind(transaction.transfer_info.chain)
            .bind(Text(transaction.transfer_info.amount))
            .bind(&transaction.transfer_info.source_address)
            .bind(&transaction.transfer_info.destination_address)
            .bind(transaction.transaction_id.block_number)
            .bind(transaction.transaction_id.position_in_block)
            .bind(&transaction.transaction_id.tx_hash)
            .bind(Json(&transaction.origin))
            .bind(transaction.status)
            .bind(transaction.transaction_type)
            .bind(Json(&transaction.outgoing_meta))
            .bind(transaction.created_at.naive_utc())
            .bind(transaction.updated_at.naive_utc());

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "create_transaction",
                    transaction_id = %transaction.id,
                    invoice_id = %transaction.invoice_id,
                    error.source = ?e,
                    "Failed to create transaction"
                );

                match &e {
                    sqlx::Error::Database(db_err) => {
                        let message = db_err.message();

                        if message.contains("FOREIGN KEY") {
                            return DaoTransactionError::InvoiceNotFound {
                                invoice_id: transaction.invoice_id,
                            };
                        }

                        if db_err.kind() == sqlx::error::ErrorKind::UniqueViolation {
                            return DaoTransactionError::DuplicateTransaction {
                                chain: transaction.transfer_info.chain,
                                general_transaction_id: transaction.transaction_id,
                            };
                        }

                        DaoTransactionError::DatabaseError
                    },
                    _ => DaoTransactionError::DatabaseError,
                }
            })
    }

    async fn get_all_completed_transactions(
        &self
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        let query = sqlx::query_as::<_, TransactionRow>(
            "SELECT *
            FROM transactions
            WHERE status = 'Completed'",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transacton",
                    error.operation = "get_all_transactions",
                    error.source = ?e,
                    "Failed to fetch all transactions"
                );
                DaoTransactionError::DatabaseError
            })
    }

    async fn update_transaction_successful(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        confirmed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError> {
        // TODO: add additional check that transaction is `Outgoing`? Check it's status?
        // TODO: add updated_at field?
        let query = sqlx::query_as::<_, TransactionRow>(
            "UPDATE transactions
            SET block_number = ?, position_in_block = ?, tx_hash = ?, updated_at = ?, status = 'Completed',
                outgoing_meta = json_set(
                    outgoing_meta,
                    '$.confirmed_at', ?
                )
            WHERE id = ?
            RETURNING *",
        )
        .bind(chain_transaction_id.block_number)
        .bind(chain_transaction_id.position_in_block)
        .bind(chain_transaction_id.tx_hash)
        .bind(confirmed_at.naive_utc())
        .bind(confirmed_at.to_rfc3339())
        .bind(transaction_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "update_transaction_successful",
                    %transaction_id,
                    error.source = ?e,
                    "Failed to update transaction as successful"
                );

                // Parse with TransactionStatus type
                if let Some(error) = TransactionStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoTransactionError::NotFound {
                        transaction_id,
                    },
                    _ => DaoTransactionError::DatabaseError,
                }
            })
    }

    async fn update_transaction_failed(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        failure_message: String,
        failed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError> {
        // TODO: add additional check that transaction is `Outgoing`? Check it's status?
        let query = sqlx::query_as::<_, TransactionRow>(
            "UPDATE transactions
            SET block_number = ?, position_in_block = ?, tx_hash = ?, updated_at = ?, status = 'Failed',
                outgoing_meta = json_set(
                    outgoing_meta,
                    '$.failed_at', ?,
                    '$.failure_message', ?
                )
            WHERE id = ?
            RETURNING *",
        )
        .bind(chain_transaction_id.block_number)
        .bind(chain_transaction_id.position_in_block)
        .bind(chain_transaction_id.tx_hash)
        .bind(failed_at.naive_utc())
        .bind(failed_at.to_rfc3339())
        .bind(failure_message)
        .bind(transaction_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "update_transaction_failed",
                    %transaction_id,
                    error.source = ?e,
                    "Failed to update transaction as failed"
                );

                // Parse with TransactionStatus type
                if let Some(error) = TransactionStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoTransactionError::NotFound {
                        transaction_id,
                    },
                    _ => DaoTransactionError::DatabaseError,
                }
            })
    }

    // This method updates all fields of the transaction, it shouldn't be used in
    // real code except for tests
    #[cfg(test)]
    async fn update_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<Transaction, DaoTransactionError> {
        let query = sqlx::query_as::<_, TransactionRow>(
            "UPDATE transactions
            SET invoice_id = ?, asset_id = ?, chain = ?, amount = ?, source_address = ?, destination_address = ?,
                block_number = ?, position_in_block = ?, tx_hash = ?, origin = ?, status = ?,
                transaction_type = ?, outgoing_meta = ?
            WHERE id = ?
            RETURNING *",
        )
        .bind(transaction.invoice_id)
        .bind(&transaction.transfer_info.asset_id)
        .bind(transaction.transfer_info.chain)
        .bind(Text(transaction.transfer_info.amount))
        .bind(&transaction.transfer_info.source_address)
        .bind(&transaction.transfer_info.destination_address)
        .bind(transaction.transaction_id.block_number)
        .bind(transaction.transaction_id.position_in_block)
        .bind(&transaction.transaction_id.tx_hash)
        .bind(Json(&transaction.origin))
        .bind(transaction.status)
        .bind(transaction.transaction_type)
        .bind(Json(&transaction.outgoing_meta))
        .bind(transaction.id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "update_transaction",
                    transaction_id = %transaction.id,
                    error.source = ?e,
                    "Failed to update transaction"
                );

                // Parse with TransactionStatus type
                if let Some(error) = TransactionStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoTransactionError::NotFound {
                        transaction_id: transaction.id,
                    },
                    _ => DaoTransactionError::DatabaseError,
                }
            })
    }

    async fn get_invoice_transactions(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        let query = sqlx::query_as::<_, TransactionRow>(
            "SELECT *
            FROM transactions
            WHERE invoice_id = ?
            ORDER BY created_at ASC",
        )
        .bind(invoice_id);

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "get_invoice_transactions",
                    %invoice_id,
                    error.source = ?e,
                    "Failed to fetch invoice transactions"
                );
                DaoTransactionError::DatabaseError
            })
    }
}

impl<T: DaoExecutor + 'static> DaoTransactionMethods for T {}

#[cfg(test)]
mod tests {
    use crate::dao::create_test_dao;
    use crate::dao::invoice::DaoInvoiceMethods;

    use crate::types::{
        OutgoingTransactionMeta,
        Transaction,
        TransactionOrigin,
        TransactionStatus,
        TransactionType,
        default_create_invoice_data,
        default_transaction,
    };

    use super::*;

    // Transaction Tests

    #[tokio::test]
    async fn test_transaction_crud_operations() {
        let dao = create_test_dao().await;

        // Create invoice (required for FK)
        let invoice = default_create_invoice_data();
        dao.create_invoice(invoice.clone())
            .await
            .unwrap();

        // 1. Create incoming transaction
        let transaction = default_transaction(invoice.id);
        let tx_id = transaction.id;
        let created = dao
            .create_transaction(transaction.clone())
            .await
            .unwrap();

        // 2. Verify all fields match
        assert_eq!(created.id, tx_id);
        assert_eq!(created.invoice_id, invoice.id);
        assert_eq!(
            created.transaction_type,
            TransactionType::Incoming
        );
        assert_eq!(
            created.transaction_id.block_number,
            Some(1000)
        ); // From default
        assert_eq!(
            created.status,
            TransactionStatus::Waiting
        );

        // 3. Update transaction (change status)
        let mut updated_tx = created.clone();
        updated_tx.status = TransactionStatus::Completed;
        updated_tx.transaction_id.tx_hash = Some("0xabcd1234".to_string());

        let updated = dao
            .update_transaction(updated_tx)
            .await
            .unwrap();
        assert_eq!(
            updated.status,
            TransactionStatus::Completed
        );
        assert_eq!(
            updated.transaction_id.tx_hash,
            Some("0xabcd1234".to_string())
        );

        // 4. Get transactions for invoice
        let txs = dao
            .get_invoice_transactions(invoice.id)
            .await
            .unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].id, tx_id);

        // 5. Get transactions for non-existent invoice
        let empty = dao
            .get_invoice_transactions(Uuid::new_v4())
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_create_transaction_types() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create Incoming transaction
        let mut incoming = Transaction {
            transaction_type: TransactionType::Incoming,
            ..default_transaction(invoice.id)
        };
        incoming.transaction_id.block_number = Some(100);
        incoming.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());

        let created_in = dao
            .create_transaction(incoming)
            .await
            .unwrap();

        assert_eq!(
            created_in.transaction_type,
            TransactionType::Incoming
        );

        // Create Outgoing transaction
        let outgoing = Transaction {
            transaction_type: TransactionType::Outgoing,
            ..default_transaction(invoice.id)
        };
        let created_out = dao
            .create_transaction(outgoing)
            .await
            .unwrap();
        assert_eq!(
            created_out.transaction_type,
            TransactionType::Outgoing
        );
    }

    #[tokio::test]
    async fn test_create_transaction_foreign_key_constraint() {
        let dao = create_test_dao().await;

        // Try to create transaction with non-existent invoice_id
        let fake_invoice_id = Uuid::new_v4();
        let transaction = default_transaction(fake_invoice_id);
        let result = dao
            .create_transaction(transaction)
            .await;

        // Should fail with InvoiceNotFound error
        assert!(result.is_err());
        match result.unwrap_err() {
            DaoTransactionError::InvoiceNotFound {
                invoice_id,
            } => {
                assert_eq!(invoice_id, fake_invoice_id);
            },
            err => panic!("Expected InvoiceNotFound, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_transaction_status_transitions() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create transaction in Waiting status
        let mut tx = default_transaction(invoice.id);
        tx.status = TransactionStatus::Waiting;
        let created = dao
            .create_transaction(tx)
            .await
            .unwrap();
        assert_eq!(
            created.status,
            TransactionStatus::Waiting
        );

        // Transition to InProgress
        let mut in_progress = created.clone();
        in_progress.status = TransactionStatus::InProgress;
        let updated1 = dao
            .update_transaction(in_progress)
            .await
            .unwrap();
        assert_eq!(
            updated1.status,
            TransactionStatus::InProgress
        );

        // Transition to Completed
        let mut completed = updated1.clone();
        completed.status = TransactionStatus::Completed;
        let updated2 = dao
            .update_transaction(completed)
            .await
            .unwrap();
        assert_eq!(
            updated2.status,
            TransactionStatus::Completed
        );

        // Test Failed status
        let mut tx_failed = default_transaction(invoice.id);
        tx_failed.status = TransactionStatus::Failed;
        tx_failed.transaction_id.block_number = Some(100);
        tx_failed.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());
        let failed = dao
            .create_transaction(tx_failed)
            .await
            .unwrap();
        assert_eq!(failed.status, TransactionStatus::Failed);
    }

    #[expect(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_update_transaction_failed_and_successful() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Test 1: Update transaction to Failed
        let tx1 = Transaction {
            transaction_id: GeneralTransactionId::empty(),
            ..default_transaction(invoice.id)
        };

        let created1 = dao
            .create_transaction(tx1)
            .await
            .unwrap();

        assert!(
            created1
                .transaction_id
                .block_number
                .is_none()
        );
        assert!(
            created1
                .transaction_id
                .position_in_block
                .is_none()
        );
        assert!(
            created1
                .transaction_id
                .tx_hash
                .is_none()
        );

        let transaction_id1 = created1.id;

        // First transition to InProgress (required before Failed)
        let mut tx_in_progress = created1.clone();
        tx_in_progress.status = TransactionStatus::InProgress;
        dao.update_transaction(tx_in_progress)
            .await
            .unwrap();

        let chain_transaction_id1 = GeneralTransactionId {
            block_number: Some(123),
            position_in_block: Some(1),
            tx_hash: None,
        };

        let now1 = Utc::now();

        let updated1 = dao
            .update_transaction_failed(
                transaction_id1,
                chain_transaction_id1.clone(),
                "Network error".to_string(),
                now1,
            )
            .await
            .unwrap();

        assert_eq!(
            updated1.transaction_id.block_number,
            Some(123)
        );
        assert_eq!(
            updated1
                .transaction_id
                .position_in_block,
            Some(1)
        );
        assert!(
            updated1
                .transaction_id
                .tx_hash
                .is_none()
        );
        assert_eq!(
            updated1.status,
            TransactionStatus::Failed
        );
        assert_eq!(
            updated1.outgoing_meta.failed_at,
            Some(now1)
        );

        // Test 2: Update different transaction to Completed
        let tx2 = Transaction {
            transaction_id: GeneralTransactionId::empty(),
            ..default_transaction(invoice.id)
        };

        let created2 = dao
            .create_transaction(tx2)
            .await
            .unwrap();

        let transaction_id2 = created2.id;

        // Transition to InProgress first
        let mut tx2_in_progress = created2.clone();
        tx2_in_progress.status = TransactionStatus::InProgress;
        dao.update_transaction(tx2_in_progress)
            .await
            .unwrap();

        let chain_transaction_id2 = GeneralTransactionId {
            block_number: Some(456),
            position_in_block: Some(2),
            tx_hash: None,
        };

        let now2 = Utc::now();

        let updated2 = dao
            .update_transaction_successful(
                transaction_id2,
                chain_transaction_id2,
                now2,
            )
            .await
            .unwrap();

        assert_eq!(
            updated2.transaction_id.block_number,
            Some(456)
        );
        assert_eq!(
            updated2
                .transaction_id
                .position_in_block,
            Some(2)
        );
        assert!(
            updated2
                .transaction_id
                .tx_hash
                .is_none()
        );
        assert_eq!(
            updated2.status,
            TransactionStatus::Completed
        );
        assert_eq!(
            updated2.outgoing_meta.confirmed_at,
            Some(now2)
        );
    }

    #[tokio::test]
    async fn test_transaction_json_fields() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Test TransactionOrigin with refund_id
        let origin_with_refund = TransactionOrigin {
            refund_id: Some(Uuid::new_v4()),
            payout_id: None,
            internal_transfer_id: None,
        };

        let mut tx_with_origin = Transaction {
            origin: origin_with_refund.clone(),
            ..default_transaction(invoice.id)
        };

        tx_with_origin
            .transaction_id
            .block_number = Some(100);
        tx_with_origin.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());

        let _created = dao
            .create_transaction(tx_with_origin)
            .await
            .unwrap();

        // Test OutgoingTransactionMeta with metadata
        let outgoing_meta = OutgoingTransactionMeta {
            extrinsic_bytes: Some("0x123456".to_string()),
            built_at: Some(Utc::now()),
            sent_at: Some(Utc::now()),
            confirmed_at: None,
            failed_at: None,
            failure_message: None,
        };

        let tx_with_meta = Transaction {
            outgoing_meta: outgoing_meta.clone(),
            ..default_transaction(invoice.id)
        };

        let created2 = dao
            .create_transaction(tx_with_meta)
            .await
            .unwrap();
        assert_eq!(
            created2.outgoing_meta.extrinsic_bytes,
            outgoing_meta.extrinsic_bytes
        );
    }

    #[tokio::test]
    async fn test_get_invoice_transactions_ordering() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create 3 transactions at different times
        let mut tx1 = default_transaction(invoice.id);
        tx1.transaction_id.block_number = Some(100);
        tx1.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());
        let id1 = tx1.id;
        dao.create_transaction(tx1)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let mut tx2 = default_transaction(invoice.id);
        tx2.transaction_id.block_number = Some(300);
        tx2.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());
        let id2 = tx2.id;
        dao.create_transaction(tx2)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let tx3 = default_transaction(invoice.id);
        let id3 = tx3.id;
        dao.create_transaction(tx3)
            .await
            .unwrap();

        // Get all transactions
        let txs = dao
            .get_invoice_transactions(invoice.id)
            .await
            .unwrap();

        // Verify ordered by created_at ASC
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].id, id1);
        assert_eq!(txs[1].id, id2);
        assert_eq!(txs[2].id, id3);
    }

    #[tokio::test]
    async fn test_update_transaction_not_found() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Try to update non-existent transaction
        let tx = default_transaction(invoice.id);
        let result = dao.update_transaction(tx).await;

        // Should fail with NotFound
        assert!(result.is_err());
        match result.unwrap_err() {
            DaoTransactionError::NotFound {
                ..
            } => { /* Expected */ },
            err => panic!("Expected NotFound, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_transaction_nullable_fields() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create transaction with NULL fields (pending transaction)
        let pending_tx = Transaction {
            transaction_id: GeneralTransactionId::empty(),
            ..default_transaction(invoice.id)
        };

        let created = dao
            .create_transaction(pending_tx)
            .await
            .unwrap();
        assert!(
            created
                .transaction_id
                .block_number
                .is_none()
        );
        assert!(
            created
                .transaction_id
                .position_in_block
                .is_none()
        );
        assert!(created.transaction_id.tx_hash.is_none());

        // Update to finalized (add blockchain location)
        let mut finalized = created.clone();
        finalized.transaction_id.block_number = Some(5000);
        finalized
            .transaction_id
            .position_in_block = Some(3);
        finalized.transaction_id.tx_hash = Some("0xfinalized".to_string());

        let updated = dao
            .update_transaction(finalized)
            .await
            .unwrap();
        assert_eq!(
            updated.transaction_id.block_number,
            Some(5000)
        );
        assert_eq!(
            updated.transaction_id.position_in_block,
            Some(3)
        );
    }

    #[tokio::test]
    async fn test_transaction_status_transition_triggers() {
        let dao = create_test_dao().await;
        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;
        dao.create_invoice(invoice)
            .await
            .unwrap();

        // Scenario 1: Invalid transition from Completed -> InProgress
        let tx1 = Transaction {
            status: TransactionStatus::Completed,
            ..default_transaction(invoice_id)
        };
        let id1 = tx1.id;
        dao.create_transaction(tx1)
            .await
            .unwrap();

        let chain_tx_id = GeneralTransactionId {
            block_number: Some(100),
            position_in_block: Some(1),
            tx_hash: Some("0x123".to_string()),
        };

        // Try to update to Completed again (idempotent - should succeed)
        // Trigger doesn't fire because NEW.status == OLD.status
        let result = dao
            .update_transaction_successful(id1, chain_tx_id, Utc::now())
            .await;

        // Should succeed (idempotent update)
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert_eq!(
            updated.status,
            TransactionStatus::Completed
        );
        assert_eq!(
            updated.transaction_id.block_number,
            Some(100)
        );
        assert_eq!(
            updated.transaction_id.position_in_block,
            Some(1)
        );

        // Scenario 2: Valid transition Waiting -> Completed (direct, for incoming
        // transactions)
        let mut tx2 = Transaction {
            status: TransactionStatus::Waiting,
            ..default_transaction(invoice_id)
        };
        tx2.transaction_id.block_number = None;
        tx2.transaction_id.tx_hash = None;
        let id2 = tx2.id;
        dao.create_transaction(tx2)
            .await
            .unwrap();

        let chain_tx_id1 = GeneralTransactionId {
            block_number: Some(500),
            position_in_block: Some(1),
            tx_hash: Some("0x12345".to_string()),
        };

        let updated = dao
            .update_transaction_successful(id2, chain_tx_id1, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            updated.status,
            TransactionStatus::Completed
        );

        // Scenario 3: Valid transition Waiting -> InProgress -> Completed
        let tx3 = Transaction {
            status: TransactionStatus::Waiting,
            ..default_transaction(invoice_id)
        };
        let id3 = tx3.id;
        dao.create_transaction(tx3.clone())
            .await
            .unwrap();

        // Update to InProgress
        let mut tx3_inprogress = tx3;
        tx3_inprogress.status = TransactionStatus::InProgress;
        let updated1 = dao
            .update_transaction(tx3_inprogress)
            .await
            .unwrap();
        assert_eq!(
            updated1.status,
            TransactionStatus::InProgress
        );

        // Then to Completed
        let chain_tx_id2 = GeneralTransactionId {
            block_number: Some(200),
            position_in_block: Some(2),
            tx_hash: Some("0x456".to_string()),
        };
        let updated2 = dao
            .update_transaction_successful(id3, chain_tx_id2, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            updated2.status,
            TransactionStatus::Completed
        );
    }
}
