use sqlx::types::Text;
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    Payout,
    PayoutRow,
    PayoutStatus,
    RetryMeta,
};

use super::DaoExecutor;
use super::error_parsing::{
    StatusTransitionError,
    StatusTriggerError,
};

// ============================================================================
// Payout Domain Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum DaoPayoutError {
    /// Payout not found by ID
    #[error("Payout not found: {payout_id}")]
    NotFound { payout_id: Uuid },

    /// Referenced invoice doesn't exist (foreign key violation)
    #[error("Invoice not found: {invoice_id}")]
    InvoiceNotFound { invoice_id: Uuid },

    /// Status transition not allowed
    #[error("Cannot transition from {current_status} to {attempted_status}")]
    StatusConstraintViolation {
        current_status: PayoutStatus,
        attempted_status: PayoutStatus,
    },

    /// Database operation failed
    #[error("Database error during payout operation")]
    DatabaseError,
}

impl From<sqlx::Error> for DaoPayoutError {
    fn from(_e: sqlx::Error) -> Self {
        DaoPayoutError::DatabaseError
    }
}

impl From<StatusTriggerError<PayoutStatus>> for DaoPayoutError {
    fn from(e: StatusTriggerError<PayoutStatus>) -> Self {
        DaoPayoutError::StatusConstraintViolation {
            current_status: e.old_status,
            attempted_status: e.new_status,
        }
    }
}

impl StatusTransitionError for PayoutStatus {
    type ErrorType = DaoPayoutError;

    const ERROR_TYPE_PREFIX: &'static str = "PAYOUT_STATUS_TRANSITION|";
}

pub trait DaoPayoutMethods: DaoExecutor + 'static {
    async fn create_payout(
        &self,
        payout: Payout,
    ) -> Result<Payout, DaoPayoutError> {
        let query = sqlx::query_as::<_, PayoutRow>(
        "INSERT INTO payouts (id, invoice_id, asset_id, asset_name, chain, source_address, destination_address, amount, initiator_type, initiator_id, status, created_at, updated_at, retry_count, last_attempt_at, next_retry_at, failure_message)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
            .bind(payout.id)
            .bind(payout.invoice_id)
            .bind(payout.transfer_info.asset_id)
            .bind(payout.transfer_info.asset_name)
            .bind(payout.transfer_info.chain)
            .bind(&payout.transfer_info.source_address)
            .bind(&payout.transfer_info.destination_address)
            .bind(Text(payout.transfer_info.amount))
            .bind(payout.initiator_type)
            .bind(payout.initiator_id)
            .bind(payout.status)
            .bind(payout.created_at.naive_utc())
            .bind(payout.updated_at.naive_utc())
            .bind(payout.retry_meta.retry_count)
            .bind(payout.retry_meta.last_attempt_at.map(|dt| dt.naive_utc()))
            .bind(payout.retry_meta.next_retry_at.map(|dt| dt.naive_utc()))
            .bind(&payout.retry_meta.failure_message);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "create_payout",
                    payout_id = %payout.id,
                    invoice_id = %payout.invoice_id,
                    error.source = ?e,
                    "Failed to create payout"
                );

                match &e {
                    sqlx::Error::Database(db_err) => {
                        let message = db_err.message();

                        if message.contains("FOREIGN KEY") {
                            return DaoPayoutError::InvoiceNotFound {
                                invoice_id: payout.invoice_id,
                            };
                        }

                        DaoPayoutError::DatabaseError
                    },
                    _ => DaoPayoutError::DatabaseError,
                }
            })
    }

    async fn get_all_payouts(&self) -> Result<Vec<Payout>, DaoPayoutError> {
        let query = sqlx::query_as::<_, PayoutRow>(
            "SELECT *
            FROM payouts",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "get_all_payouts",
                    error.source = ?e,
                    "Failed to fetch all payouts"
                );
                DaoPayoutError::DatabaseError
            })
    }

    async fn get_payout_by_id(
        &self,
        payout_id: Uuid,
    ) -> Result<Option<Payout>, DaoPayoutError> {
        let query = sqlx::query_as::<_, PayoutRow>(
            "SELECT *
            FROM payouts
            WHERE id = ?",
        )
        .bind(payout_id);

        self.fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "get_payout_by_id",
                    %payout_id,
                    error.source = ?e,
                    "Failed to fetch payout"
                );
                DaoPayoutError::DatabaseError
            })
    }

    /// Fetch pending payouts and mark them as `InProgress`
    // TODO: besides of Payouts it should also return associated outgoing
    // Transactions
    async fn get_pending_payouts(
        &self,
        limit: u32,
    ) -> Result<Vec<Payout>, DaoPayoutError> {
        // TODO: in future versions of sqlite (bundled in sqlx) we'll probably be able
        // to use UPDATE ... ORDER BY LIMIT directly
        let query = sqlx::query_as::<_, PayoutRow>(
            "WITH sel AS (
                SELECT id
                FROM payouts
                WHERE status = 'Waiting'
                    AND (next_retry_at IS NULL OR next_retry_at <= datetime('now'))
                ORDER BY created_at ASC
                LIMIT ?
            )
            UPDATE payouts
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
                    error.category = "dao.payout",
                    error.operation = "get_pending_payouts",
                    limit,
                    error.source = ?e,
                    "Failed to fetch pending payouts"
                );
                DaoPayoutError::DatabaseError
            })
    }

    async fn update_payout_status(
        &self,
        payout_id: Uuid,
        status: PayoutStatus,
    ) -> Result<Payout, DaoPayoutError> {
        let query = sqlx::query_as::<_, PayoutRow>(
            "UPDATE payouts
            SET status = ?, updated_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(status)
        .bind(payout_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "update_payout_status",
                    %payout_id,
                    new_status = ?status,
                    error.source = ?e,
                    "Failed to update payout status"
                );

                // Parse with PayoutStatus type
                if let Some(error) = PayoutStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoPayoutError::NotFound {
                        payout_id,
                    },
                    _ => DaoPayoutError::DatabaseError,
                }
            })
    }

    async fn update_payout_retry(
        &self,
        payout_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Payout, DaoPayoutError> {
        let status = if is_retriable {
            PayoutStatus::FailedRetriable
        } else {
            PayoutStatus::Failed
        };

        let query = sqlx::query_as::<_, PayoutRow>(
            "UPDATE payouts
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
        .bind(payout_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "update_payout_retry",
                    %payout_id,
                    retry_count = retry_meta.retry_count,
                    error.source = ?e,
                    "Failed to update payout retry"
                );

                // Check for trigger violation
                if let Some(error) = PayoutStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoPayoutError::NotFound {
                        payout_id,
                    },
                    _ => DaoPayoutError::DatabaseError,
                }
            })
    }
}

impl<T: DaoExecutor + 'static> DaoPayoutMethods for T {}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::dao::create_test_dao;
    use crate::dao::invoice::DaoInvoiceMethods;
    use crate::types::{
        default_create_invoice_data,
        default_payout,
    };

    use super::*;

    #[tokio::test]
    async fn test_payout_create_and_get() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create payout
        let payout = default_payout(invoice.id);
        let payout_id = payout.id;
        let created = dao
            .create_payout(payout.clone())
            .await
            .unwrap();

        // Verify fields
        assert_eq!(created, payout);

        // Get by ID
        let fetched = dao
            .get_payout_by_id(payout_id)
            .await
            .unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap(), payout);

        // Get non-existent
        let not_found = dao
            .get_payout_by_id(Uuid::new_v4())
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_get_pending_payouts_filtering() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        // Create payout with Waiting status (should be returned)
        let payout1 = default_payout(invoice.id);
        dao.create_payout(payout1)
            .await
            .unwrap();

        // Create payout with InProgress status (should NOT be returned)
        let mut payout2 = default_payout(invoice.id);
        payout2.status = PayoutStatus::InProgress;
        dao.create_payout(payout2)
            .await
            .unwrap();

        // Create payout with Completed status (should NOT be returned)
        let mut payout3 = default_payout(invoice.id);
        payout3.status = PayoutStatus::Completed;
        dao.create_payout(payout3)
            .await
            .unwrap();

        // Create payout with Waiting status but next_retry_at in future (should NOT be
        // returned)
        let mut payout4 = default_payout(invoice.id);
        payout4.retry_meta.next_retry_at = Some(Utc::now() + chrono::Duration::hours(1));
        dao.create_payout(payout4)
            .await
            .unwrap();

        // Get pending payouts
        let pending = dao
            .get_pending_payouts(2)
            .await
            .unwrap();

        // Should only return payout1 (InProgress with no next_retry_at)
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].status,
            PayoutStatus::InProgress
        );
        assert_eq!(
            pending[0].retry_meta,
            RetryMeta::default()
        );

        let payout5 = Payout {
            created_at: Utc::now() - chrono::Duration::minutes(10),
            ..default_payout(invoice.id)
        };
        dao.create_payout(payout5.clone())
            .await
            .unwrap();

        let payout6 = Payout {
            created_at: Utc::now() - chrono::Duration::minutes(5),
            retry_meta: RetryMeta {
                next_retry_at: Some(Utc::now() - chrono::Duration::minutes(2)),
                ..RetryMeta::default()
            },
            ..default_payout(invoice.id)
        };
        dao.create_payout(payout6.clone())
            .await
            .unwrap();

        let payout7 = default_payout(invoice.id);
        dao.create_payout(payout7)
            .await
            .unwrap();

        let pending_all = dao
            .get_pending_payouts(2)
            .await
            .unwrap();
        assert_eq!(pending_all.len(), 2);
        assert_eq!(
            pending_all[0].status,
            PayoutStatus::InProgress
        );
        assert_eq!(
            pending_all[1].status,
            PayoutStatus::InProgress
        );
        assert_eq!(pending_all[0].id, payout5.id);
        assert_eq!(pending_all[1].id, payout6.id);
    }

    #[tokio::test]
    async fn test_update_payout_status() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        let payout = default_payout(invoice.id);
        let payout_id = payout.id;
        let created = dao.create_payout(payout).await.unwrap();
        assert_eq!(created.status, PayoutStatus::Waiting);

        // Update to InProgress
        let updated = dao
            .update_payout_status(payout_id, PayoutStatus::InProgress)
            .await
            .unwrap();

        assert_eq!(updated.status, PayoutStatus::InProgress);

        // Update to Completed
        let completed = dao
            .update_payout_status(payout_id, PayoutStatus::Completed)
            .await
            .unwrap();

        assert_eq!(
            completed.status,
            PayoutStatus::Completed
        );
    }

    #[tokio::test]
    async fn test_update_payout_retry() {
        let dao = create_test_dao().await;
        let invoice = dao
            .create_invoice(default_create_invoice_data())
            .await
            .unwrap();

        let payout = default_payout(invoice.id);
        let payout_id = payout.id;
        dao.create_payout(payout).await.unwrap();

        // First transition to InProgress (required before FailedRetriable)
        dao.update_payout_status(payout_id, PayoutStatus::InProgress)
            .await
            .unwrap();

        // First retry - now we can transition to FailedRetriable
        let now = Utc::now();
        let next_retry = now + chrono::Duration::minutes(1);

        let retry_meta = RetryMeta {
            retry_count: 1,
            last_attempt_at: Some(now),
            next_retry_at: Some(next_retry),
            failure_message: Some("Network error".to_string()),
        };

        let updated = dao
            .update_payout_retry(payout_id, retry_meta, true)
            .await
            .unwrap();

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
            Some("Network error".to_string())
        );
        assert_eq!(
            updated.status,
            PayoutStatus::FailedRetriable
        );

        // Second retry attempt - transition back to InProgress first, then fail
        // permanently
        let now2 = Utc::now();
        let next_retry2 = now2 + chrono::Duration::minutes(5);

        // First transition to InProgress (retry attempt)
        dao.update_payout_status(payout_id, PayoutStatus::InProgress)
            .await
            .unwrap();

        // Now we can set it to Failed (not retriable this time)
        let retry_meta2 = RetryMeta {
            retry_count: 2,
            last_attempt_at: Some(now2),
            next_retry_at: Some(next_retry2),
            failure_message: Some("Connection timeout".to_string()),
        };

        let updated2 = dao
            .update_payout_retry(payout_id, retry_meta2, false)
            .await
            .unwrap();

        assert_eq!(updated2.retry_meta.retry_count, 2);
        assert_eq!(
            updated2.retry_meta.failure_message,
            Some("Connection timeout".to_string())
        );
        assert_eq!(updated2.status, PayoutStatus::Failed);
    }

    #[tokio::test]
    async fn test_payout_status_transition_triggers() {
        let dao = create_test_dao().await;
        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;
        dao.create_invoice(invoice)
            .await
            .unwrap();

        // Scenario 1: Invalid transition from Completed -> Waiting
        let payout1 = Payout {
            status: PayoutStatus::Completed,
            ..default_payout(invoice_id)
        };
        let id1 = payout1.id;
        dao.create_payout(payout1)
            .await
            .unwrap();

        let result = dao
            .update_payout_status(id1, PayoutStatus::Waiting)
            .await;
        match result.unwrap_err() {
            DaoPayoutError::StatusConstraintViolation {
                current_status,
                attempted_status,
            } => {
                assert_eq!(current_status, PayoutStatus::Completed);
                assert_eq!(attempted_status, PayoutStatus::Waiting);
            },
            err => panic!("Expected StatusConstraintViolation, got: {err:?}"),
        }

        // Scenario 2: Valid transition FailedRetriable -> InProgress (retry)
        let payout2 = Payout {
            status: PayoutStatus::FailedRetriable,
            ..default_payout(invoice_id)
        };
        let id2 = payout2.id;
        dao.create_payout(payout2)
            .await
            .unwrap();

        let updated = dao
            .update_payout_status(id2, PayoutStatus::InProgress)
            .await
            .unwrap();
        assert_eq!(updated.status, PayoutStatus::InProgress);

        // Scenario 3: Invalid transition FailedRetriable -> Completed (must go through
        // InProgress)
        let payout3 = Payout {
            status: PayoutStatus::FailedRetriable,
            ..default_payout(invoice_id)
        };
        let id3 = payout3.id;
        dao.create_payout(payout3)
            .await
            .unwrap();

        let result = dao
            .update_payout_status(id3, PayoutStatus::Completed)
            .await;
        match result.unwrap_err() {
            DaoPayoutError::StatusConstraintViolation {
                current_status,
                attempted_status,
            } => {
                assert_eq!(
                    current_status,
                    PayoutStatus::FailedRetriable
                );
                assert_eq!(
                    attempted_status,
                    PayoutStatus::Completed
                );
            },
            err => panic!("Expected StatusConstraintViolation, got: {err:?}"),
        }
    }
}
