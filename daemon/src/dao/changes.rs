//! DAO methods for fetching invoice changes with related entities.

use chrono::{
    DateTime,
    Utc,
};
use sqlx::FromRow;
use thiserror::Error;

use crate::types::{
    ChangesResponse,
    FrontEndSwap,
    FrontEndSwapJson,
    InvoiceChanges,
    InvoiceRow,
    Payout,
    PayoutChanges,
    PayoutJson,
    Refund,
    RefundChanges,
    RefundJson,
    Transaction,
    TransactionJson,
};

use super::DaoExecutor;

// ============================================================================
// Error type
// ============================================================================

#[derive(Debug, Error)]
pub enum DaoChangesError {
    #[error("Database error during changes query")]
    DatabaseError,

    #[error("Failed to parse JSON from database: {message}")]
    JsonParseError { message: String },
}

impl From<sqlx::Error> for DaoChangesError {
    fn from(_e: sqlx::Error) -> Self {
        DaoChangesError::DatabaseError
    }
}

impl crate::api::ApiErrorExt for DaoChangesError {
    fn category(&self) -> &str {
        match self {
            DaoChangesError::DatabaseError
            | DaoChangesError::JsonParseError {
                ..
            } => "INTERNAL_SERVER_ERROR",
        }
    }

    fn code(&self) -> &str {
        match self {
            DaoChangesError::DatabaseError => "DATABASE_ERROR",
            DaoChangesError::JsonParseError {
                ..
            } => "JSON_PARSE_ERROR",
        }
    }

    fn message(&self) -> &str {
        match self {
            DaoChangesError::DatabaseError => "A database error occurred.",
            DaoChangesError::JsonParseError {
                ..
            } => "Failed to parse data from database.",
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    }
}

// ============================================================================
// Row type for the aggregated query
// ============================================================================

#[derive(FromRow)]
struct InvoiceChangesRow {
    // Invoice fields
    #[sqlx(flatten)]
    invoice: InvoiceRow,

    // Aggregated JSON arrays as strings
    transactions_json: String,
    payouts_json: String,
    refunds_json: String,
    swaps_json: String,
}

// ============================================================================
// SQL Query
// ============================================================================

const GET_INVOICE_CHANGES_SQL: &str = r#"
SELECT
    i.*,
    COALESCE(
        (SELECT json_group_array(json_object(
            'id', hex(t.id),
            'invoice_id', hex(t.invoice_id),
            'asset_id', t.asset_id,
            'asset_name', t.asset_name,
            'chain', t.chain,
            'amount', t.amount,
            'source_address', t.source_address,
            'destination_address', t.destination_address,
            'block_number', t.block_number,
            'position_in_block', t.position_in_block,
            'tx_hash', t.tx_hash,
            'origin', json(t.origin),
            'status', t.status,
            'transaction_type', t.transaction_type,
            'outgoing_meta', json(t.outgoing_meta),
            'created_at', t.created_at,
            'updated_at', t.updated_at
        )) FROM transactions t WHERE t.invoice_id = i.id AND t.status = 'Completed'),
        '[]'
    ) as transactions_json,
    COALESCE(
        (SELECT json_group_array(json_object(
            'id', hex(p.id),
            'invoice_id', hex(p.invoice_id),
            'asset_id', p.asset_id,
            'asset_name', p.asset_name,
            'chain', p.chain,
            'amount', p.amount,
            'source_address', p.source_address,
            'destination_address', p.destination_address,
            'initiator_type', p.initiator_type,
            'initiator_id', hex(p.initiator_id),
            'status', p.status,
            'retry_count', p.retry_count,
            'last_attempt_at', p.last_attempt_at,
            'next_retry_at', p.next_retry_at,
            'failure_message', p.failure_message,
            'created_at', p.created_at,
            'updated_at', p.updated_at
        )) FROM payouts p WHERE p.invoice_id = i.id),
        '[]'
    ) as payouts_json,
    COALESCE(
        (SELECT json_group_array(json_object(
            'id', hex(r.id),
            'invoice_id', hex(r.invoice_id),
            'asset_id', r.asset_id,
            'asset_name', r.asset_name,
            'chain', r.chain,
            'amount', r.amount,
            'source_address', r.source_address,
            'destination_address', r.destination_address,
            'initiator_type', r.initiator_type,
            'initiator_id', hex(r.initiator_id),
            'status', r.status,
            'retry_count', r.retry_count,
            'last_attempt_at', r.last_attempt_at,
            'next_retry_at', r.next_retry_at,
            'failure_message', r.failure_message,
            'created_at', r.created_at,
            'updated_at', r.updated_at
        )) FROM refunds r WHERE r.invoice_id = i.id),
        '[]'
    ) as refunds_json,
    COALESCE(
        (SELECT json_group_array(json_object(
            'id', hex(s.id),
            'invoice_id', hex(s.invoice_id),
            'from_amount_units', s.from_amount_units,
            'from_chain_id', s.from_chain_id,
            'from_asset_id', s.from_asset_id,
            'transaction_hash', s.transaction_hash,
            'created_at', s.created_at,
            'updated_at', s.updated_at
        )) FROM front_end_swaps s WHERE s.invoice_id = i.id),
        '[]'
    ) as swaps_json
FROM invoices i
WHERE i.id IN (
    SELECT DISTINCT i2.id
    FROM invoices i2
    LEFT JOIN transactions t2 ON i2.id = t2.invoice_id
    LEFT JOIN payouts p2 ON i2.id = p2.invoice_id
    LEFT JOIN refunds r2 ON i2.id = r2.invoice_id
    LEFT JOIN front_end_swaps s2 ON i2.id = s2.invoice_id
    WHERE i2.updated_at > ?1
       OR (t2.updated_at IS NOT NULL AND t2.updated_at > ?1)
       OR (p2.updated_at IS NOT NULL AND p2.updated_at > ?1)
       OR (r2.updated_at IS NOT NULL AND r2.updated_at > ?1)
       OR (s2.updated_at IS NOT NULL AND s2.updated_at > ?1)
)
ORDER BY i.updated_at ASC
"#;

// ============================================================================
// DAO trait
// ============================================================================

pub trait DaoChangesMethods: DaoExecutor + 'static {
    /// Get all invoices with their related entities that have been modified
    /// after the given timestamp.
    ///
    /// This includes invoices where:
    /// - The invoice itself was modified
    /// - Any related transaction was modified
    /// - Any related payout was modified
    /// - Any related refund was modified
    async fn get_invoice_changes(
        &self,
        since: DateTime<Utc>,
    ) -> Result<ChangesResponse, DaoChangesError> {
        // Invoices store timestamps as naive UTC (via .naive_utc()), so we must
        // also bind `since` as naive UTC for correct string comparison in SQLite.
        let query =
            sqlx::query_as::<_, InvoiceChangesRow>(GET_INVOICE_CHANGES_SQL).bind(since.naive_utc());

        let rows: Vec<InvoiceChangesRow> = self
            .fetch_all(query)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error.category = "dao.changes",
                    error.operation = "get_invoice_changes",
                    ?since,
                    error.source = ?e,
                    "Failed to fetch invoice changes"
                );
                DaoChangesError::DatabaseError
            })?;

        let mut invoices = Vec::with_capacity(rows.len());

        for row in rows {
            let invoice_changes = parse_invoice_changes_row(row)?;
            invoices.push(invoice_changes);
        }

        Ok(ChangesResponse {
            invoices,
            sync_timestamp: Utc::now(),
        })
    }
}

impl<T: DaoExecutor + 'static> DaoChangesMethods for T {}

// ============================================================================
// Helper functions
// ============================================================================

fn parse_invoice_changes_row(row: InvoiceChangesRow) -> Result<InvoiceChanges, DaoChangesError> {
    use crate::types::TransactionType;
    use std::collections::HashMap;
    use uuid::Uuid;

    // Parse transactions JSON
    let transactions_json: Vec<TransactionJson> = serde_json::from_str(&row.transactions_json)
        .map_err(|e| {
            tracing::debug!(
                error.category = "dao.changes",
                error.operation = "parse_transactions_json",
                json = %row.transactions_json,
                error.source = ?e,
                "Failed to parse transactions JSON"
            );
            DaoChangesError::JsonParseError {
                message: format!("transactions: {e}"),
            }
        })?;

    let all_transactions: Vec<Transaction> = transactions_json
        .into_iter()
        .map(Transaction::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DaoChangesError::JsonParseError {
            message: format!("transaction conversion: {e}"),
        })?;

    // Split transactions: incoming go to invoice, outgoing go to payouts/refunds
    let mut incoming_transactions = Vec::new();
    let mut outgoing_by_payout: HashMap<Uuid, Vec<Transaction>> = HashMap::new();
    let mut outgoing_by_refund: HashMap<Uuid, Vec<Transaction>> = HashMap::new();

    for tx in all_transactions {
        if tx.transaction_type == TransactionType::Incoming {
            incoming_transactions.push(tx);
        } else {
            // Outgoing transaction - group by payout_id or refund_id
            if let Some(payout_id) = tx.origin.payout_id {
                outgoing_by_payout
                    .entry(payout_id)
                    .or_default()
                    .push(tx);
            } else if let Some(refund_id) = tx.origin.refund_id {
                outgoing_by_refund
                    .entry(refund_id)
                    .or_default()
                    .push(tx);
            }
            // Note: outgoing transactions without payout_id or refund_id are
            // dropped
        }
    }

    // Parse payouts JSON
    let payouts_json: Vec<PayoutJson> = serde_json::from_str(&row.payouts_json).map_err(|e| {
        tracing::warn!(
            error.category = "dao.changes",
            error.operation = "parse_payouts_json",
            json = %row.payouts_json,
            error.source = ?e,
            "Failed to parse payouts JSON"
        );
        DaoChangesError::JsonParseError {
            message: format!("payouts: {e}"),
        }
    })?;

    let payouts: Vec<Payout> = payouts_json
        .into_iter()
        .map(Payout::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DaoChangesError::JsonParseError {
            message: format!("payout conversion: {e}"),
        })?;

    // Build PayoutChanges with their transactions
    let payout_changes: Vec<PayoutChanges> = payouts
        .into_iter()
        .map(|payout| {
            let transactions = outgoing_by_payout
                .remove(&payout.id)
                .unwrap_or_default();
            PayoutChanges {
                payout,
                transactions,
            }
        })
        .collect();

    // Parse refunds JSON
    let refunds_json: Vec<RefundJson> = serde_json::from_str(&row.refunds_json).map_err(|e| {
        tracing::warn!(
            error.category = "dao.changes",
            error.operation = "parse_refunds_json",
            json = %row.refunds_json,
            error.source = ?e,
            "Failed to parse refunds JSON"
        );
        DaoChangesError::JsonParseError {
            message: format!("refunds: {e}"),
        }
    })?;

    let refunds: Vec<Refund> = refunds_json
        .into_iter()
        .map(Refund::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DaoChangesError::JsonParseError {
            message: format!("refund conversion: {e}"),
        })?;

    // Build RefundChanges with their transactions
    let refund_changes: Vec<RefundChanges> = refunds
        .into_iter()
        .map(|refund| {
            let transactions = outgoing_by_refund
                .remove(&refund.id)
                .unwrap_or_default();
            RefundChanges {
                refund,
                transactions,
            }
        })
        .collect();

    // Parse swaps JSON
    let swaps_json: Vec<FrontEndSwapJson> = serde_json::from_str(&row.swaps_json).map_err(|e| {
        tracing::warn!(
            error.category = "dao.changes",
            error.operation = "parse_swaps_json",
            json = %row.swaps_json,
            error.source = ?e,
            "Failed to parse swaps JSON"
        );
        DaoChangesError::JsonParseError {
            message: format!("swaps: {e}"),
        }
    })?;

    let swaps: Vec<FrontEndSwap> = swaps_json
        .into_iter()
        .map(FrontEndSwap::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DaoChangesError::JsonParseError {
            message: format!("swap conversion: {e}"),
        })?;

    Ok(InvoiceChanges {
        invoice: row.invoice.into(),
        transactions: incoming_transactions,
        payouts: payout_changes,
        refunds: refund_changes,
        swaps,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use crate::dao::create_test_dao;
    use crate::dao::invoice::DaoInvoiceMethods;
    use crate::dao::payout::DaoPayoutMethods;
    use crate::dao::transaction::DaoTransactionMethods;
    use chrono::Duration;
    use rust_decimal::Decimal;

    use crate::types::{
        TransactionStatus,
        TransactionType,
        default_create_invoice_data,
        default_payout,
        default_transaction,
    };

    use super::*;

    #[tokio::test]
    async fn test_get_invoice_changes_empty() {
        let dao = create_test_dao().await;

        // Query with current timestamp - should return empty
        let result = dao
            .get_invoice_changes(Utc::now())
            .await
            .unwrap();
        assert!(result.invoices.is_empty());
    }

    #[tokio::test]
    async fn test_get_invoice_changes_with_invoice_only() {
        let dao = create_test_dao().await;

        // Create an invoice
        let invoice_data = default_create_invoice_data();
        let invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Query with timestamp before creation
        let since = Utc::now() - Duration::hours(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        assert_eq!(result.invoices.len(), 1);
        assert_eq!(
            result.invoices[0].invoice.id,
            invoice.id
        );
        assert!(
            result.invoices[0]
                .transactions
                .is_empty()
        );
        assert!(result.invoices[0].payouts.is_empty());
        assert!(result.invoices[0].refunds.is_empty());
        assert!(result.invoices[0].swaps.is_empty());
    }

    #[tokio::test]
    async fn test_get_invoice_changes_with_transactions() {
        let dao = create_test_dao().await;

        // Create invoice
        let invoice_data = default_create_invoice_data();
        let invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Create incoming transaction (must be Completed to be included)
        let mut tx = default_transaction(invoice.id);
        tx.transaction_type = TransactionType::Incoming;
        tx.status = TransactionStatus::Completed;
        tx.transfer_info.amount = Decimal::new(10050, 2);
        dao.create_transaction(tx.clone())
            .await
            .unwrap();

        // Query
        let since = Utc::now() - Duration::hours(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        assert_eq!(result.invoices.len(), 1);
        assert_eq!(result.invoices[0].transactions.len(), 1);
        assert_eq!(
            result.invoices[0].transactions[0].id,
            tx.id
        );
    }

    #[tokio::test]
    async fn test_get_invoice_changes_with_payout() {
        let dao = create_test_dao().await;

        // Create invoice
        let invoice_data = default_create_invoice_data();
        let invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Create payout
        let payout = default_payout(invoice.id);
        dao.create_payout(payout.clone())
            .await
            .unwrap();

        // Query
        let since = Utc::now() - Duration::hours(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        assert_eq!(result.invoices.len(), 1);
        assert_eq!(result.invoices[0].payouts.len(), 1);
        assert_eq!(
            result.invoices[0].payouts[0].payout.id,
            payout.id
        );
    }

    #[tokio::test]
    async fn test_get_invoice_changes_filters_by_timestamp() {
        let dao = create_test_dao().await;

        // Create invoice
        let invoice_data = default_create_invoice_data();
        let _invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Query with future timestamp - should return empty
        let since = Utc::now() + Duration::hours(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        assert!(result.invoices.is_empty());
    }

    #[tokio::test]
    async fn test_get_invoice_changes_includes_invoice_when_transaction_updated() {
        let dao = create_test_dao().await;

        // Create old invoice (we'll simulate by just creating it)
        let invoice_data = default_create_invoice_data();
        let invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Create transaction
        let tx = default_transaction(invoice.id);
        dao.create_transaction(tx.clone())
            .await
            .unwrap();

        // Even if we query from "now", the invoice should be included
        // because the transaction was just created
        let since = Utc::now() - Duration::seconds(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        assert_eq!(result.invoices.len(), 1);
    }

    #[tokio::test]
    async fn test_get_invoice_changes_groups_outgoing_transactions_under_payouts() {
        use crate::types::TransactionOrigin;
        use uuid::Uuid;

        let dao = create_test_dao().await;

        // Create invoice
        let invoice_data = default_create_invoice_data();
        let invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Create payout
        let payout = default_payout(invoice.id);
        dao.create_payout(payout.clone())
            .await
            .unwrap();

        // Create incoming transaction (should go to invoice.transactions)
        // Must be Completed to be included in results
        let mut incoming_tx = default_transaction(invoice.id);
        incoming_tx.status = TransactionStatus::Completed;
        dao.create_transaction(incoming_tx.clone())
            .await
            .unwrap();

        // Create outgoing transaction linked to payout (should go to
        // payout.transactions)
        let mut outgoing_tx = default_transaction(invoice.id);
        outgoing_tx.id = Uuid::new_v4();
        outgoing_tx.transaction_type = TransactionType::Outgoing;
        outgoing_tx.origin = TransactionOrigin {
            payout_id: Some(payout.id),
            refund_id: None,
            internal_transfer_id: None,
        };
        outgoing_tx.transaction_id.block_number = Some(100);
        outgoing_tx.transaction_id.tx_hash = Some(Uuid::new_v4().to_string());
        outgoing_tx.status = TransactionStatus::Completed;
        dao.create_transaction(outgoing_tx.clone())
            .await
            .unwrap();

        // Query
        let since = Utc::now() - Duration::hours(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        // Verify structure
        assert_eq!(result.invoices.len(), 1);
        let inv = &result.invoices[0];

        // Invoice should have only incoming transaction
        assert_eq!(inv.transactions.len(), 1);
        assert_eq!(inv.transactions[0].id, incoming_tx.id);
        assert_eq!(
            inv.transactions[0].transaction_type,
            TransactionType::Incoming
        );

        // Payout should have the outgoing transaction
        assert_eq!(inv.payouts.len(), 1);
        assert_eq!(inv.payouts[0].payout.id, payout.id);
        assert_eq!(inv.payouts[0].transactions.len(), 1);
        assert_eq!(
            inv.payouts[0].transactions[0].id,
            outgoing_tx.id
        );
        assert_eq!(
            inv.payouts[0].transactions[0].transaction_type,
            TransactionType::Outgoing
        );
    }

    #[tokio::test]
    async fn test_get_invoice_changes_with_swaps() {
        use crate::dao::swap::DaoSwapMethods;
        use crate::types::CreateFrontEndSwapParams;
        use alloy::primitives::Address;

        let dao = create_test_dao().await;

        // Create invoice
        let invoice_data = default_create_invoice_data();
        let invoice = dao
            .create_invoice(invoice_data.clone())
            .await
            .unwrap();

        // Create front-end swap
        let swap = CreateFrontEndSwapParams {
            invoice_id: invoice.id,
            from_amount_units: 1_000_000,
            from_chain_id: 1,
            from_asset_id: Address::ZERO,
            transaction_hash: "0xabc123".to_string(),
        };
        let created_swap = dao
            .create_front_end_swap(swap.clone())
            .await
            .unwrap();

        // Query
        let since = Utc::now() - Duration::hours(1);
        let result = dao
            .get_invoice_changes(since)
            .await
            .unwrap();

        assert_eq!(result.invoices.len(), 1);
        assert_eq!(result.invoices[0].swaps.len(), 1);
        assert_eq!(
            result.invoices[0].swaps[0].invoice_id,
            invoice.id
        );
        assert_eq!(
            result.invoices[0].swaps[0].from_amount_units,
            created_swap.from_amount_units
        );
        assert_eq!(
            result.invoices[0].swaps[0].transaction_hash,
            created_swap.transaction_hash
        );
    }
}
