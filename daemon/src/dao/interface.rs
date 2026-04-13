//! High-level DAO interface traits for easy mocking and dependency injection.
//!
//! This module provides two main traits:
//! - `DaoInterface`: For regular DAO operations
//! - `DaoTransactionInterface`: For transactional operations
//!
//! Both traits can be easily mocked using mockall's `#[automock]` attribute.

use chrono::{
    DateTime,
    Utc,
};
use uuid::Uuid;

use crate::types::{
    ChangesResponse,
    CreateFrontEndSwapParams,
    CreateInvoiceData,
    FrontEndSwap,
    GeneralTransactionId,
    Invoice,
    InvoiceStatus,
    InvoiceWithReceivedAmount,
    ListInvoicesParams,
    ListPayoutsParams,
    ListSwapsParams,
    ListTransactionsParams,
    Payout,
    PayoutStatus,
    Refund,
    RefundStatus,
    RetryMeta,
    Swap,
    Transaction,
    TransferDestinationParams,
    UpdateInvoiceData,
    WebhookEvent,
};

use super::changes::{
    DaoChangesError,
    DaoChangesMethods,
};
use super::invoice::{
    DaoInvoiceError,
    DaoInvoiceMethods,
};
use super::payout::{
    DaoPayoutError,
    DaoPayoutMethods,
};
use super::refund::{
    DaoRefundError,
    DaoRefundMethods,
};
use super::swap::{
    DaoSwapError,
    DaoSwapMethods,
};
use super::transaction::{
    DaoTransactionError,
    DaoTransactionMethods,
};
use super::webhook_event::{
    DaoWebhookEventError,
    DaoWebhookEventMethods,
};

use super::{
    DAO,
    DaoResult,
    DaoTransaction,
};

/// High-level interface for database operations.
///
/// This trait defines the public API for the DAO and can be easily mocked for
/// testing. All methods delegate to the existing trait implementations.
///
/// # Example
///
/// ```rust
/// use kalatori::dao::{DaoInterface, MockDaoInterface};
///
/// #[tokio::test]
/// async fn test_with_mock() {
///     let mut mock = MockDaoInterface::new();
///
///     mock.expect_create_invoice()
///         .returning(|inv| Ok(inv));
///
///     // Use mock in your test
/// }
/// ```
#[expect(dead_code)]
#[cfg_attr(test, mockall::automock(type Transaction = MockDaoTransactionInterface;))]
#[trait_variant::make(Send)]
pub trait DaoInterface: Send + Sync + 'static {
    type Transaction: DaoTransactionInterface + Sync + Send;

    async fn begin_transaction(&self) -> DaoResult<Self::Transaction>;

    // === Invoice Methods ===

    /// Create a new invoice in the database.
    async fn create_invoice(
        &self,
        invoice: CreateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError>;

    /// Get all existing invoices.
    async fn get_all_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError>;

    /// Get an invoice by its unique ID.
    async fn get_invoice_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<Invoice>, DaoInvoiceError>;

    /// Get an invoice with sum of related incoming transactions by its unique
    /// ID.
    async fn get_invoice_with_received_amount_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<InvoiceWithReceivedAmount>, DaoInvoiceError>;

    /// Get all active invoices (Waiting or `PartiallyPaid` status) along with
    /// their incoming amounts (sum amounts of related Incoming transaction).
    async fn get_active_invoices_with_amounts(
        &self
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError>;

    /// Update an invoice's status.
    async fn update_invoice_status(
        &self,
        invoice_id: Uuid,
        status: InvoiceStatus,
    ) -> Result<Invoice, DaoInvoiceError>;

    /// Update invoice data (amount, cart, `valid_till`).
    async fn update_invoice_data(
        &self,
        data: UpdateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError>;

    async fn get_expired_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError>;

    /// Get a paginated, filtered list of invoices with their received amounts.
    async fn get_invoices_paginated(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError>;

    /// Count invoices matching the given filters.
    async fn count_invoices(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<u32, DaoInvoiceError>;

    // === Transaction Methods ===

    /// Create a new transaction record.
    async fn create_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<Transaction, DaoTransactionError>;

    /// Get all transactions.
    async fn get_all_transactions(&self) -> Result<Vec<Transaction>, DaoTransactionError>;

    /// Mark a transaction as successful with blockchain coordinates.
    async fn update_transaction_successful(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        confirmed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError>;

    /// Mark a transaction as failed with error details.
    async fn update_transaction_failed(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        failure_message: String,
        failed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError>;

    /// Get a transaction by its ID.
    async fn get_transaction_by_id(
        &self,
        transaction_id: Uuid,
    ) -> Result<Option<Transaction>, DaoTransactionError>;

    /// Get all transactions for a specific invoice.
    async fn get_invoice_transactions(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError>;

    async fn get_completed_transactions_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError>;

    /// Get a paginated, filtered list of transactions.
    async fn get_transactions_paginated(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<Vec<Transaction>, DaoTransactionError>;

    /// Count transactions matching the given filters.
    async fn count_transactions(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<u32, DaoTransactionError>;

    // === Payout Methods ===

    /// Create a new payout record.
    async fn create_payout(
        &self,
        payout: Payout,
    ) -> Result<Payout, DaoPayoutError>;

    /// Get all payouts.
    async fn get_all_payouts(&self) -> Result<Vec<Payout>, DaoPayoutError>;

    /// Get a payout by its ID.
    async fn get_payout_by_id(
        &self,
        payout_id: Uuid,
    ) -> Result<Option<Payout>, DaoPayoutError>;

    /// Get all pending payouts (Waiting status) and mark them as `InProgress`.
    /// Returns up to `limit` payouts.
    async fn get_pending_payouts(
        &self,
        limit: u32,
    ) -> Result<Vec<Payout>, DaoPayoutError>;

    /// Update a payout's status.
    async fn update_payout_status(
        &self,
        payout_id: Uuid,
        new_status: PayoutStatus,
    ) -> Result<Payout, DaoPayoutError>;

    /// Update payout retry metadata and status.
    async fn update_payout_retry(
        &self,
        payout_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Payout, DaoPayoutError>;

    /// Get a paginated, filtered list of payouts.
    async fn get_payouts_paginated(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<Vec<Payout>, DaoPayoutError>;

    /// Count payouts matching the given filters.
    async fn count_payouts(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<u32, DaoPayoutError>;

    // === Webhook Event Methods ===

    async fn create_webhook_event(
        &self,
        event: WebhookEvent,
    ) -> Result<WebhookEvent, DaoWebhookEventError>;

    async fn get_webhook_events_to_send(
        &self,
        limit: u32,
    ) -> Result<Vec<WebhookEvent>, DaoWebhookEventError>;

    async fn mark_webhook_event_as_sent(
        &self,
        event_id: Uuid,
    ) -> Result<WebhookEvent, DaoWebhookEventError>;

    // === Changes Methods ===

    /// Get all invoices and related entities modified since the given
    /// timestamp.
    async fn get_invoice_changes(
        &self,
        since: DateTime<Utc>,
    ) -> Result<ChangesResponse, DaoChangesError>;

    // === Swap Methods ===

    async fn create_front_end_swap(
        &self,
        swap: CreateFrontEndSwapParams,
    ) -> Result<FrontEndSwap, DaoSwapError>;

    async fn get_all_front_end_swaps(&self) -> Result<Vec<FrontEndSwap>, DaoSwapError>;

    async fn create_swap(
        &self,
        swap: Swap,
    ) -> Result<Swap, DaoSwapError>;

    async fn get_submitted_swaps(&self) -> Result<Vec<Swap>, DaoSwapError>;

    async fn get_pending_swaps(&self) -> Result<Vec<Swap>, DaoSwapError>;

    async fn update_swap_set_signature(
        &self,
        swap_id: Uuid,
        signature: String,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_submitted(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_submitted_with_hash(
        &self,
        swap_id: Uuid,
        transaction_hash: String,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_completed(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_failed(
        &self,
        swap_id: Uuid,
        error_message: String,
    ) -> Result<Swap, DaoSwapError>;

    /// Get a swap by its ID.
    async fn get_swap_by_id(
        &self,
        swap_id: Uuid,
    ) -> Result<Option<Swap>, DaoSwapError>;

    /// Get a paginated, filtered list of swaps.
    async fn get_swaps_paginated(
        &self,
        params: &ListSwapsParams,
    ) -> Result<Vec<Swap>, DaoSwapError>;

    async fn get_completed_incoming_swaps_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Swap>, DaoSwapError>;

    /// Count swaps matching the given filters.
    async fn count_swaps(
        &self,
        params: &ListSwapsParams,
    ) -> Result<u32, DaoSwapError>;

    // === Refund Methods ===

    async fn create_refund(
        &self,
        refund: Refund,
    ) -> Result<Refund, DaoRefundError>;

    async fn get_refund_by_id(
        &self,
        refund_id: Uuid,
    ) -> Result<Option<Refund>, DaoRefundError>;

    async fn get_all_refunds(&self) -> Result<Vec<Refund>, DaoRefundError>;

    async fn get_pending_refunds(
        &self,
        limit: u32,
    ) -> Result<Vec<Refund>, DaoRefundError>;

    async fn update_refund_status(
        &self,
        refund_id: Uuid,
        status: RefundStatus,
    ) -> Result<Refund, DaoRefundError>;

    async fn update_refund_retry(
        &self,
        refund_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Refund, DaoRefundError>;

    async fn update_refund_destination_params(
        &self,
        refund_id: Uuid,
        destination_params: TransferDestinationParams,
    ) -> Result<Refund, DaoRefundError>;
}

/// Interface for database transaction operations.
///
/// Provides the same high-level methods as `DaoInterface` but within a
/// transaction context. Must be committed or rolled back explicitly.
///
/// # Example
///
/// ```rust
/// let tx = dao.begin_transaction().await?;
/// tx.create_invoice(invoice).await?;
/// tx.create_transaction(transaction).await?;
/// tx.commit().await?;
/// ```
#[expect(dead_code)]
#[cfg_attr(test, mockall::automock)]
#[trait_variant::make(Send)]
pub trait DaoTransactionInterface {
    // === Invoice Methods ===

    async fn create_invoice(
        &self,
        invoice: CreateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError>;

    async fn get_all_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError>;

    async fn get_invoice_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<Invoice>, DaoInvoiceError>;

    async fn get_invoice_with_received_amount_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<InvoiceWithReceivedAmount>, DaoInvoiceError>;

    async fn update_invoice_status(
        &self,
        invoice_id: Uuid,
        status: InvoiceStatus,
    ) -> Result<Invoice, DaoInvoiceError>;

    async fn update_invoice_data(
        &self,
        data: UpdateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError>;

    async fn get_expired_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError>;

    async fn get_invoices_paginated(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError>;

    async fn count_invoices(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<u32, DaoInvoiceError>;

    // === Transaction Methods ===

    async fn create_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<Transaction, DaoTransactionError>;

    async fn get_all_transactions(&self) -> Result<Vec<Transaction>, DaoTransactionError>;

    async fn update_transaction_successful(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        confirmed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError>;

    async fn update_transaction_failed(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        failure_message: String,
        failed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError>;

    async fn get_transaction_by_id(
        &self,
        transaction_id: Uuid,
    ) -> Result<Option<Transaction>, DaoTransactionError>;

    async fn get_invoice_transactions(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError>;

    async fn get_completed_transactions_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError>;

    async fn get_transactions_paginated(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<Vec<Transaction>, DaoTransactionError>;

    async fn count_transactions(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<u32, DaoTransactionError>;

    // === Payout Methods ===

    async fn create_payout(
        &self,
        payout: Payout,
    ) -> Result<Payout, DaoPayoutError>;

    async fn get_all_payouts(&self) -> Result<Vec<Payout>, DaoPayoutError>;

    async fn get_payout_by_id(
        &self,
        payout_id: Uuid,
    ) -> Result<Option<Payout>, DaoPayoutError>;

    async fn get_pending_payouts(
        &self,
        limit: u32,
    ) -> Result<Vec<Payout>, DaoPayoutError>;

    async fn update_payout_status(
        &self,
        payout_id: Uuid,
        new_status: PayoutStatus,
    ) -> Result<Payout, DaoPayoutError>;

    async fn update_payout_retry(
        &self,
        payout_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Payout, DaoPayoutError>;

    async fn get_payouts_paginated(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<Vec<Payout>, DaoPayoutError>;

    async fn count_payouts(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<u32, DaoPayoutError>;

    // === Webhook Event Methods ===

    async fn create_webhook_event(
        &self,
        event: WebhookEvent,
    ) -> Result<WebhookEvent, DaoWebhookEventError>;

    async fn get_webhook_events_to_send(
        &self,
        limit: u32,
    ) -> Result<Vec<WebhookEvent>, DaoWebhookEventError>;

    async fn mark_webhook_event_as_sent(
        &self,
        event_id: Uuid,
    ) -> Result<WebhookEvent, DaoWebhookEventError>;

    // === Swap Methods ===

    async fn create_front_end_swap(
        &self,
        swap: CreateFrontEndSwapParams,
    ) -> Result<FrontEndSwap, DaoSwapError>;

    async fn get_all_front_end_swaps(&self) -> Result<Vec<FrontEndSwap>, DaoSwapError>;

    async fn create_swap(
        &self,
        swap: Swap,
    ) -> Result<Swap, DaoSwapError>;

    async fn get_submitted_swaps(&self) -> Result<Vec<Swap>, DaoSwapError>;

    async fn get_pending_swaps(&self) -> Result<Vec<Swap>, DaoSwapError>;

    async fn update_swap_set_signature(
        &self,
        swap_id: Uuid,
        signature: String,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_submitted(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_across_swap_submitted(
        &self,
        swap_id: Uuid,
        transaction_hash: String,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_completed(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError>;

    async fn update_swap_failed(
        &self,
        swap_id: Uuid,
        error_message: String,
    ) -> Result<Swap, DaoSwapError>;

    async fn get_swap_by_id(
        &self,
        swap_id: Uuid,
    ) -> Result<Option<Swap>, DaoSwapError>;

    async fn get_completed_incoming_swaps_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Swap>, DaoSwapError>;

    async fn get_swaps_paginated(
        &self,
        params: &ListSwapsParams,
    ) -> Result<Vec<Swap>, DaoSwapError>;

    async fn count_swaps(
        &self,
        params: &ListSwapsParams,
    ) -> Result<u32, DaoSwapError>;

    // === Refund Methods ===
    async fn create_refund(
        &self,
        refund: Refund,
    ) -> Result<Refund, DaoRefundError>;

    async fn get_refund_by_id(
        &self,
        refund_id: Uuid,
    ) -> Result<Option<Refund>, DaoRefundError>;

    async fn get_all_refunds(&self) -> Result<Vec<Refund>, DaoRefundError>;

    async fn get_pending_refunds(
        &self,
        limit: u32,
    ) -> Result<Vec<Refund>, DaoRefundError>;

    async fn update_refund_status(
        &self,
        refund_id: Uuid,
        status: RefundStatus,
    ) -> Result<Refund, DaoRefundError>;

    async fn update_refund_retry(
        &self,
        refund_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Refund, DaoRefundError>;

    async fn update_refund_destination_params(
        &self,
        refund_id: Uuid,
        destination_params: TransferDestinationParams,
    ) -> Result<Refund, DaoRefundError>;

    // === Transaction Control ===

    /// Commit the transaction, persisting all changes.
    async fn commit(self) -> DaoResult<()>;

    /// Rollback the transaction, discarding all changes.
    async fn rollback(self) -> DaoResult<()>;
}

// ============================================================================
// Implementation for DAO (delegates to existing trait methods)
// ============================================================================

impl DaoInterface for DAO {
    type Transaction = DaoTransaction;

    async fn begin_transaction(&self) -> DaoResult<Self::Transaction> {
        DAO::begin_transaction(self).await
    }

    async fn create_invoice(
        &self,
        invoice: CreateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError> {
        DaoInvoiceMethods::create_invoice(self, invoice).await
    }

    async fn get_all_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError> {
        DaoInvoiceMethods::get_all_invoices(self).await
    }

    async fn get_invoice_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<Invoice>, DaoInvoiceError> {
        DaoInvoiceMethods::get_invoice_by_id(self, invoice_id).await
    }

    async fn get_invoice_with_received_amount_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        DaoInvoiceMethods::get_invoice_with_received_amount_by_id(self, invoice_id).await
    }

    async fn get_active_invoices_with_amounts(
        &self
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        DaoInvoiceMethods::get_active_invoices_with_amounts(self).await
    }

    async fn update_invoice_status(
        &self,
        invoice_id: Uuid,
        status: InvoiceStatus,
    ) -> Result<Invoice, DaoInvoiceError> {
        DaoInvoiceMethods::update_invoice_status(self, invoice_id, status).await
    }

    async fn update_invoice_data(
        &self,
        data: UpdateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError> {
        DaoInvoiceMethods::update_invoice_data(self, data).await
    }

    async fn get_expired_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError> {
        DaoInvoiceMethods::get_expired_invoices(self).await
    }

    async fn get_invoices_paginated(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        DaoInvoiceMethods::get_invoices_paginated(self, params).await
    }

    async fn count_invoices(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<u32, DaoInvoiceError> {
        DaoInvoiceMethods::count_invoices(self, params).await
    }

    async fn create_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<Transaction, DaoTransactionError> {
        DaoTransactionMethods::create_transaction(self, transaction).await
    }

    async fn get_all_transactions(&self) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_all_completed_transactions(self).await
    }

    async fn update_transaction_successful(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        confirmed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError> {
        DaoTransactionMethods::update_transaction_successful(
            self,
            transaction_id,
            chain_transaction_id,
            confirmed_at,
        )
        .await
    }

    async fn update_transaction_failed(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        failure_message: String,
        failed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError> {
        DaoTransactionMethods::update_transaction_failed(
            self,
            transaction_id,
            chain_transaction_id,
            failure_message,
            failed_at,
        )
        .await
    }

    async fn get_transaction_by_id(
        &self,
        transaction_id: Uuid,
    ) -> Result<Option<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_transaction_by_id(self, transaction_id).await
    }

    async fn get_invoice_transactions(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_invoice_transactions(self, invoice_id).await
    }

    async fn get_completed_transactions_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_completed_transactions_by_invoice(self, invoice_id).await
    }

    async fn get_transactions_paginated(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_transactions_paginated(self, params).await
    }

    async fn count_transactions(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<u32, DaoTransactionError> {
        DaoTransactionMethods::count_transactions(self, params).await
    }

    async fn create_payout(
        &self,
        payout: Payout,
    ) -> Result<Payout, DaoPayoutError> {
        DaoPayoutMethods::create_payout(self, payout).await
    }

    async fn get_all_payouts(&self) -> Result<Vec<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_all_payouts(self).await
    }

    async fn get_payout_by_id(
        &self,
        payout_id: Uuid,
    ) -> Result<Option<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_payout_by_id(self, payout_id).await
    }

    async fn get_pending_payouts(
        &self,
        limit: u32,
    ) -> Result<Vec<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_pending_payouts(self, limit).await
    }

    async fn update_payout_status(
        &self,
        payout_id: Uuid,
        new_status: PayoutStatus,
    ) -> Result<Payout, DaoPayoutError> {
        DaoPayoutMethods::update_payout_status(self, payout_id, new_status).await
    }

    async fn update_payout_retry(
        &self,
        payout_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Payout, DaoPayoutError> {
        DaoPayoutMethods::update_payout_retry(
            self,
            payout_id,
            retry_meta,
            is_retriable,
        )
        .await
    }

    async fn get_payouts_paginated(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<Vec<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_payouts_paginated(self, params).await
    }

    async fn count_payouts(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<u32, DaoPayoutError> {
        DaoPayoutMethods::count_payouts(self, params).await
    }

    async fn create_webhook_event(
        &self,
        event: WebhookEvent,
    ) -> Result<WebhookEvent, DaoWebhookEventError> {
        DaoWebhookEventMethods::create_webhook_event(self, event).await
    }

    async fn get_webhook_events_to_send(
        &self,
        limit: u32,
    ) -> Result<Vec<WebhookEvent>, DaoWebhookEventError> {
        DaoWebhookEventMethods::get_webhook_events_to_send(self, limit).await
    }

    async fn mark_webhook_event_as_sent(
        &self,
        event_id: Uuid,
    ) -> Result<WebhookEvent, DaoWebhookEventError> {
        DaoWebhookEventMethods::mark_webhook_event_as_sent(self, event_id).await
    }

    async fn get_invoice_changes(
        &self,
        since: DateTime<Utc>,
    ) -> Result<ChangesResponse, DaoChangesError> {
        DaoChangesMethods::get_invoice_changes(self, since).await
    }

    async fn create_front_end_swap(
        &self,
        swap: CreateFrontEndSwapParams,
    ) -> Result<FrontEndSwap, DaoSwapError> {
        DaoSwapMethods::create_front_end_swap(self, swap).await
    }

    async fn get_all_front_end_swaps(&self) -> Result<Vec<FrontEndSwap>, DaoSwapError> {
        DaoSwapMethods::get_all_front_end_swaps(self).await
    }

    async fn create_swap(
        &self,
        swap: Swap,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::create_swap(self, swap).await
    }

    async fn get_submitted_swaps(&self) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_submitted_swaps(self).await
    }

    async fn get_pending_swaps(&self) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_pending_swaps(self).await
    }

    async fn update_swap_set_signature(
        &self,
        swap_id: Uuid,
        signature: String,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_set_signature(self, swap_id, signature).await
    }

    async fn update_swap_submitted(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_submitted(self, swap_id).await
    }

    async fn update_swap_submitted_with_hash(
        &self,
        swap_id: Uuid,
        transaction_hash: String,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_submitted_with_hash(self, swap_id, transaction_hash).await
    }

    async fn update_swap_completed(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_completed(self, swap_id).await
    }

    async fn update_swap_failed(
        &self,
        swap_id: Uuid,
        error_message: String,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_failed(self, swap_id, error_message).await
    }

    async fn get_swap_by_id(
        &self,
        swap_id: Uuid,
    ) -> Result<Option<Swap>, DaoSwapError> {
        DaoSwapMethods::get_swap_by_id(self, swap_id).await
    }

    async fn get_completed_incoming_swaps_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_completed_incoming_swaps_by_invoice(self, invoice_id).await
    }

    async fn get_swaps_paginated(
        &self,
        params: &ListSwapsParams,
    ) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_swaps_paginated(self, params).await
    }

    async fn count_swaps(
        &self,
        params: &ListSwapsParams,
    ) -> Result<u32, DaoSwapError> {
        DaoSwapMethods::count_swaps(self, params).await
    }

    async fn create_refund(
        &self,
        refund: Refund,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::create_refund(self, refund).await
    }

    async fn get_refund_by_id(
        &self,
        refund_id: Uuid,
    ) -> Result<Option<Refund>, DaoRefundError> {
        DaoRefundMethods::get_refund_by_id(self, refund_id).await
    }

    async fn get_all_refunds(&self) -> Result<Vec<Refund>, DaoRefundError> {
        DaoRefundMethods::get_all_refunds(self).await
    }

    async fn get_pending_refunds(
        &self,
        limit: u32,
    ) -> Result<Vec<Refund>, DaoRefundError> {
        DaoRefundMethods::get_pending_refunds(self, limit).await
    }

    async fn update_refund_status(
        &self,
        refund_id: Uuid,
        status: RefundStatus,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::update_refund_status(self, refund_id, status).await
    }

    async fn update_refund_retry(
        &self,
        refund_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::update_refund_retry(
            self,
            refund_id,
            retry_meta,
            is_retriable,
        )
        .await
    }

    async fn update_refund_destination_params(
        &self,
        refund_id: Uuid,
        destination_params: TransferDestinationParams,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::update_refund_destination_params(self, refund_id, destination_params)
            .await
    }
}

// ============================================================================
// Implementation for DaoTransaction (delegates to existing trait methods)
// ============================================================================

impl DaoTransactionInterface for DaoTransaction {
    async fn create_invoice(
        &self,
        invoice: CreateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError> {
        DaoInvoiceMethods::create_invoice(self, invoice).await
    }

    async fn get_all_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError> {
        DaoInvoiceMethods::get_all_invoices(self).await
    }

    async fn get_invoice_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<Invoice>, DaoInvoiceError> {
        DaoInvoiceMethods::get_invoice_by_id(self, invoice_id).await
    }

    async fn get_invoice_with_received_amount_by_id(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        DaoInvoiceMethods::get_invoice_with_received_amount_by_id(self, invoice_id).await
    }

    async fn update_invoice_status(
        &self,
        invoice_id: Uuid,
        status: InvoiceStatus,
    ) -> Result<Invoice, DaoInvoiceError> {
        DaoInvoiceMethods::update_invoice_status(self, invoice_id, status).await
    }

    async fn update_invoice_data(
        &self,
        data: UpdateInvoiceData,
    ) -> Result<Invoice, DaoInvoiceError> {
        DaoInvoiceMethods::update_invoice_data(self, data).await
    }

    async fn get_expired_invoices(&self) -> Result<Vec<Invoice>, DaoInvoiceError> {
        DaoInvoiceMethods::get_expired_invoices(self).await
    }

    async fn get_invoices_paginated(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<Vec<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        DaoInvoiceMethods::get_invoices_paginated(self, params).await
    }

    async fn count_invoices(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<u32, DaoInvoiceError> {
        DaoInvoiceMethods::count_invoices(self, params).await
    }

    async fn create_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<Transaction, DaoTransactionError> {
        DaoTransactionMethods::create_transaction(self, transaction).await
    }

    async fn get_all_transactions(&self) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_all_completed_transactions(self).await
    }

    async fn update_transaction_successful(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        confirmed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError> {
        DaoTransactionMethods::update_transaction_successful(
            self,
            transaction_id,
            chain_transaction_id,
            confirmed_at,
        )
        .await
    }

    async fn update_transaction_failed(
        &self,
        transaction_id: Uuid,
        chain_transaction_id: GeneralTransactionId,
        failure_message: String,
        failed_at: DateTime<Utc>,
    ) -> Result<Transaction, DaoTransactionError> {
        DaoTransactionMethods::update_transaction_failed(
            self,
            transaction_id,
            chain_transaction_id,
            failure_message,
            failed_at,
        )
        .await
    }

    async fn get_transaction_by_id(
        &self,
        transaction_id: Uuid,
    ) -> Result<Option<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_transaction_by_id(self, transaction_id).await
    }

    async fn get_invoice_transactions(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_invoice_transactions(self, invoice_id).await
    }

    async fn get_completed_transactions_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_completed_transactions_by_invoice(self, invoice_id).await
    }

    async fn get_transactions_paginated(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        DaoTransactionMethods::get_transactions_paginated(self, params).await
    }

    async fn count_transactions(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<u32, DaoTransactionError> {
        DaoTransactionMethods::count_transactions(self, params).await
    }

    async fn create_payout(
        &self,
        payout: Payout,
    ) -> Result<Payout, DaoPayoutError> {
        DaoPayoutMethods::create_payout(self, payout).await
    }

    async fn get_all_payouts(&self) -> Result<Vec<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_all_payouts(self).await
    }

    async fn get_payout_by_id(
        &self,
        payout_id: Uuid,
    ) -> Result<Option<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_payout_by_id(self, payout_id).await
    }

    async fn get_pending_payouts(
        &self,
        limit: u32,
    ) -> Result<Vec<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_pending_payouts(self, limit).await
    }

    async fn update_payout_status(
        &self,
        payout_id: Uuid,
        new_status: PayoutStatus,
    ) -> Result<Payout, DaoPayoutError> {
        DaoPayoutMethods::update_payout_status(self, payout_id, new_status).await
    }

    async fn update_payout_retry(
        &self,
        payout_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Payout, DaoPayoutError> {
        DaoPayoutMethods::update_payout_retry(
            self,
            payout_id,
            retry_meta,
            is_retriable,
        )
        .await
    }

    async fn get_payouts_paginated(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<Vec<Payout>, DaoPayoutError> {
        DaoPayoutMethods::get_payouts_paginated(self, params).await
    }

    async fn count_payouts(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<u32, DaoPayoutError> {
        DaoPayoutMethods::count_payouts(self, params).await
    }

    async fn create_webhook_event(
        &self,
        event: WebhookEvent,
    ) -> Result<WebhookEvent, DaoWebhookEventError> {
        DaoWebhookEventMethods::create_webhook_event(self, event).await
    }

    async fn get_webhook_events_to_send(
        &self,
        limit: u32,
    ) -> Result<Vec<WebhookEvent>, DaoWebhookEventError> {
        DaoWebhookEventMethods::get_webhook_events_to_send(self, limit).await
    }

    async fn mark_webhook_event_as_sent(
        &self,
        event_id: Uuid,
    ) -> Result<WebhookEvent, DaoWebhookEventError> {
        DaoWebhookEventMethods::mark_webhook_event_as_sent(self, event_id).await
    }

    async fn create_front_end_swap(
        &self,
        swap: CreateFrontEndSwapParams,
    ) -> Result<FrontEndSwap, DaoSwapError> {
        DaoSwapMethods::create_front_end_swap(self, swap).await
    }

    async fn get_all_front_end_swaps(&self) -> Result<Vec<FrontEndSwap>, DaoSwapError> {
        DaoSwapMethods::get_all_front_end_swaps(self).await
    }

    async fn create_swap(
        &self,
        swap: Swap,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::create_swap(self, swap).await
    }

    async fn get_submitted_swaps(&self) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_submitted_swaps(self).await
    }

    async fn get_pending_swaps(&self) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_pending_swaps(self).await
    }

    async fn update_swap_set_signature(
        &self,
        swap_id: Uuid,
        signature: String,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_set_signature(self, swap_id, signature).await
    }

    async fn update_swap_submitted(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_submitted(self, swap_id).await
    }

    async fn update_across_swap_submitted(
        &self,
        swap_id: Uuid,
        transaction_hash: String,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_submitted_with_hash(self, swap_id, transaction_hash).await
    }

    async fn update_swap_completed(
        &self,
        swap_id: Uuid,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_completed(self, swap_id).await
    }

    async fn update_swap_failed(
        &self,
        swap_id: Uuid,
        error_message: String,
    ) -> Result<Swap, DaoSwapError> {
        DaoSwapMethods::update_swap_failed(self, swap_id, error_message).await
    }

    async fn get_swap_by_id(
        &self,
        swap_id: Uuid,
    ) -> Result<Option<Swap>, DaoSwapError> {
        DaoSwapMethods::get_swap_by_id(self, swap_id).await
    }

    async fn get_completed_incoming_swaps_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_completed_incoming_swaps_by_invoice(self, invoice_id).await
    }

    async fn get_swaps_paginated(
        &self,
        params: &ListSwapsParams,
    ) -> Result<Vec<Swap>, DaoSwapError> {
        DaoSwapMethods::get_swaps_paginated(self, params).await
    }

    async fn count_swaps(
        &self,
        params: &ListSwapsParams,
    ) -> Result<u32, DaoSwapError> {
        DaoSwapMethods::count_swaps(self, params).await
    }

    async fn create_refund(
        &self,
        refund: Refund,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::create_refund(self, refund).await
    }

    async fn get_refund_by_id(
        &self,
        refund_id: Uuid,
    ) -> Result<Option<Refund>, DaoRefundError> {
        DaoRefundMethods::get_refund_by_id(self, refund_id).await
    }

    async fn get_all_refunds(&self) -> Result<Vec<Refund>, DaoRefundError> {
        DaoRefundMethods::get_all_refunds(self).await
    }

    async fn get_pending_refunds(
        &self,
        limit: u32,
    ) -> Result<Vec<Refund>, DaoRefundError> {
        DaoRefundMethods::get_pending_refunds(self, limit).await
    }

    async fn update_refund_status(
        &self,
        refund_id: Uuid,
        status: RefundStatus,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::update_refund_status(self, refund_id, status).await
    }

    async fn update_refund_retry(
        &self,
        refund_id: Uuid,
        retry_meta: RetryMeta,
        is_retriable: bool,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::update_refund_retry(
            self,
            refund_id,
            retry_meta,
            is_retriable,
        )
        .await
    }

    async fn update_refund_destination_params(
        &self,
        refund_id: Uuid,
        destination_params: TransferDestinationParams,
    ) -> Result<Refund, DaoRefundError> {
        DaoRefundMethods::update_refund_destination_params(self, refund_id, destination_params)
            .await
    }

    async fn commit(self) -> DaoResult<()> {
        DaoTransaction::commit(self).await
    }

    async fn rollback(self) -> DaoResult<()> {
        DaoTransaction::rollback(self).await
    }
}
