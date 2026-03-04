use alloy::primitives::Address;
use chrono::{
    DateTime,
    Utc,
};
use thiserror::Error;
use uuid::Uuid;
use sqlx::types::{Text, Json};
use rust_decimal::Decimal;

use crate::types::{
    CreateFrontEndSwapParams,
    FrontEndSwap,
    Swap,
    CreateSwapData,
    SwapStatus,
    SwapChainType,
    SwapExecutorType,
    InternalSwapDetails,
};

use super::DaoExecutor;
use super::error_parsing::{
    StatusTriggerError,
    StatusTransitionError,
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
        }
    }
}

#[derive(sqlx::FromRow)]
struct SwapRow {
    id: Uuid,
    #[sqlx(flatten)]
    request: CreateSwapDataRow,
    status: SwapStatus,
    estimated_to_amount: Text<Decimal>,  // approximate
    swap_details: Json<InternalSwapDetails>,
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
            DaoSwapError::NotFound { .. } => "ENTITY_NOT_FOUND",
            DaoSwapError::StatusConstraintViolation { .. } => "STATUS_CONSTRAINT_VIOLATION",
        }
    }

    fn code(&self) -> &str {
        match self {
            DaoSwapError::NotFound { .. } => "SWAP_NOT_FOUND",
            DaoSwapError::InvoiceNotFound {
                ..
            } => "RELATED_INVOICE_NOT_FOUND",
            DaoSwapError::StatusConstraintViolation { .. } => "SWAP_STATUS_CONSTRAINT_VIOLATION",
            DaoSwapError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn message(&self) -> &str {
        match self {
            DaoSwapError::NotFound { .. } => "The requested swap was not found.",
            DaoSwapError::InvoiceNotFound {
                ..
            } => "The related invoice id was not found.",
            DaoSwapError::StatusConstraintViolation { .. } => "The requested status transition is not allowed.",
            DaoSwapError::DatabaseError => "A database error occurred.",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            DaoSwapError::NotFound { .. } => reqwest::StatusCode::NOT_FOUND,
            DaoSwapError::InvoiceNotFound {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoSwapError::StatusConstraintViolation { .. } => reqwest::StatusCode::CONFLICT,
            DaoSwapError::DatabaseError => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        }
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
            "INSERT INTO swaps (id, invoice_id, swap_executor, from_chain, to_chain, from_token_address, to_token_address, from_amount_units, expected_to_amount_units, from_address, to_address, status, estimated_to_amount, swap_details, created_at, valid_till)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(swap.status)
        .bind(Text(swap.estimated_to_amount))
        .bind(Json(swap.swap_details))
        .bind(swap.created_at.to_rfc3339())
        .bind(swap.valid_till.to_rfc3339());

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
            RETURNING *"
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

    async fn update_swap_submitted(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "UPDATE swaps
            SET status = 'Submitted', submitted_at = datetime('now')
            WHERE id = ?
            RETURNING *"
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
                    _ => DaoSwapError::DatabaseError
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
            RETURNING *"
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
                    _ => DaoSwapError::DatabaseError
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
            RETURNING *"
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
                    _ => DaoSwapError::DatabaseError
                }
            })
    }

    #[cfg_attr(not(test), expect(dead_code))]
    async fn get_swap_by_id(&self, swap_id: Uuid) -> Result<Option<Swap>, DaoSwapError> {
        let query = sqlx::query_as::<_, SwapRow>(
            "SELECT *
            FROM swaps
            WHERE id = ?"
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
    use crate::{dao::{create_test_dao, invoice::DaoInvoiceMethods}, types::{default_create_invoice_data, default_swap}};

    use super::*;

    #[tokio::test]
    async fn test_swap_dao() {
        let dao = create_test_dao().await;

        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;

        dao.create_invoice(invoice).await.unwrap();

        let swap = default_swap(invoice_id);
        let swap_id = swap.id;
        let result = dao.create_swap(swap.clone()).await.unwrap();
        assert_eq!(result, swap);

        let submitted_swaps = dao.get_submitted_swaps().await.unwrap();
        assert!(submitted_swaps.is_empty());

        let mut submitted = dao.update_swap_submitted(swap_id).await.unwrap();
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

        let mut pending = dao.get_swap_by_id(swap_id).await.unwrap().unwrap();
        pending.trunc_timestamps();

        let mut expected_pending = Swap {
            status: SwapStatus::Pending,
            ..expected_submitted
        };
        expected_pending.trunc_timestamps();

        assert_eq!(pending, expected_pending);

        let mut completed = dao.update_swap_completed(swap_id).await.unwrap();
        completed.trunc_timestamps();

        let mut expected_completed = Swap {
            status: SwapStatus::Completed,
            finished_at: Some(Utc::now()),
            ..expected_pending
        };
        expected_completed.trunc_timestamps();

        assert_eq!(completed, expected_completed);

        // Create another swap for testing failure flow and some status transition constraints
        let swap = default_swap(invoice_id);
        let swap_id = swap.id;
        let result = dao.create_swap(swap.clone()).await.unwrap();
        assert_eq!(result, swap);

        let completed_err = dao.update_swap_completed(swap_id).await.unwrap_err();
        assert_eq!(completed_err, DaoSwapError::StatusConstraintViolation { current_status: SwapStatus::Created, attempted_status: SwapStatus::Completed });

        let pending = dao.get_submitted_swaps().await.unwrap();
        assert!(pending.is_empty());

        let mut submitted = dao.update_swap_submitted(swap_id).await.unwrap();
        submitted.trunc_timestamps();

        let mut expected_submitted = Swap {
            status: SwapStatus::Submitted,
            submitted_at: Some(Utc::now()),
            ..swap
        };
        expected_submitted.trunc_timestamps();

        assert_eq!(submitted, expected_submitted);

        let completed_err = dao.update_swap_completed(swap_id).await.unwrap_err();
        assert_eq!(completed_err, DaoSwapError::StatusConstraintViolation { current_status: SwapStatus::Submitted, attempted_status: SwapStatus::Completed });

        let submitted_swaps = dao.get_submitted_swaps().await.unwrap();
        assert_eq!(submitted_swaps.len(), 1);
        assert_eq!(submitted_swaps[0].id, swap.id);

        let mut failed = dao.update_swap_failed(swap_id, "Failure message".to_string()).await.unwrap();
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

        let result = dao.update_swap_submitted(swap_id).await.unwrap_err();
        assert_eq!(result, DaoSwapError::NotFound { swap_id });

        let result = dao.update_swap_completed(swap_id).await.unwrap_err();
        assert_eq!(result, DaoSwapError::NotFound { swap_id });

        let result = dao.update_swap_failed(swap_id, "message".to_string()).await.unwrap_err();
        assert_eq!(result, DaoSwapError::NotFound { swap_id });

        let invoice_id = Uuid::new_v4();
        let swap = default_swap(invoice_id);

        let result = dao.create_swap(swap).await.unwrap_err();
        assert_eq!(result, DaoSwapError::InvoiceNotFound { invoice_id });
    }
}
