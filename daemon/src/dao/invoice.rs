use sqlx::QueryBuilder;
use sqlx::types::{
    Json,
    Text,
};
use thiserror::Error;
use uuid::Uuid;

use crate::dao::error_parsing::parse_update_not_allowed_error;
use crate::types::{
    CreateInvoiceData,
    Invoice,
    InvoiceRow,
    InvoiceStatus,
    InvoiceWithReceivedAmount,
    ListInvoicesParams,
    UpdateInvoiceData,
};

use super::DaoExecutor;
use super::error_parsing::{
    StatusTransitionError,
    StatusTriggerError,
};

#[derive(sqlx::FromRow)]
struct UuidWrapper(Uuid);

impl From<UuidWrapper> for Uuid {
    fn from(value: UuidWrapper) -> Self {
        value.0
    }
}

// ============================================================================
// Invoice Domain Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum DaoInvoiceError {
    /// Invoice not found by ID or `order_id`
    #[error("Invoice not found: {invoice_id}")]
    NotFound { invoice_id: Uuid },

    /// Update not allowed due to status
    #[error("Invoice {invoice_id} cannot be updated in its current status: {current_status}")]
    UpdateNotAllowed {
        invoice_id: Uuid,
        current_status: InvoiceStatus,
    },

    /// Status transition not allowed (invoice in wrong state)
    #[error("Cannot transition from {current_status} to {attempted_status}")]
    StatusConstraintViolation {
        current_status: InvoiceStatus,
        attempted_status: InvoiceStatus,
    },

    /// Duplicate `order_id` (UNIQUE constraint violation)
    #[error("Order ID '{order_id}' already exists")]
    DuplicateOrderId { order_id: String },

    /// Database operation failed
    #[error("Database error during invoice operation")]
    DatabaseError,
}

impl crate::api::ApiErrorExt for DaoInvoiceError {
    // TODO: create enum for categories and codes
    fn category(&self) -> &str {
        match self {
            DaoInvoiceError::NotFound {
                ..
            } => "ENTITY_NOT_FOUND",
            DaoInvoiceError::UpdateNotAllowed {
                ..
            } => "UPDATE_NOT_ALLOWED",
            DaoInvoiceError::StatusConstraintViolation {
                ..
            } => "STATUS_CONSTRAINT_VIOLATION",
            DaoInvoiceError::DuplicateOrderId {
                ..
            } => "DUPLICATE_ENTITY",
            DaoInvoiceError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn code(&self) -> &str {
        match self {
            DaoInvoiceError::NotFound {
                ..
            } => "INVOICE_NOT_FOUND",
            DaoInvoiceError::UpdateNotAllowed {
                ..
            } => "INVOICE_UPDATE_NOT_ALLOWED",
            DaoInvoiceError::StatusConstraintViolation {
                ..
            } => "INVOICE_STATUS_CONSTRAINT_VIOLATION",
            DaoInvoiceError::DuplicateOrderId {
                ..
            } => "INVOICE_DUPLICATE_ORDER_ID",
            DaoInvoiceError::DatabaseError => "INTERNAL_SERVER_ERROR",
        }
    }

    fn message(&self) -> &str {
        match self {
            DaoInvoiceError::NotFound {
                ..
            } => "The requested invoice was not found.",
            DaoInvoiceError::UpdateNotAllowed {
                ..
            } => "Invoice cannot be updated in its current status.",
            DaoInvoiceError::StatusConstraintViolation {
                ..
            } => "The requested status transition is not allowed.",
            DaoInvoiceError::DuplicateOrderId {
                ..
            } => "An invoice with the specified order ID already exists.",
            DaoInvoiceError::DatabaseError => "A database error occurred.",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            DaoInvoiceError::NotFound {
                ..
            } => reqwest::StatusCode::NOT_FOUND,
            DaoInvoiceError::UpdateNotAllowed {
                ..
            } => reqwest::StatusCode::CONFLICT,
            DaoInvoiceError::StatusConstraintViolation {
                ..
            } => reqwest::StatusCode::BAD_REQUEST,
            DaoInvoiceError::DuplicateOrderId {
                ..
            } => reqwest::StatusCode::CONFLICT,
            DaoInvoiceError::DatabaseError => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<sqlx::Error> for DaoInvoiceError {
    fn from(_e: sqlx::Error) -> Self {
        // Only convert generic database errors
        // Specific errors are handled at call site
        DaoInvoiceError::DatabaseError
    }
}

impl From<StatusTriggerError<InvoiceStatus>> for DaoInvoiceError {
    fn from(err: StatusTriggerError<InvoiceStatus>) -> Self {
        DaoInvoiceError::StatusConstraintViolation {
            current_status: err.old_status,
            attempted_status: err.new_status,
        }
    }
}

impl StatusTransitionError for InvoiceStatus {
    type ErrorType = DaoInvoiceError;

    const ERROR_TYPE_PREFIX: &'static str = "INVOICE_STATUS_TRANSITION|";
}

#[derive(sqlx::FromRow)]
struct InvoiceWithAmountsRow {
    #[sqlx(flatten)]
    invoice: InvoiceRow,
    amounts: sqlx::types::Json<Vec<String>>,
}

impl From<InvoiceWithAmountsRow> for InvoiceWithReceivedAmount {
    fn from(row: InvoiceWithAmountsRow) -> Self {
        let incoming_amount = row
            .amounts
            .0
            .into_iter()
            .filter_map(|amt_str| {
                amt_str
                    .parse::<rust_decimal::Decimal>()
                    .ok()
            })
            .sum();

        Self {
            invoice: row.invoice.into(),
            total_received_amount: incoming_amount,
        }
    }
}

pub trait DaoInvoiceMethods: DaoExecutor + 'static {
    async fn create_invoice(
        &self,
        invoice: CreateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError> {
        let invoice: Invoice = invoice.into();

        let query = sqlx::query_as::<_, InvoiceRow>(
        "INSERT INTO invoices (id, order_id, asset_id, asset_name, chain, amount, payment_address, status, cart, redirect_url, valid_till, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *"
        )
            .bind(invoice.id)
            .bind(&invoice.order_id)
            .bind(invoice.asset_id)
            .bind(invoice.asset_name)
            .bind(invoice.chain)
            .bind(Text(invoice.amount))
            .bind(&invoice.payment_address)
            .bind(invoice.status)
            .bind(Json(invoice.cart))
            .bind(invoice.redirect_url)
            .bind(invoice.valid_till.naive_utc())
            .bind(invoice.created_at.naive_utc())
            .bind(invoice.updated_at.naive_utc());

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "create_invoice",
                    order_id = %invoice.order_id,
                    invoice_id = %invoice.id,
                    error.source = ?e,
                    "Failed to create invoice"
                );

                match &e {
                    sqlx::Error::Database(db_err) => {
                        let message = db_err.message();

                        if message.contains("UNIQUE") && message.contains("order_id") {
                            return DaoInvoiceError::DuplicateOrderId {
                                order_id: invoice.order_id,
                            };
                        }

                        DaoInvoiceError::DatabaseError
                    },
                    _ => DaoInvoiceError::DatabaseError,
                }
            })
    }

    async fn get_all_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceRow>(
            "SELECT *
            FROM invoices",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "get_all_invoices",
                    error.source = ?e,
                    "Failed to fetch all invoices"
                );
                DaoInvoiceError::DatabaseError
            })
    }

    #[cfg_attr(not(test), expect(dead_code))]
    async fn get_invoice_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<Invoice>, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceRow>(
            "SELECT *
            FROM invoices
            WHERE id = ?",
        )
        .bind(invoice_id);

        self.fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "get_invoice_by_id",
                    %invoice_id,
                    error.source = ?e,
                    "Failed to fetch invoice"
                );
                DaoInvoiceError::DatabaseError
            })
    }

    async fn get_invoice_with_received_amount_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceWithAmountsRow>(
            "SELECT
                i.*,
                CASE
                    WHEN COUNT(t.amount) = 0 THEN '[]'
                    ELSE json_group_array(t.amount)
                END as amounts
            FROM invoices i
            LEFT JOIN transactions t
                ON i.id = t.invoice_id
                AND t.transaction_type = 'Incoming'
            WHERE i.id = ?
            GROUP BY i.id",
        )
        .bind(invoice_id);

        self.fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "get_invoice_with_received_amount_by_id",
                    %invoice_id,
                    error.source = ?e,
                    "Failed to fetch invoice with received amount"
                );
                DaoInvoiceError::DatabaseError
            })
    }

    /// Get all active invoices that need to be monitored and total amount of
    /// received incoming transactions. We suppose that invoices with status
    /// 'Waiting' or '`PartiallyPaid`' don't have outgoing transactions,
    /// so they are not included in calculations.
    /// Returns invoices with status 'Waiting' or '`PartiallyPaid`'
    async fn get_active_invoices_with_amounts(
        &self
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceWithAmountsRow>(
            "SELECT
                i.*,
                CASE
                    WHEN COUNT(t.amount) = 0 THEN '[]'
                    ELSE json_group_array(t.amount)
                END as amounts
            FROM invoices i
            LEFT JOIN transactions t
                ON i.id = t.invoice_id
                AND t.transaction_type = 'Incoming'
            WHERE i.status IN ('Waiting', 'PartiallyPaid')
            GROUP BY i.id
            ORDER BY i.created_at ASC",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "get_invoices_paid_amount",
                    error.source = ?e,
                    "Failed to fetch paid amounts for invoices"
                );
                DaoInvoiceError::DatabaseError
            })
    }

    async fn update_invoice_status(
        &self,
        invoice_id: Uuid,
        status: InvoiceStatus,
    ) -> Result<Invoice, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceRow>(
            "UPDATE invoices
            SET status = ?,
                updated_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(status)
        .bind(invoice_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "update_invoice_status",
                    %invoice_id,
                    new_status = ?status,
                    error.source = ?e,
                    "Failed to update invoice status"
                );

                // Check for trigger violation
                if let Some(error) = InvoiceStatus::from_sqlx_error(&e) {
                    return error;
                }

                match e {
                    sqlx::Error::RowNotFound => DaoInvoiceError::NotFound {
                        invoice_id,
                    },
                    _ => DaoInvoiceError::DatabaseError,
                }
            })
    }

    async fn update_invoice_data(
        &self,
        data: UpdateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceRow>(
            "UPDATE invoices
            SET amount = ?,
                cart = ?,
                valid_till = ?,
                updated_at = datetime('now')
            WHERE id = ?
            RETURNING *",
        )
        .bind(Text(data.amount))
        .bind(Json(data.cart))
        .bind(data.valid_till.naive_utc())
        .bind(data.invoice_id);

        self.fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "update_invoice_data",
                    invoice_id = %data.invoice_id,
                    error.source = ?e,
                    "Update failed"
                );

                // Not a trigger error, check if RowNotFound
                if let Some(current_status) =
                    parse_update_not_allowed_error(&e, "INVOICE_UPDATE_NOT_ALLOWED|")
                {
                    return DaoInvoiceError::UpdateNotAllowed {
                        invoice_id: data.invoice_id,
                        current_status,
                    };
                }

                match e {
                    sqlx::Error::RowNotFound => DaoInvoiceError::NotFound {
                        invoice_id: data.invoice_id,
                    },
                    _ => DaoInvoiceError::DatabaseError,
                }
            })
    }

    async fn get_expired_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError> {
        let query = sqlx::query_as::<_, InvoiceRow>(
            "SELECT *
            FROM invoices
            WHERE status = 'Waiting' AND valid_till < datetime('now')",
        );

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "get_expired_invoices",
                    error.source = ?e,
                    "Failed to get expired invoices"
                );
                DaoInvoiceError::DatabaseError
            })
    }

    /// Get a paginated, filtered list of invoices with their received amounts.
    async fn get_invoices_paginated(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        let mut builder = QueryBuilder::new(
            "SELECT i.*,
                CASE
                    WHEN COUNT(t.amount) = 0 THEN '[]'
                    ELSE json_group_array(t.amount)
                END as amounts
            FROM invoices i
            LEFT JOIN transactions t
                ON i.id = t.invoice_id
                AND t.transaction_type = 'Incoming'
            WHERE 1=1",
        );

        push_invoice_filters(&mut builder, params);

        let sort_order = params.sort_order.unwrap_or_default();

        builder.push(" GROUP BY i.id ORDER BY i.created_at ");
        builder.push(sort_order.as_sql());

        let per_page = params.pagination.validated_per_page();
        let offset = params.pagination.offset();

        builder.push(" LIMIT ");
        builder.push_bind(per_page);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let query = builder.build_query_as::<InvoiceWithAmountsRow>();

        self.fetch_all(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "get_invoices_paginated",
                    error.source = ?e,
                    "Failed to fetch paginated invoices"
                );
                DaoInvoiceError::DatabaseError
            })
    }

    /// Count invoices matching the given filters (for pagination metadata).
    async fn count_invoices(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<u32, DaoInvoiceError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) as count FROM invoices i WHERE 1=1");

        push_invoice_filters(&mut builder, params);

        let query = builder.build_query_as::<CountRow>();

        let row: CountRow = self
            .fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.invoice",
                    error.operation = "count_invoices",
                    error.source = ?e,
                    "Failed to count invoices"
                );
                DaoInvoiceError::DatabaseError
            })?;

        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(row.count as u32)
    }
}

impl<T: DaoExecutor + 'static> DaoInvoiceMethods for T {}

#[derive(sqlx::FromRow)]
struct CountRow {
    count: i64,
}

/// Push WHERE clause conditions to the query builder based on filter params.
/// Shared between `get_invoices_paginated` and `count_invoices`.
fn push_invoice_filters(
    builder: &mut QueryBuilder<'_, sqlx::Sqlite>,
    params: &ListInvoicesParams,
) {
    if let Some(statuses) = &params.status
        && !statuses.is_empty()
    {
        builder.push(" AND i.status IN (");
        let mut separated = builder.separated(", ");
        for status in statuses {
            separated.push_bind(status.to_string());
        }
        separated.push_unseparated(")");
    }

    if let Some(chain) = &params.chain {
        builder.push(" AND i.chain = ");
        builder.push_bind(chain.to_string());
    }

    if let Some(asset_id) = &params.asset_id {
        builder.push(" AND i.asset_id = ");
        builder.push_bind(asset_id.clone());
    }

    if let Some(created_from) = &params.created_from {
        builder.push(" AND i.created_at >= ");
        builder.push_bind(created_from.naive_utc());
    }

    if let Some(created_to) = &params.created_to {
        builder.push(" AND i.created_at <= ");
        builder.push_bind(created_to.naive_utc());
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::dao::create_test_dao;
    use crate::dao::transaction::DaoTransactionMethods;
    use crate::types::{
        ChainType,
        CreateInvoiceData,
        InvoiceCart,
        ListInvoicesParams,
        PaginationParams,
        SortOrder,
        Transaction,
        TransactionType,
        default_create_invoice_data,
        default_transaction,
        default_update_invoice_data,
    };

    use super::*;

    #[expect(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_get_active_invoices_with_amounts() {
        let dao = create_test_dao().await;

        // Create invoice 1 with Waiting status (will have 2 incoming transactions)
        let invoice1 = default_create_invoice_data();
        let invoice1_id = invoice1.id;

        dao.create_invoice(invoice1)
            .await
            .unwrap();

        // Create 2 incoming transactions for invoice1
        let mut tx1 = Transaction {
            id: Uuid::new_v4(),
            invoice_id: invoice1_id,
            transaction_type: TransactionType::Incoming,
            ..default_transaction(invoice1_id)
        };

        let tx1_amount = Decimal::new(10050, 2); // 100.50
        tx1.transfer_info.amount = tx1_amount;
        tx1.transaction_id.block_number = Some(100);
        tx1.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());

        dao.create_transaction(tx1)
            .await
            .unwrap();

        let mut tx2 = Transaction {
            id: Uuid::new_v4(),
            invoice_id: invoice1_id,
            transaction_type: TransactionType::Incoming,
            ..default_transaction(invoice1_id)
        };

        let tx2_amount = Decimal::new(5025, 2); // 50.25
        tx2.transfer_info.amount = tx2_amount;
        tx2.transaction_id.block_number = Some(300);
        tx2.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());

        dao.create_transaction(tx2)
            .await
            .unwrap();

        // Create invoice 2 with PartiallyPaid status (will have 1 incoming transaction)
        let invoice2 = default_create_invoice_data();
        let invoice2_id = invoice2.id;

        dao.create_invoice(invoice2)
            .await
            .unwrap();

        dao.update_invoice_status(
            invoice2_id,
            InvoiceStatus::PartiallyPaid,
        )
        .await
        .unwrap();

        let mut tx3 = Transaction {
            id: Uuid::new_v4(),
            invoice_id: invoice2_id,
            transaction_type: TransactionType::Incoming,
            ..default_transaction(invoice2_id)
        };

        let tx3_amount = Decimal::new(7599, 2); // 75.99
        tx3.transfer_info.amount = tx3_amount;
        tx3.transaction_id.block_number = Some(500);
        tx3.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());

        dao.create_transaction(tx3)
            .await
            .unwrap();

        // Create invoice 3 with Waiting status (no transactions)
        let invoice3 = default_create_invoice_data();
        let invoice3_id = invoice3.id;

        dao.create_invoice(invoice3)
            .await
            .unwrap();

        // Create invoice 4 with Waiting status and an Outgoing transaction (should not
        // be counted)
        let invoice4 = default_create_invoice_data();
        let invoice4_id = invoice4.id;

        dao.create_invoice(invoice4)
            .await
            .unwrap();

        let mut tx4_outgoing = Transaction {
            id: Uuid::new_v4(),
            invoice_id: invoice4_id,
            transaction_type: TransactionType::Outgoing,
            ..default_transaction(invoice4_id)
        };

        tx4_outgoing.transfer_info.amount = Decimal::new(10000, 2); // 100.00
        tx4_outgoing.transaction_id.block_number = Some(700);
        tx4_outgoing.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());

        dao.create_transaction(tx4_outgoing)
            .await
            .unwrap();

        // Create invoice 5 with Paid status (should not be in results)
        let invoice5 = default_create_invoice_data();
        let invoice5_id = invoice5.id;

        dao.create_invoice(invoice5)
            .await
            .unwrap();

        dao.update_invoice_status(invoice5_id, InvoiceStatus::Paid)
            .await
            .unwrap();

        let mut tx5 = Transaction {
            id: Uuid::new_v4(),
            invoice_id: invoice5_id,
            transaction_type: TransactionType::Incoming,
            ..default_transaction(invoice5_id)
        };

        tx5.transfer_info.amount = Decimal::new(10000, 2); // 100.00

        dao.create_transaction(tx5)
            .await
            .unwrap();

        // Execute the test
        let results = dao
            .get_active_invoices_with_amounts()
            .await
            .unwrap();

        // Should return 4 active invoices (Waiting and PartiallyPaid only)
        assert_eq!(results.len(), 4);

        // Find each invoice in results
        let invoice1_result = results
            .iter()
            .find(|r| r.invoice.id == invoice1_id)
            .expect("Invoice 1 should be in results");

        let invoice2_result = results
            .iter()
            .find(|r| r.invoice.id == invoice2_id)
            .expect("Invoice 2 should be in results");

        let invoice3_result = results
            .iter()
            .find(|r| r.invoice.id == invoice3_id)
            .expect("Invoice 3 should be in results");

        let invoice4_result = results
            .iter()
            .find(|r| r.invoice.id == invoice4_id)
            .expect("Invoice 4 should be in results");

        // Verify amounts are summed correctly with full precision
        let expected_invoice1_total = tx1_amount + tx2_amount; // 100.50 + 50.25 = 150.75
        assert_eq!(
            invoice1_result.total_received_amount, expected_invoice1_total,
            "Invoice 1 should have sum of 2 incoming transactions"
        );

        assert_eq!(
            invoice2_result.total_received_amount, tx3_amount,
            "Invoice 2 should have amount from single incoming transaction"
        );

        assert_eq!(
            invoice3_result.total_received_amount,
            Decimal::ZERO,
            "Invoice 3 should have zero incoming amount (no transactions)"
        );

        assert_eq!(
            invoice4_result.total_received_amount,
            Decimal::ZERO,
            "Invoice 4 should have zero incoming amount (only outgoing transaction)"
        );

        // Verify invoice 5 (Paid status) is NOT in results
        assert!(
            results
                .iter()
                .all(|r| r.invoice.id != invoice5_id),
            "Paid invoice should not be in active invoices results"
        );

        // Verify ordering (should be by created_at ASC)
        assert_eq!(
            results[0].invoice.id, invoice1_id,
            "First invoice should be invoice1"
        );
        assert_eq!(
            results[1].invoice.id, invoice2_id,
            "Second invoice should be invoice2"
        );
        assert_eq!(
            results[2].invoice.id, invoice3_id,
            "Third invoice should be invoice3"
        );
        assert_eq!(
            results[3].invoice.id, invoice4_id,
            "Fourth invoice should be invoice4"
        );
    }

    #[tokio::test]
    async fn test_invoice_crud_operations() {
        let dao = create_test_dao().await;

        // Create invoice
        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;
        let order_id = invoice.order_id.clone();

        let created = dao
            .create_invoice(invoice)
            .await
            .unwrap();

        // Verify created invoice fields
        assert_eq!(created.id, invoice_id);
        assert_eq!(created.order_id, order_id);
        assert_eq!(created.status, InvoiceStatus::Waiting);

        // Get by ID - should return Some
        let by_id = dao
            .get_invoice_by_id(invoice_id)
            .await
            .unwrap();
        assert!(by_id.is_some());
        let by_id = by_id.unwrap();
        assert_eq!(by_id.id, invoice_id);
        assert_eq!(by_id.order_id, order_id);

        // Get by non-existent ID - should return None
        let non_existent_id = dao
            .get_invoice_by_id(Uuid::new_v4())
            .await
            .unwrap();
        assert!(non_existent_id.is_none());
    }

    #[tokio::test]
    async fn test_create_invoice_duplicate_order_id_fails() {
        let dao = create_test_dao().await;

        // Create first invoice
        let invoice1 = default_create_invoice_data();
        let order_id = invoice1.order_id.clone();
        dao.create_invoice(invoice1)
            .await
            .unwrap();

        // Try to create second invoice with same order_id
        let invoice2 = CreateInvoiceData {
            order_id: order_id.clone(),
            ..default_create_invoice_data()
        };

        let result = dao.create_invoice(invoice2).await;

        // Should fail with DuplicateOrderId error
        assert!(result.is_err());
        match result.unwrap_err() {
            DaoInvoiceError::DuplicateOrderId {
                order_id: oid,
            } => {
                assert_eq!(oid, order_id);
            },
            err => panic!("Expected DuplicateOrderId error, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_update_invoice_status_and_triggers() {
        let dao = create_test_dao().await;

        // Create invoice with Waiting status
        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;
        let created = dao
            .create_invoice(invoice)
            .await
            .unwrap();

        assert_eq!(created.status, InvoiceStatus::Waiting);
        let original_updated_at = created.updated_at;

        // Sleep to ensure timestamp will change
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Update status to Paid
        let updated = dao
            .update_invoice_status(invoice_id, InvoiceStatus::Paid)
            .await
            .unwrap();

        // Verify status changed
        assert_eq!(updated.status, InvoiceStatus::Paid);

        // Verify trigger updated timestamp
        assert_ne!(updated.updated_at, original_updated_at);

        // Try to update non-existent invoice
        let result = dao
            .update_invoice_status(Uuid::new_v4(), InvoiceStatus::Paid)
            .await;

        // Should fail with NotFound
        assert!(result.is_err());
        match result.unwrap_err() {
            DaoInvoiceError::NotFound {
                ..
            } => { /* Expected */ },
            err => panic!("Expected NotFound, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_update_invoice_data_happy_path() {
        let dao = create_test_dao().await;

        // Create invoice (amount=100.00)
        let invoice = default_create_invoice_data();
        let invoice_id = invoice.id;
        let created = dao
            .create_invoice(invoice)
            .await
            .unwrap();

        assert_eq!(
            created.amount,
            rust_decimal::Decimal::new(10000, 2)
        );

        // Update amount to 150.00
        let update_data = default_update_invoice_data(invoice_id);
        let expected_cart = update_data.cart.clone();

        let updated = dao
            .update_invoice_data(update_data)
            .await
            .unwrap();

        // Verify amount updated
        assert_eq!(
            updated.amount,
            rust_decimal::Decimal::new(15000, 2)
        );

        // Verify cart and valid_till also updated
        assert_eq!(updated.cart, expected_cart);

        let mut update_data2 = default_update_invoice_data(invoice_id);
        update_data2.amount = rust_decimal::Decimal::new(20000, 2); // 200.00

        let updated2 = dao
            .update_invoice_data(update_data2)
            .await
            .unwrap();

        assert_eq!(
            updated2.amount,
            rust_decimal::Decimal::new(20000, 2)
        );
    }

    #[tokio::test]
    async fn test_update_invoice_data_optimistic_locking_failures() {
        let dao = create_test_dao().await;

        // Scenario A: Wrong status (not in Waiting state)
        let invoice = default_create_invoice_data();
        let id = invoice.id;

        dao.create_invoice(invoice)
            .await
            .unwrap();

        // Update status to Paid
        dao.update_invoice_status(id, InvoiceStatus::Paid)
            .await
            .unwrap();

        // Try update_invoice_data (requires status='Waiting')
        let update_data = default_update_invoice_data(id);

        let result = dao
            .update_invoice_data(update_data)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            DaoInvoiceError::UpdateNotAllowed {
                invoice_id,
                current_status,
            } => {
                assert_eq!(invoice_id, id);
                assert_eq!(current_status, InvoiceStatus::Paid);
            },
            err => panic!("Expected UpdateNotAllowed, got: {err:?}"),
        }

        // Scenario B: Non-existent invoice
        let update_data2 = default_update_invoice_data(Uuid::new_v4());
        let id2 = update_data2.invoice_id;

        let result2 = dao
            .update_invoice_data(update_data2)
            .await;

        // Should fail with NotFound
        assert!(result2.is_err());
        match result2.unwrap_err() {
            DaoInvoiceError::NotFound {
                invoice_id,
            } => {
                assert_eq!(invoice_id, id2);
            },
            err => panic!("Expected NotFound, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_invoice_status_transition_triggers() {
        let dao = create_test_dao().await;

        // Scenario 1: Invalid transition from Paid (final state) -> Waiting
        let invoice1 = default_create_invoice_data();
        let id1 = invoice1.id;

        dao.create_invoice(invoice1)
            .await
            .unwrap();

        dao.update_invoice_status(id1, InvoiceStatus::Paid)
            .await
            .inspect_err(|e| println!("Error updating to Paid: {:?}", e))
            .unwrap();

        let result = dao
            .update_invoice_status(id1, InvoiceStatus::Waiting)
            .await;

        match result.unwrap_err() {
            DaoInvoiceError::StatusConstraintViolation {
                current_status,
                attempted_status,
            } => {
                assert_eq!(current_status, InvoiceStatus::Paid);
                assert_eq!(attempted_status, InvoiceStatus::Waiting);
            },
            err => panic!("Expected StatusConstraintViolation, got: {err:?}"),
        }

        // Scenario 2: Valid transition from Waiting -> Paid
        let invoice2 = default_create_invoice_data();
        let id2 = invoice2.id;

        dao.create_invoice(invoice2)
            .await
            .unwrap();

        let updated = dao
            .update_invoice_status(id2, InvoiceStatus::Paid)
            .await
            .unwrap();
        assert_eq!(updated.status, InvoiceStatus::Paid);

        // Scenario 3: Invalid transition from PartiallyPaid -> Waiting
        let invoice3 = default_create_invoice_data();
        let id3 = invoice3.id;

        dao.create_invoice(invoice3)
            .await
            .unwrap();

        dao.update_invoice_status(id3, InvoiceStatus::PartiallyPaid)
            .await
            .unwrap();

        let result = dao
            .update_invoice_status(id3, InvoiceStatus::Waiting)
            .await;
        match result.unwrap_err() {
            DaoInvoiceError::StatusConstraintViolation {
                current_status,
                attempted_status,
            } => {
                assert_eq!(
                    current_status,
                    InvoiceStatus::PartiallyPaid
                );
                assert_eq!(attempted_status, InvoiceStatus::Waiting);
            },
            err => panic!("Expected StatusConstraintViolation, got: {err:?}"),
        }
    }

    // ========================================================================
    // Paginated invoice listing — snapshot tests
    // ========================================================================

    /// Helper to create an invoice with specific chain and amount.
    fn make_invoice(
        chain: ChainType,
        amount: Decimal,
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
            amount,
            payment_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
            cart: InvoiceCart::empty(),
            redirect_url: "http://localhost:8080/thankyou".to_string(),
            #[expect(clippy::arithmetic_side_effects)]
            valid_till: chrono::Utc::now() + chrono::Duration::hours(24),
        }
    }

    /// Add an incoming transaction for an invoice.
    async fn add_incoming_tx(
        dao: &crate::dao::DAO,
        invoice_id: Uuid,
        amount: Decimal,
        block_number: u32,
    ) {
        let mut tx = Transaction {
            id: Uuid::new_v4(),
            invoice_id,
            transaction_type: TransactionType::Incoming,
            ..default_transaction(invoice_id)
        };
        tx.transfer_info.amount = amount;
        tx.transaction_id.block_number = Some(block_number);
        tx.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());
        dao.create_transaction(tx)
            .await
            .unwrap();
    }

    /// Seed 8 invoices with diverse properties, return their IDs in insertion
    /// order. A small sleep separates the first 4 from the last 4 to allow
    /// date range filtering tests.
    ///
    /// | # | Chain       | Asset | Amount | Status              | Incoming Txs    |
    /// |---|-------------|-------|--------|---------------------|-----------------|
    /// | 1 | AssetHub    | USDT  | 100.00 | Waiting             | none            |
    /// | 2 | Polygon     | USDC  | 250.50 | Waiting             | none            |
    /// | 3 | AssetHub    | USDT  |  75.00 | Paid                | 75.00           |
    /// | 4 | Polygon     | USDC  | 500.00 | PartiallyPaid       | 150.00 + 50.00  |
    /// |   |             |       |        |                     | (sleep ~15ms)   |
    /// | 5 | AssetHub    | USDC  | 300.00 | OverPaid            | 300.00 + 25.00  |
    /// | 6 | Polygon     | USDT  |  42.00 | UnpaidExpired       | none            |
    /// | 7 | AssetHub    | USDT  | 180.00 | Waiting             | 60.00           |
    /// | 8 | Polygon     | USDC  |  99.99 | AdminCanceled       | none            |
    #[expect(clippy::too_many_lines)]
    async fn seed_invoices(dao: &crate::dao::DAO) -> Vec<Uuid> {
        let mut ids = Vec::new();

        // --- First batch (before the sleep) ---

        // Invoice 1: AssetHub, USDT, 100.00, Waiting, no txs
        let inv = make_invoice(
            ChainType::PolkadotAssetHub,
            Decimal::new(10000, 2),
            "1984",
        );
        ids.push(inv.id);
        dao.create_invoice(inv).await.unwrap();

        // Invoice 2: Polygon, USDC, 250.50, Waiting, no txs
        let inv = make_invoice(
            ChainType::Polygon,
            Decimal::new(25050, 2),
            "USDC",
        );
        ids.push(inv.id);
        dao.create_invoice(inv).await.unwrap();

        // Invoice 3: AssetHub, USDT, 75.00, Paid, 1 incoming tx (75.00)
        let inv = make_invoice(
            ChainType::PolkadotAssetHub,
            Decimal::new(7500, 2),
            "1984",
        );
        let inv_id = inv.id;
        ids.push(inv_id);
        dao.create_invoice(inv).await.unwrap();
        add_incoming_tx(dao, inv_id, Decimal::new(7500, 2), 100).await;
        dao.update_invoice_status(inv_id, InvoiceStatus::Paid)
            .await
            .unwrap();

        // Invoice 4: Polygon, USDC, 500.00, PartiallyPaid, 2 incoming txs
        // (150.00 + 50.00 = 200.00)
        let inv = make_invoice(
            ChainType::Polygon,
            Decimal::new(50000, 2),
            "USDC",
        );
        let inv_id = inv.id;
        ids.push(inv_id);
        dao.create_invoice(inv).await.unwrap();
        add_incoming_tx(dao, inv_id, Decimal::new(15000, 2), 200).await;
        add_incoming_tx(dao, inv_id, Decimal::new(5000, 2), 201).await;
        dao.update_invoice_status(inv_id, InvoiceStatus::PartiallyPaid)
            .await
            .unwrap();

        // Sleep to create a timestamp gap between batches
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;

        // --- Second batch (after the sleep) ---

        // Invoice 5: AssetHub, USDC, 300.00, OverPaid, 2 incoming txs
        // (300.00 + 25.00 = 325.00)
        let inv = make_invoice(
            ChainType::PolkadotAssetHub,
            Decimal::new(30000, 2),
            "USDC",
        );
        let inv_id = inv.id;
        ids.push(inv_id);
        dao.create_invoice(inv).await.unwrap();
        add_incoming_tx(dao, inv_id, Decimal::new(30000, 2), 300).await;
        add_incoming_tx(dao, inv_id, Decimal::new(2500, 2), 301).await;
        dao.update_invoice_status(inv_id, InvoiceStatus::OverPaid)
            .await
            .unwrap();

        // Invoice 6: Polygon, USDT, 42.00, UnpaidExpired, no txs
        let inv = make_invoice(
            ChainType::Polygon,
            Decimal::new(4200, 2),
            "1984",
        );
        let inv_id = inv.id;
        ids.push(inv_id);
        dao.create_invoice(inv).await.unwrap();
        dao.update_invoice_status(inv_id, InvoiceStatus::UnpaidExpired)
            .await
            .unwrap();

        // Invoice 7: AssetHub, USDT, 180.00, Waiting, 1 incoming tx (60.00)
        let inv = make_invoice(
            ChainType::PolkadotAssetHub,
            Decimal::new(18000, 2),
            "1984",
        );
        let inv_id = inv.id;
        ids.push(inv_id);
        dao.create_invoice(inv).await.unwrap();
        add_incoming_tx(dao, inv_id, Decimal::new(6000, 2), 400).await;

        // Invoice 8: Polygon, USDC, 99.99, AdminCanceled, no txs
        let inv = make_invoice(
            ChainType::Polygon,
            Decimal::new(9999, 2),
            "USDC",
        );
        let inv_id = inv.id;
        ids.push(inv_id);
        dao.create_invoice(inv).await.unwrap();
        dao.update_invoice_status(inv_id, InvoiceStatus::AdminCanceled)
            .await
            .unwrap();

        ids
    }

    #[tokio::test]
    async fn test_paginated_no_filters() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        let params = ListInvoicesParams::default();
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 8);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_filter_single_status() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // Waiting: inv1, inv2, inv7
        let params = ListInvoicesParams {
            status: Some(vec![InvoiceStatus::Waiting]),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 3);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_filter_multiple_statuses() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // Paid + PartiallyPaid: inv3, inv4
        let params = ListInvoicesParams {
            status: Some(vec![
                InvoiceStatus::Paid,
                InvoiceStatus::PartiallyPaid,
            ]),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 2);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_filter_by_chain() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // Polygon: inv2, inv4, inv6, inv8
        let params = ListInvoicesParams {
            chain: Some(ChainType::Polygon),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_filter_by_asset_id() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // USDC: inv2, inv4, inv5, inv8
        let params = ListInvoicesParams {
            asset_id: Some("USDC".to_string()),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_sort_asc() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        let params = ListInvoicesParams {
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();

        // ASC by created_at: inv1 (100.00) should be first
        assert_eq!(
            result[0].invoice.amount,
            Decimal::new(10000, 2)
        );
        // inv8 (99.99) should be last
        assert_eq!(
            result[7].invoice.amount,
            Decimal::new(9999, 2)
        );
        assert_eq!(result.len(), 8);
    }

    #[tokio::test]
    async fn test_paginated_sort_desc() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        let params = ListInvoicesParams {
            sort_order: Some(SortOrder::Desc),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();

        // DESC by created_at: inv8 (99.99) should be first
        assert_eq!(
            result[0].invoice.amount,
            Decimal::new(9999, 2)
        );
        // inv1 (100.00) should be last
        assert_eq!(
            result[7].invoice.amount,
            Decimal::new(10000, 2)
        );
        assert_eq!(result.len(), 8);
    }

    #[tokio::test]
    async fn test_paginated_limit_offset() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // Page 2 of 3 per page, ASC — should return invoices 4, 5, 6
        let params = ListInvoicesParams {
            pagination: PaginationParams {
                page: Some(2),
                per_page: Some(3),
            },
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 8);
        assert_eq!(result.len(), 3);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_beyond_last_page() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        let params = ListInvoicesParams {
            pagination: PaginationParams {
                page: Some(100),
                per_page: Some(20),
            },
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 8);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_paginated_empty_status_filter() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // CustomerCanceled — none seeded
        let params = ListInvoicesParams {
            status: Some(vec![InvoiceStatus::CustomerCanceled]),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 0);
        insta::assert_yaml_snapshot!(result);
    }

    #[tokio::test]
    async fn test_paginated_received_amounts() {
        let dao = create_test_dao().await;
        let ids = seed_invoices(&dao).await;

        // Fetch all in ASC order, check received amounts
        let params = ListInvoicesParams {
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();

        assert_eq!(result.len(), 8);

        // inv1: no txs → 0
        assert_eq!(result[0].invoice.id, ids[0]);
        assert_eq!(
            result[0].total_received_amount,
            Decimal::ZERO
        );

        // inv3: 75.00
        assert_eq!(result[2].invoice.id, ids[2]);
        assert_eq!(
            result[2].total_received_amount,
            Decimal::new(7500, 2)
        );

        // inv4: 150.00 + 50.00 = 200.00
        assert_eq!(result[3].invoice.id, ids[3]);
        assert_eq!(
            result[3].total_received_amount,
            Decimal::new(20000, 2)
        );

        // inv5: 300.00 + 25.00 = 325.00
        assert_eq!(result[4].invoice.id, ids[4]);
        assert_eq!(
            result[4].total_received_amount,
            Decimal::new(32500, 2)
        );

        // inv7: 60.00
        assert_eq!(result[6].invoice.id, ids[6]);
        assert_eq!(
            result[6].total_received_amount,
            Decimal::new(6000, 2)
        );
    }

    #[tokio::test]
    async fn test_paginated_filter_by_date_range() {
        let dao = create_test_dao().await;
        let _ids = seed_invoices(&dao).await;

        // Get all invoices ASC to find the timestamp boundary
        let all_params = ListInvoicesParams {
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let all = dao
            .get_invoices_paginated(&all_params)
            .await
            .unwrap();
        assert_eq!(all.len(), 8);

        // The sleep boundary is between inv4 (index 3) and inv5 (index 4).
        // Use inv4's created_at as `created_to` to get only the first batch.
        let boundary = all[3].invoice.created_at;

        let params = ListInvoicesParams {
            created_to: Some(boundary),
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        // Should return first 4 invoices only
        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
        insta::assert_yaml_snapshot!("date_range_before_boundary", result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });

        // Use inv5's created_at as `created_from` to get only the second batch.
        let after_boundary = all[4].invoice.created_at;

        let params = ListInvoicesParams {
            created_from: Some(after_boundary),
            sort_order: Some(SortOrder::Asc),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        // Should return last 4 invoices only
        assert_eq!(count, 4);
        assert_eq!(result.len(), 4);
        insta::assert_yaml_snapshot!("date_range_after_boundary", result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_combined_chain_and_status() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // Polygon + Waiting: only inv2
        let params = ListInvoicesParams {
            status: Some(vec![InvoiceStatus::Waiting]),
            chain: Some(ChainType::Polygon),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 1);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_combined_asset_and_chain() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // AssetHub + USDC: only inv5
        let params = ListInvoicesParams {
            chain: Some(ChainType::PolkadotAssetHub),
            asset_id: Some("USDC".to_string()),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 1);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }

    #[tokio::test]
    async fn test_paginated_all_terminal_statuses() {
        let dao = create_test_dao().await;
        seed_invoices(&dao).await;

        // All non-active statuses: Paid(inv3), OverPaid(inv5),
        // UnpaidExpired(inv6), AdminCanceled(inv8)
        let params = ListInvoicesParams {
            status: Some(vec![
                InvoiceStatus::Paid,
                InvoiceStatus::OverPaid,
                InvoiceStatus::UnpaidExpired,
                InvoiceStatus::AdminCanceled,
            ]),
            ..Default::default()
        };
        let result = dao
            .get_invoices_paginated(&params)
            .await
            .unwrap();
        let count = dao
            .count_invoices(&params)
            .await
            .unwrap();

        assert_eq!(count, 4);
        insta::assert_yaml_snapshot!(result, {
            "[].invoice.id" => "[uuid]",
            "[].invoice.order_id" => "[uuid]",
            "[].invoice.created_at" => "[timestamp]",
            "[].invoice.updated_at" => "[timestamp]",
            "[].invoice.valid_till" => "[timestamp]",
        });
    }
}
