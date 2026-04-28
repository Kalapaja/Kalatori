use chrono::{
    DateTime,
    Utc,
};
use sqlx::QueryBuilder;
use sqlx::types::{
    Json,
    Text,
};
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    ChainType,
    GeneralTransactionId,
    ListTransactionsParams,
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

impl crate::api::ApiErrorExt for DaoTransactionError {
    fn category(&self) -> &str {
        match self {
            DaoTransactionError::NotFound {
                ..
            } => "ENTITY_NOT_FOUND",
            DaoTransactionError::InvoiceNotFound {
                ..
            } => "RELATED_ENTITY_NOT_FOUND",
            DaoTransactionError::StatusConstraintViolation {
                ..
            } => "STATUS_CONSTRAINT_VIOLATION",
            DaoTransactionError::DuplicateTransaction {
                ..
            } => "DUPLICATE_ENTITY",
            DaoTransactionError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn code(&self) -> &str {
        match self {
            DaoTransactionError::NotFound {
                ..
            } => "TRANSACTION_NOT_FOUND",
            DaoTransactionError::InvoiceNotFound {
                ..
            } => "RELATED_INVOICE_NOT_FOUND",
            DaoTransactionError::StatusConstraintViolation {
                ..
            } => "TRANSACTION_STATUS_CONSTRAINT_VIOLATION",
            DaoTransactionError::DuplicateTransaction {
                ..
            } => "TRANSACTION_DUPLICATE",
            DaoTransactionError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn message(&self) -> &str {
        match self {
            DaoTransactionError::NotFound {
                ..
            } => "The requested transaction was not found.",
            DaoTransactionError::InvoiceNotFound {
                ..
            } => "The related invoice id was not found.",
            DaoTransactionError::StatusConstraintViolation {
                ..
            } => "The requested status transition is not allowed.",
            DaoTransactionError::DuplicateTransaction {
                ..
            } => "A transaction with the same blockchain coordinates already exists.",
            DaoTransactionError::DatabaseError => "A database error occurred.",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            DaoTransactionError::NotFound {
                ..
            } => reqwest::StatusCode::NOT_FOUND,
            DaoTransactionError::InvoiceNotFound {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoTransactionError::StatusConstraintViolation {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoTransactionError::DuplicateTransaction {
                ..
            } => reqwest::StatusCode::CONFLICT,
            DaoTransactionError::DatabaseError => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
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

    async fn get_transaction_by_id(
        &self,
        transaction_id: Uuid,
    ) -> Result<Option<Transaction>, DaoTransactionError> {
        let query = sqlx::query_as::<_, TransactionRow>(
            "SELECT *
            FROM transactions
            WHERE id = ?",
        )
        .bind(transaction_id);

        self.fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "get_transaction_by_id",
                    %transaction_id,
                    error.source = ?e,
                    "Failed to fetch transaction"
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

    /// Get a paginated, filtered list of transactions.
    async fn get_transactions_paginated(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        let mut builder = QueryBuilder::new("SELECT * FROM transactions t WHERE 1=1");

        push_transaction_filters(&mut builder, params);

        let sort_order = params.sort_order.unwrap_or_default();

        builder.push(" ORDER BY t.created_at ");
        builder.push(sort_order.as_sql());

        let per_page = params.pagination.validated_per_page();
        let offset = params.pagination.offset();

        builder.push(" LIMIT ");
        builder.push_bind(per_page);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let query = builder.build_query_as::<TransactionRow>();

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "get_transactions_paginated",
                    error.source = ?e,
                    "Failed to fetch paginated transactions"
                );
                DaoTransactionError::DatabaseError
            })
    }

    /// Count transactions matching the given filters (for pagination metadata).
    async fn count_transactions(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<u32, DaoTransactionError> {
        let mut builder =
            QueryBuilder::new("SELECT COUNT(*) as count FROM transactions t WHERE 1=1");

        push_transaction_filters(&mut builder, params);

        let query = builder.build_query_as::<CountRow>();

        let row: CountRow = self
            .fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "count_transactions",
                    error.source = ?e,
                    "Failed to count transactions"
                );
                DaoTransactionError::DatabaseError
            })?;

        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(row.count as u32)
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

    async fn get_completed_transactions_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        let query = sqlx::query_as::<_, TransactionRow>(
            "SELECT *
            FROM transactions
            WHERE invoice_id = ? AND transaction_type = 'Incoming' AND status = 'Completed'
            ORDER BY created_at ASC",
        )
        .bind(invoice_id);

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.transaction",
                    error.operation = "get_completed_transactions_by_invoice",
                    %invoice_id,
                    error.source = ?e,
                    "Failed to fetch completed transactions by invoice id"
                );
                DaoTransactionError::DatabaseError
            })
    }
}

impl<T: DaoExecutor + 'static> DaoTransactionMethods for T {}

#[derive(sqlx::FromRow)]
struct CountRow {
    count: i64,
}

/// Push WHERE clause conditions to the query builder based on filter params.
/// Shared between `get_transactions_paginated` and `count_transactions`.
fn push_transaction_filters(
    builder: &mut QueryBuilder<'_, sqlx::Sqlite>,
    params: &ListTransactionsParams,
) {
    if let Some(statuses) = &params.status
        && !statuses.is_empty()
    {
        builder.push(" AND t.status IN (");
        let mut separated = builder.separated(", ");
        for status in statuses {
            separated.push_bind(status.to_string());
        }
        separated.push_unseparated(")");
    }

    if let Some(transaction_type) = &params.transaction_type {
        builder.push(" AND t.transaction_type = ");
        builder.push_bind(transaction_type.to_string());
    }

    if let Some(chain) = &params.chain {
        builder.push(" AND t.chain = ");
        builder.push_bind(chain.to_string());
    }

    if let Some(asset_id) = &params.asset_id {
        builder.push(" AND t.asset_id = ");
        builder.push_bind(asset_id.clone());
    }

    if let Some(invoice_id) = &params.invoice_id {
        builder.push(" AND t.invoice_id = ");
        builder.push_bind(*invoice_id);
    }

    if let Some(created_from) = &params.created_from {
        builder.push(" AND t.created_at >= ");
        builder.push_bind(created_from.naive_utc());
    }

    if let Some(created_to) = &params.created_to {
        builder.push(" AND t.created_at <= ");
        builder.push_bind(created_to.naive_utc());
    }
}

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
            origin: origin_with_refund,
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

    // ========================================================================
    // Paginated / filtered snapshot tests
    // ========================================================================

    use crate::types::{
        CreateInvoiceData,
        InvoiceCart,
        TransferInfo,
    };
    use rust_decimal::Decimal;

    fn make_invoice(
        chain: ChainType,
        asset_id: &str,
    ) -> CreateInvoiceData {
        let id = Uuid::new_v4();
        CreateInvoiceData {
            id,
            order_id: id.to_string(),
            asset_id: asset_id.to_string(),
            asset_name: if asset_id == "1984" {
                "USDT".to_string()
            } else {
                "USDC".to_string()
            },
            chain,
            amount: Decimal::new(10000, 2),
            payment_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
            cart: InvoiceCart::empty(),
            redirect_url: "http://localhost:8080/thankyou".to_string(),
            #[expect(clippy::arithmetic_side_effects)]
            valid_till: chrono::Utc::now() + chrono::Duration::hours(24),
        }
    }

    fn make_transaction(
        invoice_id: Uuid,
        chain: ChainType,
        asset_id: &str,
        amount: Decimal,
        tx_type: TransactionType,
    ) -> Transaction {
        let id = Uuid::new_v4();
        Transaction {
            id,
            transfer_info: TransferInfo {
                chain,
                asset_id: asset_id.to_string(),
                asset_name: if asset_id == "1984" {
                    "USDT".to_string()
                } else {
                    "USDC".to_string()
                },
                amount,
                source_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
                destination_address: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
            },
            transaction_id: GeneralTransactionId {
                block_number: None,
                position_in_block: None,
                tx_hash: Some(id.to_string()),
            },
            transaction_type: tx_type,
            ..default_transaction(invoice_id)
        }
    }

    /// Seed 8 transactions with diverse properties, return their IDs in
    /// insertion order. A small sleep separates the first 4 from the last 4
    /// to allow date range filtering tests.
    ///
    /// | # | Chain    | Asset | Amount | Type     | Status      |
    /// |---|----------|-------|--------|----------|-------------|
    /// | 1 | AssetHub | USDT  | 100.00 | Incoming | Waiting     |
    /// | 2 | Polygon  | USDC  | 250.50 | Incoming | Waiting     |
    /// | 3 | AssetHub | USDT  |  75.00 | Outgoing | Completed   |
    /// | 4 | Polygon  | USDC  | 500.00 | Incoming | InProgress  |
    /// | 5 | AssetHub | USDC  | 300.00 | Outgoing | Failed      |
    /// | 6 | Polygon  | USDT  |  42.00 | Incoming | Completed   |
    /// | 7 | AssetHub | USDT  | 180.00 | Incoming | Waiting     |
    /// | 8 | Polygon  | USDC  |  99.99 | Outgoing | Completed   |
    async fn seed_transactions(
        dao: &crate::dao::DAO
    ) -> (
        Vec<Uuid>,
        Vec<Uuid>,
        chrono::DateTime<chrono::Utc>,
    ) {
        let mut tx_ids = Vec::new();

        // Create 2 invoices (one per chain) to parent the transactions
        let inv_ah = make_invoice(ChainType::PolkadotAssetHub, "1984");
        let inv_ah_id = inv_ah.id;
        dao.create_invoice(inv_ah)
            .await
            .unwrap();

        let inv_poly = make_invoice(ChainType::Polygon, "USDC");
        let inv_poly_id = inv_poly.id;
        dao.create_invoice(inv_poly)
            .await
            .unwrap();

        let invoice_ids = vec![inv_ah_id, inv_poly_id];

        // --- First batch (before the sleep) ---

        // Tx 1: AssetHub, USDT, 100.00, Incoming, Waiting
        let t = make_transaction(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "1984",
            Decimal::new(10000, 2),
            TransactionType::Incoming,
        );
        tx_ids.push(t.id);
        dao.create_transaction(t).await.unwrap();

        // Tx 2: Polygon, USDC, 250.50, Incoming, Waiting
        let t = make_transaction(
            inv_poly_id,
            ChainType::Polygon,
            "USDC",
            Decimal::new(25050, 2),
            TransactionType::Incoming,
        );
        tx_ids.push(t.id);
        dao.create_transaction(t).await.unwrap();

        // Tx 3: AssetHub, USDT, 75.00, Outgoing, Completed (Waiting -> InProgress ->
        // Completed)
        let t = make_transaction(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "1984",
            Decimal::new(7500, 2),
            TransactionType::Outgoing,
        );
        tx_ids.push(t.id);
        let mut created = dao.create_transaction(t).await.unwrap();
        created.status = TransactionStatus::InProgress;
        let mut created = dao
            .update_transaction(created)
            .await
            .unwrap();
        created.status = TransactionStatus::Completed;
        dao.update_transaction(created)
            .await
            .unwrap();

        // Tx 4: Polygon, USDC, 500.00, Incoming, InProgress (Waiting -> InProgress)
        let t = make_transaction(
            inv_poly_id,
            ChainType::Polygon,
            "USDC",
            Decimal::new(50000, 2),
            TransactionType::Incoming,
        );
        tx_ids.push(t.id);
        let mut created = dao.create_transaction(t).await.unwrap();
        created.status = TransactionStatus::InProgress;
        dao.update_transaction(created)
            .await
            .unwrap();

        // Sleep to create a timestamp gap between batches
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
        let batch_cutoff = chrono::Utc::now();
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;

        // --- Second batch (after the sleep) ---

        // Tx 5: AssetHub, USDC, 300.00, Outgoing, Failed (Waiting -> InProgress ->
        // Failed)
        let t = make_transaction(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "USDC",
            Decimal::new(30000, 2),
            TransactionType::Outgoing,
        );
        tx_ids.push(t.id);
        let mut created = dao.create_transaction(t).await.unwrap();
        created.status = TransactionStatus::InProgress;
        let mut created = dao
            .update_transaction(created)
            .await
            .unwrap();
        created.status = TransactionStatus::Failed;
        dao.update_transaction(created)
            .await
            .unwrap();

        // Tx 6: Polygon, USDT, 42.00, Incoming, Completed (Waiting -> InProgress ->
        // Completed)
        let t = make_transaction(
            inv_poly_id,
            ChainType::Polygon,
            "1984",
            Decimal::new(4200, 2),
            TransactionType::Incoming,
        );
        tx_ids.push(t.id);
        let mut created = dao.create_transaction(t).await.unwrap();
        created.status = TransactionStatus::InProgress;
        let mut created = dao
            .update_transaction(created)
            .await
            .unwrap();
        created.status = TransactionStatus::Completed;
        dao.update_transaction(created)
            .await
            .unwrap();

        // Tx 7: AssetHub, USDT, 180.00, Incoming, Waiting
        let t = make_transaction(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "1984",
            Decimal::new(18000, 2),
            TransactionType::Incoming,
        );
        tx_ids.push(t.id);
        dao.create_transaction(t).await.unwrap();

        // Tx 8: Polygon, USDC, 99.99, Outgoing, Completed (Waiting -> InProgress ->
        // Completed)
        let t = make_transaction(
            inv_poly_id,
            ChainType::Polygon,
            "USDC",
            Decimal::new(9999, 2),
            TransactionType::Outgoing,
        );
        tx_ids.push(t.id);
        let mut created = dao.create_transaction(t).await.unwrap();
        created.status = TransactionStatus::InProgress;
        let mut created = dao
            .update_transaction(created)
            .await
            .unwrap();
        created.status = TransactionStatus::Completed;
        dao.update_transaction(created)
            .await
            .unwrap();

        (tx_ids, invoice_ids, batch_cutoff)
    }

    #[tokio::test]
    async fn test_paginated_transactions_no_filters() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        let params = ListTransactionsParams::default();
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 8);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_filter_single_status() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // Waiting: tx1, tx2, tx7
        let params = ListTransactionsParams {
            status: Some(vec![TransactionStatus::Waiting]),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 3);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_filter_multiple_statuses() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // Completed: tx3, tx6, tx8 + InProgress: tx4
        let params = ListTransactionsParams {
            status: Some(vec![
                TransactionStatus::Completed,
                TransactionStatus::InProgress,
            ]),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_filter_by_type() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // Outgoing: tx3, tx5, tx8
        let params = ListTransactionsParams {
            transaction_type: Some(TransactionType::Outgoing),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 3);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_filter_by_chain() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // AssetHub: tx1, tx3, tx5, tx7
        let params = ListTransactionsParams {
            chain: Some(ChainType::PolkadotAssetHub),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_filter_by_asset_id() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // USDC: tx2, tx4, tx5, tx8
        let params = ListTransactionsParams {
            asset_id: Some("USDC".to_string()),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_filter_by_invoice_id() {
        let dao = create_test_dao().await;
        let (_tx_ids, invoice_ids, _) = seed_transactions(&dao).await;

        // Polygon invoice: tx2, tx4, tx6, tx8
        let params = ListTransactionsParams {
            invoice_id: Some(invoice_ids[1]),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_sort_asc() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        let params = ListTransactionsParams {
            sort_order: Some(crate::types::SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();

        assert_eq!(result.len(), 8);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_pagination() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        use crate::types::PaginationParams;

        // Page 1, size 3
        let params = ListTransactionsParams {
            pagination: PaginationParams {
                page: Some(1),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let page1 = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let total = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(total, 8);
        assert_eq!(page1.len(), 3);
        insta::assert_yaml_snapshot!("pagination_page1", page1, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });

        // Page 2
        let params2 = ListTransactionsParams {
            pagination: PaginationParams {
                page: Some(2),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let page2 = dao
            .get_transactions_paginated(&params2)
            .await
            .unwrap();
        assert_eq!(page2.len(), 3);
        insta::assert_yaml_snapshot!("pagination_page2", page2, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });

        // Page 3 (last, partial)
        let params3 = ListTransactionsParams {
            pagination: PaginationParams {
                page: Some(3),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let page3 = dao
            .get_transactions_paginated(&params3)
            .await
            .unwrap();
        assert_eq!(page3.len(), 2);
        insta::assert_yaml_snapshot!("pagination_page3", page3, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_date_range() {
        let dao = create_test_dao().await;
        let (_, _, batch_cutoff) = seed_transactions(&dao).await;

        // Filter to transactions created before the cutoff between batches.
        let params = ListTransactionsParams {
            created_to: Some(batch_cutoff),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        // Should get first batch (tx1-tx4)
        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_combined_filters() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // AssetHub + Incoming + Waiting => tx1, tx7
        let params = ListTransactionsParams {
            chain: Some(ChainType::PolkadotAssetHub),
            transaction_type: Some(TransactionType::Incoming),
            status: Some(vec![TransactionStatus::Waiting]),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 2);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_transactions_empty_result() {
        let dao = create_test_dao().await;
        seed_transactions(&dao).await;

        // No transactions with Incoming + Failed
        let params = ListTransactionsParams {
            transaction_type: Some(TransactionType::Incoming),
            status: Some(vec![TransactionStatus::Failed]),
            ..Default::default()
        };
        let result = dao
            .get_transactions_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_transactions(&params)
            .await
            .unwrap();

        assert_eq!(count, 0);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].tx_hash" => "[tx_hash]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }
}
