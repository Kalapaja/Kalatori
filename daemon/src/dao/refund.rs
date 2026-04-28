use sqlx::types::Text;
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    Refund,
    RefundRow,
    RefundStatus,
    RetryMeta,
    TransferDestinationParams,
};

use super::DaoExecutor;
use super::error_parsing::{
    StatusTransitionError,
    StatusTriggerError,
};

// ============================================================================
// Refund Domain Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum DaoRefundError {
    /// Refund not found by ID
    #[error("Refund not found: {refund_id}")]
    NotFound { refund_id: Uuid },

    /// Referenced invoice doesn't exist (foreign key violation)
    #[error("Invoice not found: {invoice_id}")]
    InvoiceNotFound { invoice_id: Uuid },

    /// Status transition not allowed
    #[error("Cannot transition from {current_status} to {attempted_status}")]
    StatusConstraintViolation {
        current_status: RefundStatus,
        attempted_status: RefundStatus,
    },

    /// Database operation failed
    #[error("Database error during refund operation")]
    DatabaseError,
}

impl From<sqlx::Error> for DaoRefundError {
    fn from(_e: sqlx::Error) -> Self {
        DaoRefundError::DatabaseError
    }
}

impl From<StatusTriggerError<RefundStatus>> for DaoRefundError {
    fn from(e: StatusTriggerError<RefundStatus>) -> Self {
        DaoRefundError::StatusConstraintViolation {
            current_status: e.old_status,
            attempted_status: e.new_status,
        }
    }
}

impl StatusTransitionError for RefundStatus {
    type ErrorType = DaoRefundError;

    const ERROR_TYPE_PREFIX: &'static str = "REFUND_STATUS_TRANSITION|";
}

pub trait DaoRefundMethods: DaoExecutor + 'static {
    async fn create_refund(
        &self,
        refund: Refund,
    ) -> Result<Refund, DaoRefundError> {
        let query = sqlx::query_as::<_, RefundRow>(
        "INSERT INTO refunds (id, invoice_id, asset_id, asset_name, chain, amount, source_address, destination_address, destination_chain, destination_asset_id, initiator_type, initiator_id, status, created_at, updated_at, retry_count, last_attempt_at, next_retry_at, failure_message)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
            .bind(refund.id)
            .bind(refund.invoice_id)
            .bind(&refund.asset_id)
            .bind(&refund.asset_name)
            .bind(refund.chain)
            .bind(Text(refund.amount))
            .bind(&refund.source_address)
            .bind(refund.destination_params.as_ref().map(|p| &p.destination_address))
            .bind(refund.destination_params.as_ref().map(|p| p.destination_chain))
            .bind(refund.destination_params.as_ref().map(|p| &p.destination_asset_id))
            .bind(refund.initiator_type)
            .bind(refund.initiator_id)
            .bind(refund.status)
            .bind(refund.created_at.naive_utc())
            .bind(refund.updated_at.naive_utc())
            .bind(refund.retry_meta.retry_count)
            .bind(refund.retry_meta.last_attempt_at.map(|dt| dt.naive_utc()))
            .bind(refund.retry_meta.next_retry_at.map(|dt| dt.naive_utc()))
            .bind(&refund.retry_meta.failure_message);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "create_refund",
                    refund_id = %refund.id,
                    invoice_id = %refund.invoice_id,
                    error.source = ?e,
                    "Failed to create refund"
                );

                match &e {
                    sqlx::Error::Database(db_err) => {
                        let message = db_err.message();

                        if message.contains("FOREIGN KEY") {
                            return DaoRefundError::InvoiceNotFound {
                                invoice_id: refund.invoice_id,
                            };
                        }

                        DaoRefundError::DatabaseError
                    },
                    _ => DaoRefundError::DatabaseError,
                }
            })
    }

    async fn get_all_refunds(&self) -> Result<Vec<Refund>, DaoRefundError> {
        let query = sqlx::query_as::<_, RefundRow>(
            "SELECT *
            FROM refunds",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "get_all_refunds",
                    error.source = ?e,
                    "Failed to fetch all refunds"
                );
                DaoRefundError::DatabaseError
            })
    }

    async fn get_refund_by_id(
        &self,
        refund_id: Uuid,
    ) -> Result<Option<Refund>, DaoRefundError> {
        let query = sqlx::query_as::<_, RefundRow>(
            "SELECT *
                FROM refunds
                WHERE id = ?",
        )
        .bind(refund_id);

        self.fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "get_refund_by_id",
                    %refund_id,
                    error.source = ?e,
                    "Failed to fetch refund"
                );
                DaoRefundError::DatabaseError
            })
    }

    async fn get_pending_refunds(
        &self,
        limit: u32,
    ) -> Result<Vec<Refund>, DaoRefundError> {
        let query = sqlx::query_as::<_, RefundRow>(
            "WITH sel AS (
                SELECT id
                FROM refunds
                WHERE
                    status = 'Waiting'
                    OR (status = 'FailedRetriable' AND next_retry_at <= datetime('now'))
                ORDER BY created_at ASC
                LIMIT ?
            )
            UPDATE refunds
            SET status = 'InProgress',
                updated_at = datetime('now')
            WHERE id IN (SELECT id FROM sel)
            RETURNING *",
        )
        .bind(limit);

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "get_pending_refunds",
                    limit,
                    error.source = ?e,
                    "Failed to fetch pending refunds"
                );
                DaoRefundError::DatabaseError
            })
    }

    async fn update_refund_status(
        &self,
        refund_id: Uuid,
        status: RefundStatus,
    ) -> Result<Refund, DaoRefundError> {
        let query = sqlx::query_as::<_, RefundRow>(
            "UPDATE refunds
            SET status = ?, updated_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(status)
        .bind(refund_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "update_refund_status",
                    %refund_id,
                    new_status = ?status,
                    error.source = ?e,
                    "Failed to update refund status"
                );

                // Parse with RefundStatus type
                if let Some(error) = RefundStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoRefundError::NotFound {
                        refund_id,
                    },
                    _ => DaoRefundError::DatabaseError,
                }
            })
    }

    async fn update_refund_retry(
        &self,
        refund_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Refund, DaoRefundError> {
        let status = if is_retriable {
            RefundStatus::FailedRetriable
        } else {
            RefundStatus::Failed
        };

        let query = sqlx::query_as::<_, RefundRow>(
            "UPDATE refunds
            SET retry_count = ?,
                last_attempt_at = ?,
                next_retry_at = ?,
                failure_message = ?,
                status = ?,
                updated_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(retry_meta.retry_count)
        .bind(
            retry_meta
                .last_attempt_at
                .map(|dt| dt.naive_utc()),
        )
        .bind(
            retry_meta
                .next_retry_at
                .map(|dt| dt.naive_utc()),
        )
        .bind(&retry_meta.failure_message)
        .bind(status)
        .bind(refund_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "update_refund_retry",
                    %refund_id,
                    retry_count = retry_meta.retry_count,
                    error.source = ?e,
                    "Failed to update refund retry"
                );

                // Check for trigger violation
                if let Some(error) = RefundStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoRefundError::NotFound {
                        refund_id,
                    },
                    _ => DaoRefundError::DatabaseError,
                }
            })
    }

    async fn update_refund_destination_params(
        &self,
        refund_id: Uuid,
        destination_params: TransferDestinationParams,
    ) -> Result<Refund, DaoRefundError> {
        let query = sqlx::query_as::<_, RefundRow>(
            "UPDATE refunds
            SET destination_address = ?,
                destination_asset_id = ?,
                destination_chain = ?,
                updated_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(destination_params.destination_address)
        .bind(destination_params.destination_asset_id)
        .bind(destination_params.destination_chain)
        .bind(refund_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.refund",
                    error.operation = "update_refund_retry",
                    %refund_id,
                    error.source = ?e,
                    "Failed to update refund retry"
                );

                DaoRefundError::DatabaseError
            })
    }
}

impl<T: DaoExecutor + 'static> DaoRefundMethods for T {}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::dao::create_test_dao;
    use crate::dao::invoice::DaoInvoiceMethods;
    use crate::types::{
        default_create_invoice_data,
        default_refund,
    };

    use super::*;

    #[tokio::test]
    async fn test_refund_create_and_get() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create refund
        let refund = default_refund(invoice.id);
        let refund_id = refund.id;
        let created = dao.create_refund(refund).await.unwrap();

        // Verify fields
        assert_eq!(created.id, refund_id);
        assert_eq!(created.invoice_id, invoice.id);
        assert_eq!(created.status, RefundStatus::Waiting);
        assert_eq!(created.retry_meta.retry_count, 0);

        // Get by ID
        let fetched = dao
            .get_refund_by_id(refund_id)
            .await
            .unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, refund_id);

        // Get non-existent
        let not_found = dao
            .get_refund_by_id(Uuid::new_v4())
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_get_pending_refunds_filtering() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create refund with Waiting status (should be returned)
        let refund1 = default_refund(invoice.id);
        dao.create_refund(refund1.clone())
            .await
            .unwrap();

        // Create refund with InProgress status (should NOT be returned)
        let mut refund2 = default_refund(invoice.id);
        refund2.status = RefundStatus::InProgress;
        dao.create_refund(refund2)
            .await
            .unwrap();

        // Create refund with Completed status (should NOT be returned)
        let mut refund3 = default_refund(invoice.id);
        refund3.status = RefundStatus::Completed;
        dao.create_refund(refund3)
            .await
            .unwrap();

        // Create refund with Waiting status and next_retry_at in the future — should
        // still be returned because all Waiting refunds are eligible regardless
        // of next_retry_at
        let mut refund4 = default_refund(invoice.id);
        refund4.retry_meta.next_retry_at = Some(Utc::now() + chrono::Duration::hours(1));
        dao.create_refund(refund4.clone())
            .await
            .unwrap();

        // Get pending refunds
        let pending = dao
            .get_pending_refunds(2)
            .await
            .unwrap();

        // Should return refund1 and refund4 (both Waiting), not refund2 (InProgress) or
        // refund3 (Completed)
        assert_eq!(pending.len(), 2);
        assert_eq!(
            pending[0].status,
            RefundStatus::InProgress
        );
        assert_eq!(
            pending[1].status,
            RefundStatus::InProgress
        );
        assert_eq!(pending[0].id, refund1.id);
        assert_eq!(pending[1].id, refund4.id);

        // Create FailedRetriable refund with next_retry_at in the past (should be
        // returned)
        let mut refund5 = default_refund(invoice.id);
        refund5.status = RefundStatus::FailedRetriable;
        refund5.created_at = Utc::now() - chrono::Duration::minutes(10);
        refund5.retry_meta.next_retry_at = Some(Utc::now() - chrono::Duration::minutes(2));
        dao.create_refund(refund5.clone())
            .await
            .unwrap();

        // Create FailedRetriable refund with next_retry_at in the future (should NOT be
        // returned)
        let mut refund6 = default_refund(invoice.id);
        refund6.status = RefundStatus::FailedRetriable;
        refund6.retry_meta.next_retry_at = Some(Utc::now() + chrono::Duration::hours(1));
        dao.create_refund(refund6)
            .await
            .unwrap();

        // Create another Waiting refund
        let refund7 = default_refund(invoice.id);
        dao.create_refund(refund7.clone())
            .await
            .unwrap();

        let pending_all = dao
            .get_pending_refunds(2)
            .await
            .unwrap();
        // refund5 (FailedRetriable, past retry, oldest) and refund7 (Waiting) picked
        // up. refund6 (FailedRetriable, future retry) excluded. Limit caps at
        // 2.
        assert_eq!(pending_all.len(), 2);
        assert_eq!(pending_all[0].id, refund5.id);
        assert_eq!(pending_all[1].id, refund7.id);
        assert_eq!(
            pending_all[0].status,
            RefundStatus::InProgress
        );
        assert_eq!(
            pending_all[1].status,
            RefundStatus::InProgress
        );
    }

    #[tokio::test]
    async fn test_update_refund_status() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        let refund = default_refund(invoice.id);
        let refund_id = refund.id;
        let created = dao.create_refund(refund).await.unwrap();
        assert_eq!(created.status, RefundStatus::Waiting);

        // Update to InProgress
        let updated = dao
            .update_refund_status(refund_id, RefundStatus::InProgress)
            .await
            .unwrap();
        assert_eq!(updated.status, RefundStatus::InProgress);

        // Update to Completed
        let completed = dao
            .update_refund_status(refund_id, RefundStatus::Completed)
            .await
            .unwrap();
        assert_eq!(
            completed.status,
            RefundStatus::Completed
        );
    }

    #[tokio::test]
    async fn test_update_refund_retry() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        let refund = default_refund(invoice.id);
        let refund_id = refund.id;
        dao.create_refund(refund).await.unwrap();

        // Transition to InProgress first (required before FailedRetriable)
        dao.update_refund_status(refund_id, RefundStatus::InProgress)
            .await
            .unwrap();

        // First retriable failure
        let mut retry_meta = RetryMeta::default();
        retry_meta.increment_retry("Insufficient balance".to_string());

        let updated = dao
            .update_refund_retry(refund_id, retry_meta, true)
            .await
            .unwrap();

        assert_eq!(
            updated.status,
            RefundStatus::FailedRetriable
        );
        assert_eq!(updated.retry_meta.retry_count, 1);
        assert!(
            updated
                .retry_meta
                .last_attempt_at
                .is_some()
        );
        assert!(
            updated
                .retry_meta
                .next_retry_at
                .is_some()
        );
        assert_eq!(
            updated.retry_meta.failure_message,
            Some("Insufficient balance".to_string())
        );

        // Transition back to InProgress via get_pending_refunds
        // (FailedRetriable with past next_retry_at gets picked up)
        // For simplicity, use update_refund_status directly
        dao.update_refund_status(refund_id, RefundStatus::InProgress)
            .await
            .unwrap();

        // Final non-retriable failure
        let mut retry_meta2 = updated.retry_meta;
        retry_meta2.increment_retry("Fatal error".to_string());

        let failed = dao
            .update_refund_retry(refund_id, retry_meta2, false)
            .await
            .unwrap();

        assert_eq!(failed.status, RefundStatus::Failed);
        assert_eq!(failed.retry_meta.retry_count, 2);
        assert_eq!(
            failed.retry_meta.failure_message,
            Some("Fatal error".to_string())
        );
    }

    #[tokio::test]
    async fn test_refund_status_transition_triggers() {
        let dao = create_test_dao().await;
        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;
        dao.create_invoice(invoice)
            .await
            .unwrap();

        // Scenario 1: Invalid transition from Completed -> Waiting
        let refund1 = Refund {
            status: RefundStatus::Completed,
            ..default_refund(invoice_id)
        };
        let id1 = refund1.id;
        dao.create_refund(refund1)
            .await
            .unwrap();

        let result = dao
            .update_refund_status(id1, RefundStatus::Waiting)
            .await;
        match result.unwrap_err() {
            DaoRefundError::StatusConstraintViolation {
                current_status,
                attempted_status,
            } => {
                assert_eq!(current_status, RefundStatus::Completed);
                assert_eq!(attempted_status, RefundStatus::Waiting);
            },
            err => panic!("Expected StatusConstraintViolation, got: {err:?}"),
        }

        // Scenario 2: Valid transition Waiting -> InProgress -> Completed
        let refund2 = Refund {
            status: RefundStatus::Waiting,
            ..default_refund(invoice_id)
        };
        let id2 = refund2.id;
        dao.create_refund(refund2)
            .await
            .unwrap();

        let updated1 = dao
            .update_refund_status(id2, RefundStatus::InProgress)
            .await
            .unwrap();
        assert_eq!(
            updated1.status,
            RefundStatus::InProgress
        );

        let updated2 = dao
            .update_refund_status(id2, RefundStatus::Completed)
            .await
            .unwrap();
        assert_eq!(updated2.status, RefundStatus::Completed);
    }
}
