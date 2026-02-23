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
}

impl<T: DaoExecutor + 'static> DaoInvoiceMethods for T {}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::dao::create_test_dao;
    use crate::dao::transaction::DaoTransactionMethods;
    use crate::types::{
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
}
