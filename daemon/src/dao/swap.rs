use alloy::primitives::Address;
use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use sqlx::types::{
    Json,
    Text,
};
use thiserror::Error;
use uuid::Uuid;

use sqlx::QueryBuilder;

use crate::types::{
    CreateFrontEndSwapParams,
    CreateSwapData,
    FrontEndSwap,
    ListSwapsParams,
    Swap,
    SwapChainType,
    SwapDetails,
    SwapDirection,
    SwapExecutorType,
    SwapStatus,
};

use super::DaoExecutor;
use super::error_parsing::{
    StatusTransitionError,
    StatusTriggerError,
};

#[derive(sqlx::FromRow)]
struct FrontEndSwapRow {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub from_amount_units: Text<u128>,
    pub from_chain_id: u32,
    pub from_asset_id: Text<Address>,
    pub transaction_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<FrontEndSwapRow> for FrontEndSwap {
    fn from(value: FrontEndSwapRow) -> Self {
        Self {
            id: value.id,
            invoice_id: value.invoice_id,
            from_amount_units: value.from_amount_units.0,
            from_chain_id: value.from_chain_id,
            from_asset_id: value.from_asset_id.0,
            transaction_hash: value.transaction_hash,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct CreateSwapDataRow {
    invoice_id: Uuid,
    swap_executor: SwapExecutorType,
    from_chain: SwapChainType,
    to_chain: SwapChainType,
    from_token_address: String,
    to_token_address: String,
    from_amount_units: Text<u128>,
    expected_to_amount_units: Text<u128>,
    from_address: String,
    to_address: String,
    direction: SwapDirection,
}

impl From<CreateSwapDataRow> for CreateSwapData {
    fn from(value: CreateSwapDataRow) -> Self {
        Self {
            invoice_id: value.invoice_id,
            swap_executor: value.swap_executor,
            from_chain: value.from_chain,
            to_chain: value.to_chain,
            from_token_address: value.from_token_address,
            to_token_address: value.to_token_address,
            from_amount_units: value.from_amount_units.0,
            expected_to_amount_units: value.expected_to_amount_units.0,
            from_address: value.from_address,
            to_address: value.to_address,
            direction: value.direction,
        }
    }
}

#[derive(sqlx::FromRow)]
struct SwapRow {
    id: Uuid,
    #[sqlx(flatten)]
    request: CreateSwapDataRow,
    status: SwapStatus,
    estimated_to_amount: Text<Decimal>, // approximate
    swap_details: Json<SwapDetails>,
    created_at: DateTime<Utc>,
    submitted_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    valid_till: DateTime<Utc>,
    error_message: Option<String>,
}

impl From<SwapRow> for Swap {
    fn from(value: SwapRow) -> Self {
        Self {
            id: value.id,
            request: value.request.into(),
            status: value.status,
            estimated_to_amount: value.estimated_to_amount.0,
            swap_details: value.swap_details.0,
            created_at: value.created_at,
            submitted_at: value.submitted_at,
            finished_at: value.finished_at,
            valid_till: value.valid_till,
            error_message: value.error_message,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Error)]
pub enum DaoSwapError {
    #[error("Swap not found: {swap_id}")]
    NotFound { swap_id: Uuid },

    #[error("Cannot transition from {current_status} to {attempted_status}")]
    StatusConstraintViolation {
        current_status: SwapStatus,
        attempted_status: SwapStatus,
    },

    /// Referenced invoice doesn't exist (foreign key violation)
    #[error("Invoice not found: {invoice_id}")]
    InvoiceNotFound { invoice_id: Uuid },

    #[error("Database error during swap operation")]
    DatabaseError,
}

impl From<StatusTriggerError<SwapStatus>> for DaoSwapError {
    fn from(e: StatusTriggerError<SwapStatus>) -> Self {
        DaoSwapError::StatusConstraintViolation {
            current_status: e.old_status,
            attempted_status: e.new_status,
        }
    }
}

impl StatusTransitionError for SwapStatus {
    type ErrorType = DaoSwapError;

    const ERROR_TYPE_PREFIX: &'static str = "SWAP_STATUS_TRANSITION|";
}

impl crate::api::ApiErrorExt for DaoSwapError {
    // TODO: create enum for categories and codes
    fn category(&self) -> &str {
        match self {
            DaoSwapError::InvoiceNotFound {
                ..
            } => "RELATED_ENTITY_NOT_FOUND",
            DaoSwapError::DatabaseError => "INTERNAL_SERVER_ERROR",
            DaoSwapError::NotFound {
                ..
            } => "ENTITY_NOT_FOUND",
            DaoSwapError::StatusConstraintViolation {
                ..
            } => "STATUS_CONSTRAINT_VIOLATION",
        }
    }

    fn code(&self) -> &str {
        match self {
            DaoSwapError::NotFound {
                ..
            } => "SWAP_NOT_FOUND",
            DaoSwapError::InvoiceNotFound {
                ..
            } => "RELATED_INVOICE_NOT_FOUND",
            DaoSwapError::StatusConstraintViolation {
                ..
            } => "SWAP_STATUS_CONSTRAINT_VIOLATION",
            DaoSwapError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn message(&self) -> &str {
        match self {
            DaoSwapError::NotFound {
                ..
            } => "The requested swap was not found.",
            DaoSwapError::InvoiceNotFound {
                ..
            } => "The related invoice id was not found.",
            DaoSwapError::StatusConstraintViolation {
                ..
            } => "The requested status transition is not allowed.",
            DaoSwapError::DatabaseError => "A database error occurred.",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            DaoSwapError::NotFound {
                ..
            } => reqwest::StatusCode::NOT_FOUND,
            DaoSwapError::InvoiceNotFound {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoSwapError::StatusConstraintViolation {
                ..
            } => reqwest::StatusCode::CONFLICT,
            DaoSwapError::DatabaseError => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(sqlx::FromRow)]
struct CountRow {
    count: i64,
}

fn push_swap_filters(
    builder: &mut QueryBuilder<'_, sqlx::Sqlite>,
    params: &ListSwapsParams,
) {
    if let Some(statuses) = &params.status
        && !statuses.is_empty()
    {
        builder.push(" AND s.status IN (");
        let mut separated = builder.separated(", ");
        for status in statuses {
            separated.push_bind(status.to_string());
        }
        separated.push_unseparated(")");
    }

    if let Some(executor) = &params.swap_executor {
        builder.push(" AND s.swap_executor = ");
        builder.push_bind(executor.to_string());
    }

    if let Some(invoice_id) = &params.invoice_id {
        builder.push(" AND s.invoice_id = ");
        builder.push_bind(*invoice_id);
    }

    if let Some(created_from) = &params.created_from {
        builder.push(" AND s.created_at >= ");
        builder.push_bind(created_from.naive_utc());
    }

    if let Some(created_to) = &params.created_to {
        builder.push(" AND s.created_at <= ");
        builder.push_bind(created_to.naive_utc());
    }
}

pub trait DaoSwapMethods: DaoExecutor + 'static {
    async fn create_front_end_swap(
        &self,
        swap: CreateFrontEndSwapParams,
    ) -> Result<FrontEndSwap, DaoSwapError> {
        let query = sqlx::query_as::<_, FrontEndSwapRow>(
            "INSERT INTO front_end_swaps (id, invoice_id, from_amount_units, from_chain_id, from_asset_id, transaction_hash)
            VALUES (?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
        .bind(Uuid::new_v4())
        .bind(swap.invoice_id)
        .bind(Text(swap.from_amount_units))
        .bind(swap.from_chain_id)
        .bind(Text(swap.from_asset_id))
        .bind(&swap.transaction_hash);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "create_front_end_swap",
                    invoice_id = %swap.invoice_id,
                    transaction_hash = %swap.transaction_hash,
                    error.source = ?e,
                    "Failed to create front end swap"
                );

                match &e {
                    sqlx::Error::Database(db_err) => {
                        let message = db_err.message();

                        if message.contains("FOREIGN KEY") {
                            return DaoSwapError::InvoiceNotFound {
                                invoice_id: swap.invoice_id,
                            };
                        }

                        DaoSwapError::DatabaseError
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    async fn get_all_front_end_swaps(&self) -> Result<Vec<FrontEndSwap>, DaoSwapError> {
        let query = sqlx::query_as::<_, FrontEndSwapRow>(
            "SELECT *
            FROM front_end_swaps",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "get_all_front_end_swaps",
                    error.source = ?e,
                    "Failed to fetch all front end swaps"
                );
                DaoSwapError::DatabaseError
            })
    }

    async fn create_swap(
        &self,
        swap: impl Into<Swap>,
    ) -> Result<Swap, DaoSwapError> {
        let swap = swap.into();
        let invoice_id = swap.request.invoice_id;

        let query = sqlx::query_as::<_, SwapRow>(
            "INSERT INTO swaps (id, invoice_id, swap_executor, from_chain, to_chain, from_token_address, to_token_address, from_amount_units, expected_to_amount_units, from_address, to_address, direction, status, estimated_to_amount, swap_details, created_at, valid_till)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
        .bind(swap.id)
        .bind(swap.request.invoice_id)
        .bind(swap.request.swap_executor)
        .bind(swap.request.from_chain)
        .bind(swap.request.to_chain)
        .bind(Text(swap.request.from_token_address))
        .bind(Text(swap.request.to_token_address))
        .bind(Text(swap.request.from_amount_units))
        .bind(Text(swap.request.expected_to_amount_units))
        .bind(Text(swap.request.from_address))
        .bind(Text(swap.request.to_address))
        .bind(swap.request.direction)
        .bind(swap.status)
        .bind(Text(swap.estimated_to_amount))
        .bind(Json(swap.swap_details))
        .bind(swap.created_at.naive_utc())
        .bind(swap.valid_till.naive_utc());

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "create_swap",
                    error.source = ?e,
                    swap.id = %swap.id,
                );

                match &e {
                    sqlx::Error::Database(db_err) => {
                        let message = db_err.message();

                        if message.contains("FOREIGN KEY") {
                            return DaoSwapError::InvoiceNotFound {
                                invoice_id,
                            };
                        }

                        DaoSwapError::DatabaseError
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    async fn get_submitted_swaps(&self) -> Result<Vec<Swap>, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET status = 'Pending'
            WHERE status = 'Submitted'
            RETURNING *",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "get_submitted_swaps",
                    error.source = ?e,
                    "Failed get get submitted swaps and set their status to 'Pending'"
                );

                DaoSwapError::DatabaseError
            })
    }

    async fn get_pending_swaps(&self) -> Result<Vec<Swap>, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "SELECT *
            FROM swaps
            WHERE status = 'Pending'",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "get_pending_swaps",
                    error.source = ?e,
                    "Failed get get pending swaps"
                );

                DaoSwapError::DatabaseError
            })
    }

    async fn update_swap_submitted(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET status = 'Submitted', submitted_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(swap_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "update_swap_submitted",
                    %swap_id,
                    error.source = ?e,
                    "Failed to update swap as submitted"
                );

                if let Some(error) = SwapStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoSwapError::NotFound {
                        swap_id,
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    async fn update_swap_set_signature(
        &self,
        swap_id: Uuid,
        signature: String,
    ) -> Result<Swap, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET swap_details = json_set(
                    swap_details,
                    '$.signature', ?
                )
            WHERE id = ?
            RETURNING *",
        )
        .bind(signature)
        .bind(swap_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "update_swap_set_signature",
                    %swap_id,
                    error.source = ?e,
                    "Failed to update swap signature"
                );

                if let Some(error) = SwapStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoSwapError::NotFound {
                        swap_id,
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    // TODO: probably this might be unified for all front-end submitting swaps. We
    // can provide some "common" structure like
    // ```
    // struct FrontEndSubmittableSwapData<T> {
    //     transaction_hash: Option<String>,
    //     data: T,
    // }
    // ```
    async fn update_swap_submitted_with_hash(
        &self,
        swap_id: Uuid,
        transaction_hash: String,
    ) -> Result<Swap, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET status = 'Submitted',
                submitted_at = datetime('now'),
                swap_details = json_set(
                    swap_details,
                    '$.transaction_hash', ?
                )
            WHERE id = ?
            RETURNING *",
        )
        .bind(transaction_hash)
        .bind(swap_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "update_swap_submitted_with_hash",
                    %swap_id,
                    error.source = ?e,
                    "Failed to update swap as submitted with transaction"
                );

                if let Some(error) = SwapStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoSwapError::NotFound {
                        swap_id,
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    async fn update_swap_completed(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET status = 'Completed', finished_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(swap_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "update_swap_completed",
                    %swap_id,
                    error.source = ?e,
                    "Failed to update swap as completed"
                );

                if let Some(error) = SwapStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoSwapError::NotFound {
                        swap_id,
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    async fn update_swap_failed(
        &self,
        swap_id: Uuid,
        error_message: String,
    ) -> Result<Swap, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET status = 'Failed', error_message = ?, finished_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(error_message.to_string())
        .bind(swap_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "update_swap_completed",
                    %swap_id,
                    error.source = ?e,
                    "Failed to update swap as completed"
                );

                if let Some(error) = SwapStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoSwapError::NotFound {
                        swap_id,
                    },
                    _ => DaoSwapError::DatabaseError,
                }
            })
    }

    /// Get a paginated, filtered list of swaps.
    async fn get_swaps_paginated(
        &self,
        params: &ListSwapsParams,
    ) -> Result<Vec<Swap>, DaoSwapError> {
        let mut builder = QueryBuilder::new("SELECT * FROM swaps s WHERE 1=1");

        push_swap_filters(&mut builder, params);

        let sort_order = params.sort_order.unwrap_or_default();

        builder.push(" ORDER BY s.created_at ");
        builder.push(sort_order.as_sql());

        let per_page = params.pagination.validated_per_page();
        let offset = params.pagination.offset();

        builder.push(" LIMIT ");
        builder.push_bind(per_page);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let query = builder.build_query_as::<SwapRow>();

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "get_swaps_paginated",
                    error.source = ?e,
                    "Failed to fetch paginated swaps"
                );
                DaoSwapError::DatabaseError
            })
    }

    /// Count swaps matching the given filters (for pagination metadata).
    async fn count_swaps(
        &self,
        params: &ListSwapsParams,
    ) -> Result<u32, DaoSwapError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) as count FROM swaps s WHERE 1=1");

        push_swap_filters(&mut builder, params);

        let query = builder.build_query_as::<CountRow>();

        let row: CountRow = self
            .fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "count_swaps",
                    error.source = ?e,
                    "Failed to count swaps"
                );
                DaoSwapError::DatabaseError
            })?;

        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(row.count as u32)
    }

    async fn get_swap_by_id(
        &self,
        swap_id: Uuid,
    ) -> Result<Option<Swap>, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "SELECT *
            FROM swaps
            WHERE id = ?",
        )
        .bind(swap_id);

        self.fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.swap",
                    error.operation = "get_swap_by_id",
                    %swap_id,
                    error.source = ?e,
                    "Failed to get swap by id"
                );

                DaoSwapError::DatabaseError
            })
    }
}

impl<T: DaoExecutor + 'static> DaoSwapMethods for T {}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::clients::RawSwapDetails;
    use crate::dao::create_test_dao;
    use crate::dao::invoice::DaoInvoiceMethods;
    use crate::types::{
        ChainType,
        CreateInvoiceData,
        InvoiceCart,
        ListSwapsParams,
        PaginationParams,
        SortOrder,
        default_create_invoice_data,
        default_swap,
    };

    use super::*;

    #[tokio::test]
    async fn test_swap_dao() {
        let dao = create_test_dao().await;

        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;

        dao.create_invoice(invoice)
            .await
            .unwrap();

        let swap = default_swap(invoice_id);
        let swap_id = swap.id;
        let result = dao
            .create_swap(swap.clone())
            .await
            .unwrap();
        assert_eq!(result, swap);

        let submitted_swaps = dao.get_submitted_swaps().await.unwrap();
        assert!(submitted_swaps.is_empty());

        let mut submitted = dao
            .update_swap_submitted(swap_id)
            .await
            .unwrap();
        submitted.trunc_timestamps();

        // TODO: add methods for cutting timestamps for testing
        let mut expected_submitted = Swap {
            status: SwapStatus::Submitted,
            submitted_at: Some(Utc::now()),
            ..swap
        };
        expected_submitted.trunc_timestamps();

        assert_eq!(submitted, expected_submitted);
        assert_eq!(submitted.status, SwapStatus::Submitted);
        assert!(submitted.submitted_at.is_some());

        let submitted_swaps = dao.get_submitted_swaps().await.unwrap();
        assert_eq!(submitted_swaps.len(), 1);
        assert_eq!(submitted_swaps[0].id, swap.id);

        let mut pending = dao
            .get_swap_by_id(swap_id)
            .await
            .unwrap()
            .unwrap();
        pending.trunc_timestamps();

        let mut expected_pending = Swap {
            status: SwapStatus::Pending,
            ..expected_submitted
        };
        expected_pending.trunc_timestamps();

        assert_eq!(pending, expected_pending);

        let mut completed = dao
            .update_swap_completed(swap_id)
            .await
            .unwrap();
        completed.trunc_timestamps();

        let mut expected_completed = Swap {
            status: SwapStatus::Completed,
            finished_at: Some(Utc::now()),
            ..expected_pending
        };
        expected_completed.trunc_timestamps();

        assert_eq!(completed, expected_completed);

        // Create another swap for testing failure flow and some status transition
        // constraints
        let swap = default_swap(invoice_id);
        let swap_id = swap.id;
        let result = dao
            .create_swap(swap.clone())
            .await
            .unwrap();
        assert_eq!(result, swap);

        let completed_err = dao
            .update_swap_completed(swap_id)
            .await
            .unwrap_err();
        assert_eq!(
            completed_err,
            DaoSwapError::StatusConstraintViolation {
                current_status: SwapStatus::Created,
                attempted_status: SwapStatus::Completed
            }
        );

        let pending = dao.get_submitted_swaps().await.unwrap();
        assert!(pending.is_empty());

        let mut submitted = dao
            .update_swap_submitted_with_hash(
                swap_id,
                "transaction_hash123".to_string(),
            )
            .await
            .unwrap();
        submitted.trunc_timestamps();

        let mut expected_submitted = Swap {
            status: SwapStatus::Submitted,
            submitted_at: Some(Utc::now()),
            ..swap
        };
        let RawSwapDetails::Across(ref _details) = expected_submitted
            .swap_details
            .raw_transaction
        else {
            panic!("Not across internal swap details");
        };
        expected_submitted
            .swap_details
            .transaction_hash = Some("transaction_hash123".to_string());
        expected_submitted.trunc_timestamps();

        assert_eq!(submitted, expected_submitted);

        let completed_err = dao
            .update_swap_completed(swap_id)
            .await
            .unwrap_err();
        assert_eq!(
            completed_err,
            DaoSwapError::StatusConstraintViolation {
                current_status: SwapStatus::Submitted,
                attempted_status: SwapStatus::Completed
            }
        );

        let submitted_swaps = dao.get_submitted_swaps().await.unwrap();
        assert_eq!(submitted_swaps.len(), 1);
        assert_eq!(submitted_swaps[0].id, swap.id);

        let mut failed = dao
            .update_swap_failed(swap_id, "Failure message".to_string())
            .await
            .unwrap();
        failed.trunc_timestamps();

        let mut expected_failed = Swap {
            status: SwapStatus::Failed,
            finished_at: Some(Utc::now()),
            error_message: Some("Failure message".to_string()),
            ..expected_submitted
        };
        expected_failed.trunc_timestamps();

        assert_eq!(failed, expected_failed);

        // Check correct "Not Found" errors and foreign key error
        let swap_id = Uuid::new_v4();

        let result = dao
            .update_swap_submitted(swap_id)
            .await
            .unwrap_err();
        assert_eq!(
            result,
            DaoSwapError::NotFound {
                swap_id
            }
        );

        let result = dao
            .update_swap_completed(swap_id)
            .await
            .unwrap_err();
        assert_eq!(
            result,
            DaoSwapError::NotFound {
                swap_id
            }
        );

        let result = dao
            .update_swap_failed(swap_id, "message".to_string())
            .await
            .unwrap_err();
        assert_eq!(
            result,
            DaoSwapError::NotFound {
                swap_id
            }
        );

        let invoice_id = Uuid::new_v4();
        let swap = default_swap(invoice_id);

        let result = dao.create_swap(swap).await.unwrap_err();
        assert_eq!(
            result,
            DaoSwapError::InvoiceNotFound {
                invoice_id
            }
        );
    }

    // ========================================================================
    // Paginated swap listing — snapshot tests
    // ========================================================================

    /// Helper to create an invoice for seeding.
    fn make_invoice(
        chain: ChainType,
        asset_id: &str,
    ) -> CreateInvoiceData {
        let id = Uuid::new_v4();
        CreateInvoiceData {
            id,
            order_id: id.to_string(),
            asset_id: asset_id.to_string(),
            asset_name: "USDC".to_string(),
            chain,
            amount: Decimal::new(10000, 2),
            payment_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
            cart: InvoiceCart::empty(),
            redirect_url: "http://localhost:8080/thankyou".to_string(),
            #[expect(clippy::arithmetic_side_effects)]
            valid_till: chrono::Utc::now() + chrono::Duration::hours(24),
        }
    }

    /// Helper to create a swap with specific properties.
    fn make_swap(
        invoice_id: Uuid,
        executor: SwapExecutorType,
        from_chain: SwapChainType,
        to_chain: SwapChainType,
    ) -> Swap {
        Swap {
            request: CreateSwapData {
                swap_executor: executor,
                from_chain,
                to_chain,
                ..crate::types::default_create_swap_data(invoice_id)
            },
            ..default_swap(invoice_id)
        }
    }

    /// Seed 8 swaps with diverse properties, return their IDs in insertion
    /// order. A small sleep separates the first 4 from the last 4 to allow
    /// date range filtering tests.
    ///
    /// | # | Executor | From      | To      | Status    | Invoice |
    /// |---|----------|-----------|---------|-----------|---------|
    /// | 1 | Across   | Base      | Polygon | Created   | inv_1   |
    /// | 2 | Bungee   | Ethereum  | Polygon | Submitted | inv_1   |
    /// | 3 | Across   | Arbitrum  | Polygon | Pending   | inv_2   |
    /// | 4 | Bungee   | Base      | Polygon | Completed | inv_2   |
    /// |   |          |           |         | (sleep)   |         |
    /// | 5 | Across   | Ethereum  | Polygon | Failed    | inv_1   |
    /// | 6 | Bungee   | Arbitrum  | Polygon | Created   | inv_2   |
    /// | 7 | Across   | Base      | Polygon | Completed | inv_1   |
    /// | 8 | Bungee   | Ethereum  | Polygon | Pending   | inv_2   |
    async fn seed_swaps(dao: &crate::dao::DAO) -> (Vec<Uuid>, Vec<Uuid>) {
        let mut swap_ids = Vec::new();

        let inv_1 = make_invoice(ChainType::PolkadotAssetHub, "1984");
        let inv_1_id = inv_1.id;
        dao.create_invoice(inv_1).await.unwrap();

        let inv_2 = make_invoice(ChainType::Polygon, "USDC");
        let inv_2_id = inv_2.id;
        dao.create_invoice(inv_2).await.unwrap();

        let invoice_ids = vec![inv_1_id, inv_2_id];

        // --- First batch ---

        // Swap 1: Across, Base→Polygon, Created, inv_1
        let s = make_swap(
            inv_1_id,
            SwapExecutorType::Across,
            SwapChainType::Base,
            SwapChainType::Polygon,
        );
        swap_ids.push(s.id);
        dao.create_swap(s).await.unwrap();

        // Swap 2: Bungee, Ethereum→Polygon, Submitted, inv_1
        let s = make_swap(
            inv_1_id,
            SwapExecutorType::Bungee,
            SwapChainType::Ethereum,
            SwapChainType::Polygon,
        );
        let s_id = s.id;
        swap_ids.push(s_id);
        dao.create_swap(s).await.unwrap();
        dao.update_swap_submitted(s_id)
            .await
            .unwrap();

        // Swap 3: Across, Arbitrum→Polygon, Pending, inv_2
        let s = make_swap(
            inv_2_id,
            SwapExecutorType::Across,
            SwapChainType::Arbitrum,
            SwapChainType::Polygon,
        );
        let s_id = s.id;
        swap_ids.push(s_id);
        dao.create_swap(s).await.unwrap();
        dao.update_swap_submitted(s_id)
            .await
            .unwrap();
        // get_submitted_swaps transitions Submitted → Pending
        dao.get_submitted_swaps().await.unwrap();

        // Swap 4: Bungee, Base→Polygon, Completed, inv_2
        let s = make_swap(
            inv_2_id,
            SwapExecutorType::Bungee,
            SwapChainType::Base,
            SwapChainType::Polygon,
        );
        let s_id = s.id;
        swap_ids.push(s_id);
        dao.create_swap(s).await.unwrap();
        dao.update_swap_submitted(s_id)
            .await
            .unwrap();
        dao.get_submitted_swaps().await.unwrap();
        dao.update_swap_completed(s_id)
            .await
            .unwrap();

        // Sleep to create a timestamp gap between batches
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;

        // --- Second batch ---

        // Swap 5: Across, Ethereum→Polygon, Failed, inv_1
        let s = make_swap(
            inv_1_id,
            SwapExecutorType::Across,
            SwapChainType::Ethereum,
            SwapChainType::Polygon,
        );
        let s_id = s.id;
        swap_ids.push(s_id);
        dao.create_swap(s).await.unwrap();
        dao.update_swap_failed(s_id, "Network error".to_string())
            .await
            .unwrap();

        // Swap 6: Bungee, Arbitrum→Polygon, Created, inv_2
        let s = make_swap(
            inv_2_id,
            SwapExecutorType::Bungee,
            SwapChainType::Arbitrum,
            SwapChainType::Polygon,
        );
        swap_ids.push(s.id);
        dao.create_swap(s).await.unwrap();

        // Swap 7: Across, Base→Polygon, Completed, inv_1
        let s = make_swap(
            inv_1_id,
            SwapExecutorType::Across,
            SwapChainType::Base,
            SwapChainType::Polygon,
        );
        let s_id = s.id;
        swap_ids.push(s_id);
        dao.create_swap(s).await.unwrap();
        dao.update_swap_submitted(s_id)
            .await
            .unwrap();
        dao.get_submitted_swaps().await.unwrap();
        dao.update_swap_completed(s_id)
            .await
            .unwrap();

        // Swap 8: Bungee, Ethereum→Polygon, Pending, inv_2
        let s = make_swap(
            inv_2_id,
            SwapExecutorType::Bungee,
            SwapChainType::Ethereum,
            SwapChainType::Polygon,
        );
        let s_id = s.id;
        swap_ids.push(s_id);
        dao.create_swap(s).await.unwrap();
        dao.update_swap_submitted(s_id)
            .await
            .unwrap();
        dao.get_submitted_swaps().await.unwrap();

        (swap_ids, invoice_ids)
    }

    #[tokio::test]
    async fn test_paginated_swaps_no_filters() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        let params = ListSwapsParams::default();
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 8);
        assert_eq!(result.len(), 8);
        insta::assert_yaml_snapshot!(result, {
            "[].id" => "[uuid]",
            "[].request.invoice_id" => "[uuid]",
            "[].created_at" => "[timestamp]",
            "[].submitted_at" => "[timestamp]",
            "[].finished_at" => "[timestamp]",
            "[].valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_swaps_filter_single_status() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Created: s1, s6
        let params = ListSwapsParams {
            status: Some(vec![SwapStatus::Created]),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 2);
        assert_eq!(result.len(), 2);
        for swap in &result {
            assert_eq!(swap.status, SwapStatus::Created);
        }
    }

    #[tokio::test]
    async fn test_paginated_swaps_filter_multiple_statuses() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Created + Completed: s1, s4, s6, s7
        let params = ListSwapsParams {
            status: Some(vec![
                SwapStatus::Created,
                SwapStatus::Completed,
            ]),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn test_paginated_swaps_filter_by_executor() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Across: s1, s3, s5, s7
        let params = ListSwapsParams {
            swap_executor: Some(SwapExecutorType::Across),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
        for swap in &result {
            assert_eq!(
                swap.request.swap_executor,
                SwapExecutorType::Across
            );
        }

        // Bungee: s2, s4, s6, s8
        let params = ListSwapsParams {
            swap_executor: Some(SwapExecutorType::Bungee),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
        for swap in &result {
            assert_eq!(
                swap.request.swap_executor,
                SwapExecutorType::Bungee
            );
        }
    }

    #[tokio::test]
    async fn test_paginated_swaps_filter_by_invoice_id() {
        let dao = create_test_dao().await;
        let (_, invoice_ids) = seed_swaps(&dao).await;

        // inv_1: s1, s2, s5, s7
        let params = ListSwapsParams {
            invoice_id: Some(invoice_ids[0]),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
        for swap in &result {
            assert_eq!(swap.request.invoice_id, invoice_ids[0]);
        }
    }

    #[tokio::test]
    async fn test_paginated_swaps_sort_asc() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        let params = ListSwapsParams {
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();

        assert_eq!(result.len(), 8);
        for i in 1..result.len() {
            assert!(result[i].created_at >= result[i - 1].created_at);
        }
    }

    #[tokio::test]
    async fn test_paginated_swaps_pagination() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Page 1, 3 per page
        let params = ListSwapsParams {
            pagination: PaginationParams {
                page: Some(1),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 8);
        assert_eq!(result.len(), 3);

        // Page 3, 3 per page (last page, 2 items)
        let params = ListSwapsParams {
            pagination: PaginationParams {
                page: Some(3),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        assert_eq!(result.len(), 2);

        // Beyond last page
        let params = ListSwapsParams {
            pagination: PaginationParams {
                page: Some(10),
                per_page: Some(3),
            },
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_paginated_swaps_date_range() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Fetch all in DESC order to find the boundary
        let all = dao
            .get_swaps_paginated(&ListSwapsParams::default())
            .await
            .unwrap();
        // In DESC order, items 0-3 are batch 2 (newer), items 4-7 are batch 1
        // (older). Use the created_at of item at index 3 (newest of batch 2)
        // as created_to to only get batch 1 items.
        let boundary = all[4].created_at;

        let params = ListSwapsParams {
            created_to: Some(boundary),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn test_paginated_swaps_combined_filters() {
        let dao = create_test_dao().await;
        let (_, invoice_ids) = seed_swaps(&dao).await;

        // Across + inv_1: s1, s5, s7
        let params = ListSwapsParams {
            swap_executor: Some(SwapExecutorType::Across),
            invoice_id: Some(invoice_ids[0]),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 3);
        assert_eq!(result.len(), 3);
        for swap in &result {
            assert_eq!(
                swap.request.swap_executor,
                SwapExecutorType::Across
            );
            assert_eq!(swap.request.invoice_id, invoice_ids[0]);
        }
    }

    #[tokio::test]
    async fn test_paginated_swaps_empty_result() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Abandoned status doesn't exist in seed data
        let params = ListSwapsParams {
            status: Some(vec![SwapStatus::Abandoned]),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 0);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_paginated_swaps_failed_status() {
        let dao = create_test_dao().await;
        seed_swaps(&dao).await;

        // Failed: s5
        let params = ListSwapsParams {
            status: Some(vec![SwapStatus::Failed]),
            ..Default::default()
        };
        let result = dao
            .get_swaps_paginated(&params)
            .await
            .unwrap();
        let count = dao.count_swaps(&params).await.unwrap();

        assert_eq!(count, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, SwapStatus::Failed);
        assert_eq!(
            result[0].error_message,
            Some("Network error".to_string())
        );
    }
}
