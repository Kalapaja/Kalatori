use sqlx::QueryBuilder;
use sqlx::types::Text;
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    ListPayoutsParams,
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

impl crate::api::ApiErrorExt for DaoPayoutError {
    fn category(&self) -> &str {
        match self {
            DaoPayoutError::NotFound {
                ..
            } => "ENTITY_NOT_FOUND",
            DaoPayoutError::InvoiceNotFound {
                ..
            } => "RELATED_ENTITY_NOT_FOUND",
            DaoPayoutError::StatusConstraintViolation {
                ..
            } => "STATUS_CONSTRAINT_VIOLATION",
            DaoPayoutError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn code(&self) -> &str {
        match self {
            DaoPayoutError::NotFound {
                ..
            } => "PAYOUT_NOT_FOUND",
            DaoPayoutError::InvoiceNotFound {
                ..
            } => "RELATED_INVOICE_NOT_FOUND",
            DaoPayoutError::StatusConstraintViolation {
                ..
            } => "PAYOUT_STATUS_CONSTRAINT_VIOLATION",
            DaoPayoutError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn message(&self) -> &str {
        match self {
            DaoPayoutError::NotFound {
                ..
            } => "The requested payout was not found.",
            DaoPayoutError::InvoiceNotFound {
                ..
            } => "The related invoice id was not found.",
            DaoPayoutError::StatusConstraintViolation {
                ..
            } => "The requested status transition is not allowed.",
            DaoPayoutError::DatabaseError => "A database error occurred.",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            DaoPayoutError::NotFound {
                ..
            } => reqwest::StatusCode::NOT_FOUND,
            DaoPayoutError::InvoiceNotFound {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoPayoutError::StatusConstraintViolation {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoPayoutError::DatabaseError => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub trait DaoPayoutMethods: DaoExecutor + 'static {
    async fn create_payout(
        &self,
        payout: Payout,
    ) -> Result<Payout, DaoPayoutError> {
        let query = sqlx::query_as::<_, PayoutRow>(
        "INSERT INTO payouts (id, invoice_id, asset_id, asset_name, chain, source_address, destination_address, amount, destination_chain, destination_asset_id, initiator_type, initiator_id, status, created_at, updated_at, retry_count, last_attempt_at, next_retry_at, failure_message)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
            .bind(payout.id)
            .bind(payout.invoice_id)
            .bind(&payout.asset_id)
            .bind(&payout.asset_name)
            .bind(payout.chain)
            .bind(&payout.source_address)
            .bind(&payout.destination_params.destination_address)
            .bind(Text(payout.amount))
            .bind(payout.destination_params.destination_chain)
            .bind(&payout.destination_params.destination_asset_id)
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
                WHERE
                    status = 'Waiting'
                    OR (status = 'FailedRetriable' AND next_retry_at <= datetime('now'))
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

    /// Get a paginated, filtered list of payouts.
    async fn get_payouts_paginated(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<Vec<Payout>, DaoPayoutError> {
        let mut builder = QueryBuilder::new("SELECT * FROM payouts p WHERE 1=1");

        push_payout_filters(&mut builder, params);

        builder.push(" ORDER BY p.created_at ");
        builder.push(params.sort_order.as_sql());

        let per_page = params.pagination.validated_per_page();
        let offset = params.pagination.offset();

        builder.push(" LIMIT ");
        builder.push_bind(per_page);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let query = builder.build_query_as::<PayoutRow>();

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "get_payouts_paginated",
                    error.source = ?e,
                    "Failed to fetch paginated payouts"
                );
                DaoPayoutError::DatabaseError
            })
    }

    /// Count payouts matching the given filters (for pagination metadata).
    async fn count_payouts(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<u32, DaoPayoutError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) as count FROM payouts p WHERE 1=1");

        push_payout_filters(&mut builder, params);

        let query = builder.build_query_as::<CountRow>();

        let row: CountRow = self
            .fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.payout",
                    error.operation = "count_payouts",
                    error.source = ?e,
                    "Failed to count payouts"
                );
                DaoPayoutError::DatabaseError
            })?;

        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(row.count as u32)
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

#[derive(sqlx::FromRow)]
struct CountRow {
    count: i64,
}

/// Push WHERE clause conditions to the query builder based on filter params.
/// Shared between `get_payouts_paginated` and `count_payouts`.
fn push_payout_filters(
    builder: &mut QueryBuilder<'_, sqlx::Sqlite>,
    params: &ListPayoutsParams,
) {
    if let Some(statuses) = &params.status
        && !statuses.is_empty()
    {
        builder.push(" AND p.status IN (");
        let mut separated = builder.separated(", ");
        for status in statuses {
            separated.push_bind(status.to_string());
        }
        separated.push_unseparated(")");
    }

    if let Some(chain) = &params.chain {
        builder.push(" AND p.chain = ");
        builder.push_bind(chain.to_string());
    }

    if let Some(asset_id) = &params.asset_id {
        builder.push(" AND p.asset_id = ");
        builder.push_bind(asset_id.clone());
    }

    if let Some(invoice_id) = &params.invoice_id {
        builder.push(" AND p.invoice_id = ");
        builder.push_bind(*invoice_id);
    }

    if let Some(created_from) = &params.created_from {
        builder.push(" AND p.created_at >= ");
        builder.push_bind(created_from.naive_utc());
    }

    if let Some(created_to) = &params.created_to {
        builder.push(" AND p.created_at <= ");
        builder.push_bind(created_to.naive_utc());
    }
}

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
        dao.create_payout(payout1.clone())
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

        // Create payout with Waiting status and next_retry_at in the future — should
        // still be returned because all Waiting payouts are eligible regardless
        // of next_retry_at
        let mut payout4 = default_payout(invoice.id);
        payout4.retry_meta.next_retry_at = Some(Utc::now() + chrono::Duration::hours(1));
        dao.create_payout(payout4.clone())
            .await
            .unwrap();

        // Get pending payouts
        let pending = dao
            .get_pending_payouts(2)
            .await
            .unwrap();

        // Should return payout1 and payout4 (both Waiting), not payout2 (InProgress) or
        // payout3 (Completed)
        assert_eq!(pending.len(), 2);
        assert_eq!(
            pending[0].status,
            PayoutStatus::InProgress
        );
        assert_eq!(
            pending[1].status,
            PayoutStatus::InProgress
        );
        assert_eq!(pending[0].id, payout1.id);
        assert_eq!(pending[1].id, payout4.id);

        // Create FailedRetriable payout with next_retry_at in the past (should be
        // returned)
        let mut payout5 = default_payout(invoice.id);
        payout5.status = PayoutStatus::FailedRetriable;
        payout5.created_at = Utc::now() - chrono::Duration::minutes(10);
        payout5.retry_meta.next_retry_at = Some(Utc::now() - chrono::Duration::minutes(2));
        dao.create_payout(payout5.clone())
            .await
            .unwrap();

        // Create FailedRetriable payout with next_retry_at in the future (should NOT be
        // returned)
        let mut payout6 = default_payout(invoice.id);
        payout6.status = PayoutStatus::FailedRetriable;
        payout6.retry_meta.next_retry_at = Some(Utc::now() + chrono::Duration::hours(1));
        dao.create_payout(payout6)
            .await
            .unwrap();

        // Create another Waiting payout
        let payout7 = default_payout(invoice.id);
        dao.create_payout(payout7.clone())
            .await
            .unwrap();

        let pending_all = dao
            .get_pending_payouts(2)
            .await
            .unwrap();
        // payout5 (FailedRetriable, past retry, oldest) and payout7 (Waiting) picked
        // up. payout6 (FailedRetriable, future retry) excluded. Limit caps at
        // 2.
        assert_eq!(pending_all.len(), 2);
        assert_eq!(pending_all[0].id, payout5.id);
        assert_eq!(pending_all[1].id, payout7.id);
        assert_eq!(
            pending_all[0].status,
            PayoutStatus::InProgress
        );
        assert_eq!(
            pending_all[1].status,
            PayoutStatus::InProgress
        );
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

    // ========================================================================
    // Paginated payout listing — snapshot tests
    // ========================================================================

    use rust_decimal::Decimal;

    use crate::types::{
        ChainType,
        CreateInvoiceData,
        InvoiceCart,
        ListPayoutsParams,
        PaginationParams,
        SortOrder,
    };

    /// Helper to create an invoice for a given chain.
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

    /// Helper to create a payout with specific properties.
    fn make_payout(
        invoice_id: Uuid,
        chain: ChainType,
        asset_id: &str,
        amount: Decimal,
    ) -> Payout {
        Payout {
            chain,
            asset_id: asset_id.to_string(),
            asset_name: if asset_id == "1984" {
                "USDT".to_string()
            } else {
                "USDC".to_string()
            },
            amount,
            ..default_payout(invoice_id)
        }
    }

    /// Seed 8 payouts with diverse properties, return their IDs in insertion
    /// order. A small sleep separates the first 4 from the last 4 to allow
    /// date range filtering tests.
    ///
    /// | # | Chain       | Asset | Amount | Status           |
    /// |---|-------------|-------|--------|------------------|
    /// | 1 | AssetHub    | USDT  | 100.00 | Waiting          |
    /// | 2 | Polygon     | USDC  | 250.50 | Waiting          |
    /// | 3 | AssetHub    | USDT  |  75.00 | Completed        |
    /// | 4 | Polygon     | USDC  | 500.00 | InProgress       |
    /// |   |             |       |        | (sleep ~15ms)    |
    /// | 5 | AssetHub    | USDC  | 300.00 | FailedRetriable  |
    /// | 6 | Polygon     | USDT  |  42.00 | Failed           |
    /// | 7 | AssetHub    | USDT  | 180.00 | Waiting          |
    /// | 8 | Polygon     | USDC  |  99.99 | Completed        |
    async fn seed_payouts(dao: &crate::dao::DAO) -> (Vec<Uuid>, Vec<Uuid>) {
        let mut payout_ids = Vec::new();

        // Create 2 invoices (one per chain) to parent the payouts
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

        // Payout 1: AssetHub, USDT, 100.00, Waiting
        let p = make_payout(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "1984",
            Decimal::new(10000, 2),
        );
        payout_ids.push(p.id);
        dao.create_payout(p).await.unwrap();

        // Payout 2: Polygon, USDC, 250.50, Waiting
        let p = make_payout(
            inv_poly_id,
            ChainType::Polygon,
            "USDC",
            Decimal::new(25050, 2),
        );
        payout_ids.push(p.id);
        dao.create_payout(p).await.unwrap();

        // Payout 3: AssetHub, USDT, 75.00, Completed (Waiting -> InProgress ->
        // Completed)
        let p = make_payout(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "1984",
            Decimal::new(7500, 2),
        );
        let p_id = p.id;
        payout_ids.push(p_id);
        dao.create_payout(p).await.unwrap();
        dao.update_payout_status(p_id, PayoutStatus::InProgress)
            .await
            .unwrap();
        dao.update_payout_status(p_id, PayoutStatus::Completed)
            .await
            .unwrap();

        // Payout 4: Polygon, USDC, 500.00, InProgress (Waiting -> InProgress)
        let p = make_payout(
            inv_poly_id,
            ChainType::Polygon,
            "USDC",
            Decimal::new(50000, 2),
        );
        let p_id = p.id;
        payout_ids.push(p_id);
        dao.create_payout(p).await.unwrap();
        dao.update_payout_status(p_id, PayoutStatus::InProgress)
            .await
            .unwrap();

        // Sleep to create a timestamp gap between batches
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;

        // --- Second batch (after the sleep) ---

        // Payout 5: AssetHub, USDC, 300.00, FailedRetriable
        let p = make_payout(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "USDC",
            Decimal::new(30000, 2),
        );
        let p_id = p.id;
        payout_ids.push(p_id);
        dao.create_payout(p).await.unwrap();
        dao.update_payout_status(p_id, PayoutStatus::InProgress)
            .await
            .unwrap();
        let retry_meta = RetryMeta {
            retry_count: 1,
            last_attempt_at: Some(Utc::now()),
            next_retry_at: Some(Utc::now() + chrono::Duration::minutes(5)),
            failure_message: Some("Network error".to_string()),
        };
        dao.update_payout_retry(p_id, retry_meta, true)
            .await
            .unwrap();

        // Payout 6: Polygon, USDT, 42.00, Failed
        let p = make_payout(
            inv_poly_id,
            ChainType::Polygon,
            "1984",
            Decimal::new(4200, 2),
        );
        let p_id = p.id;
        payout_ids.push(p_id);
        dao.create_payout(p).await.unwrap();
        dao.update_payout_status(p_id, PayoutStatus::InProgress)
            .await
            .unwrap();
        let retry_meta = RetryMeta {
            retry_count: 5,
            last_attempt_at: Some(Utc::now()),
            next_retry_at: None,
            failure_message: Some("Permanent failure".to_string()),
        };
        dao.update_payout_retry(p_id, retry_meta, false)
            .await
            .unwrap();

        // Payout 7: AssetHub, USDT, 180.00, Waiting
        let p = make_payout(
            inv_ah_id,
            ChainType::PolkadotAssetHub,
            "1984",
            Decimal::new(18000, 2),
        );
        payout_ids.push(p.id);
        dao.create_payout(p).await.unwrap();

        // Payout 8: Polygon, USDC, 99.99, Completed
        let p = make_payout(
            inv_poly_id,
            ChainType::Polygon,
            "USDC",
            Decimal::new(9999, 2),
        );
        let p_id = p.id;
        payout_ids.push(p_id);
        dao.create_payout(p).await.unwrap();
        dao.update_payout_status(p_id, PayoutStatus::InProgress)
            .await
            .unwrap();
        dao.update_payout_status(p_id, PayoutStatus::Completed)
            .await
            .unwrap();

        (payout_ids, invoice_ids)
    }

    #[tokio::test]
    async fn test_paginated_payouts_no_filters() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        let params = ListPayoutsParams::default();
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 8);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
            "[].last_attempt_at" => "[timestamp]",
            "[].next_retry_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_filter_single_status() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // Waiting: p1, p2, p7
        let params = ListPayoutsParams {
            status: Some(vec![PayoutStatus::Waiting]),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 3);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_filter_multiple_statuses() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // Completed + InProgress: p3, p4, p8
        let params = ListPayoutsParams {
            status: Some(vec![
                PayoutStatus::Completed,
                PayoutStatus::InProgress,
            ]),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 3);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_filter_by_chain() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // Polygon: p2, p4, p6, p8
        let params = ListPayoutsParams {
            chain: Some(ChainType::Polygon),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
            "[].last_attempt_at" => "[timestamp]",
            "[].next_retry_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_filter_by_asset_id() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // USDC: p2, p4, p5, p8
        let params = ListPayoutsParams {
            asset_id: Some("USDC".to_string()),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
            "[].last_attempt_at" => "[timestamp]",
            "[].next_retry_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_filter_by_invoice_id() {
        let dao = create_test_dao().await;
        let (_payout_ids, invoice_ids) = seed_payouts(&dao).await;

        // AssetHub invoice: p1, p3, p5, p7
        let params = ListPayoutsParams {
            invoice_id: Some(invoice_ids[0]),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
            "[].last_attempt_at" => "[timestamp]",
            "[].next_retry_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_sort_asc() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        let params = ListPayoutsParams {
            sort_order: SortOrder::Asc,
            pagination: PaginationParams {
                per_page: Some(3),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();

        assert_eq!(result.len(), 3);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_pagination() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // Page 1 of 3 per page (default DESC → p8, p7, p6)
        let params = ListPayoutsParams {
            pagination: PaginationParams {
                page: Some(1),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let page1 = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        assert_eq!(page1.len(), 3);

        // Page 3 of 3 per page (p2, p1)
        let params = ListPayoutsParams {
            pagination: PaginationParams {
                page: Some(3),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let page3 = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        assert_eq!(page3.len(), 2);

        // Beyond last page
        let params = ListPayoutsParams {
            pagination: PaginationParams {
                page: Some(10),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let empty = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_paginated_payouts_date_range() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // Get the boundary: first batch created before sleep, second after.
        // Use a tight window around "now minus a little" to get only first batch.
        let all_desc = dao
            .get_payouts_paginated(&ListPayoutsParams::default())
            .await
            .unwrap();

        // The 5th item in DESC order is p4 (last of first batch)
        let boundary = all_desc[4].created_at;

        // created_to = boundary → should get first batch only (4 payouts)
        let params = ListPayoutsParams {
            created_to: Some(boundary),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn test_paginated_payouts_combined_filters() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // AssetHub + Waiting: p1, p7
        let params = ListPayoutsParams {
            chain: Some(ChainType::PolkadotAssetHub),
            status: Some(vec![PayoutStatus::Waiting]),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 2);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_payouts_empty_result() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // No payouts match this combination
        let params = ListPayoutsParams {
            chain: Some(ChainType::Polygon),
            status: Some(vec![PayoutStatus::FailedRetriable]),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 0);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_paginated_payouts_failed_statuses() {
        let dao = create_test_dao().await;
        seed_payouts(&dao).await;

        // FailedRetriable + Failed: p5, p6
        let params = ListPayoutsParams {
            status: Some(vec![
                PayoutStatus::FailedRetriable,
                PayoutStatus::Failed,
            ]),
            ..Default::default()
        };
        let result = dao
            .get_payouts_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_payouts(&params)
            .await
            .unwrap();

        assert_eq!(count, 2);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].updated_at" => "[timestamp]",
            "[].last_attempt_at" => "[timestamp]",
            "[].next_retry_at" => "[timestamp]",
        });
    }
}
