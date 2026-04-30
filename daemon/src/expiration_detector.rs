use std::time::Duration;

use kalatori_client::types::KalatoriEventExt;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::balance_checker::{
    BalanceChecker,
    BalanceCheckerError,
};
use crate::chain::InvoiceRegistry;
use crate::configs::PaymentsConfig;
use crate::dao::{
    DAO,
    DaoInterface,
    DaoTransactionInterface,
};
use crate::types::{
    Invoice,
    InvoiceEventType,
    InvoiceStatus,
    InvoiceWithReceivedAmount,
    Refund,
};

const EXPIRATION_CHECK_INTERVAL_MILLIS: u64 = 10_000;

#[derive(Debug)]
enum ExpirationDetectorError {
    DatabaseError,
}

pub struct ExpirationDetector<D: DaoInterface + 'static = DAO> {
    dao: D,
    registry: InvoiceRegistry,
    config: PaymentsConfig,
    balance_checker: BalanceChecker,
}

impl<D: DaoInterface + 'static> ExpirationDetector<D> {
    pub fn new(
        dao: D,
        registry: InvoiceRegistry,
        config: PaymentsConfig,
        balance_checker: BalanceChecker,
    ) -> Self {
        ExpirationDetector {
            dao,
            registry,
            config,
            balance_checker,
        }
    }

    async fn fetch_expired_invoices(&self) -> Vec<Invoice> {
        self.dao
            .get_expired_invoices()
            .await
            .inspect_err(|_| {
                tracing::warn!("Failed to fetch expired invoices from database");
            })
            .unwrap_or_default()
    }

    #[tracing::instrument(
        skip(self),
        fields(
            invoice_id = %invoice.invoice.id,
            total_received_amount = %invoice.total_received_amount,
            invoice_status = %invoice.invoice.status,
        )
    )]
    async fn update_invoice_expired(
        &self,
        invoice: InvoiceWithReceivedAmount,
    ) -> Result<(), ExpirationDetectorError> {
        let invoice_id = invoice.invoice.id;
        let invoice_status = invoice.invoice.status;

        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        let new_status = if invoice_status == InvoiceStatus::PartiallyPaid {
            let refund = Refund::from_invoice(
                invoice.invoice.clone(),
                invoice.total_received_amount,
            );

            let refund = dao_transaction
                .create_refund(refund)
                .await
                .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

            tracing::info!(
                refund_id = %refund.id,
                "Invoice has been partially paid, refund for respective amount created"
            );

            InvoiceStatus::PartiallyPaidExpired
        } else if invoice_status == InvoiceStatus::Waiting {
            tracing::trace!("Invoice hasn't been paid at all, no need to refund anything");
            InvoiceStatus::UnpaidExpired
        } else {
            tracing::error!(
                "Unexpected invoice status, interrupt expiration operation. It might require manual intervention"
            );
            return Err(ExpirationDetectorError::DatabaseError)
        };

        dao_transaction
            .update_invoice_status(invoice_id, new_status)
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        let event = invoice
            .into_public_invoice(&self.config.payment_url_base)
            .build_event(InvoiceEventType::Expired)
            .into();

        dao_transaction
            .create_webhook_event(event)
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        dao_transaction
            .commit()
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        self.registry
            .remove_invoice(&invoice_id)
            .await;

        tracing::info!("Invoice has been marked as expired");

        Ok(())
    }

    // 1. Update statuses in the database for expired and partially paid expired
    //    invoices
    // 2. Notify tracker, it should remove them from tracking
    // 3. Check balances one last time, if it doesn't match with recorded received
    //    amount, fetch transactions from indexer and update status
    // 4. Schedule webhooks for expired invoices
    async fn handle_expirations(&self) {
        let expired_invoices = self.fetch_expired_invoices().await;

        let expired_invoices_ids: Vec<_> = expired_invoices
            .iter()
            .map(|inv| inv.id)
            .collect();

        if expired_invoices_ids.is_empty() {
            tracing::trace!("There are no expired invoices, do nothing");
        } else {
            tracing::info!(
                expired_invoices_ids = ?expired_invoices_ids,
                "There are {} expired invoices, trying to process them", expired_invoices_ids.len()
            );
        }

        for invoice in expired_invoices {
            let invoice_id = invoice.id;

            match self
                .balance_checker
                .check_invoice_balance(invoice_id)
                .await
            {
                Ok(invoice) => {
                    // Check only final, it should be enough as long as we fetch only Waiting
                    // invoices here
                    if !invoice.invoice.status.is_final() {
                        if let Err(e) = self
                            .update_invoice_expired(invoice)
                            .await
                        {
                            tracing::warn!(
                                %invoice_id,
                                error = ?e,
                                "Failed to update invoice status to Expired in database, will retry later"
                            );
                        } else {
                            tracing::info!(
                                %invoice_id,
                                "Expired invoice has been processed successfully"
                            );
                        }
                    }
                },
                Err(BalanceCheckerError::InvoiceNotFound {
                    invoice_id,
                }) => {
                    tracing::error!(
                        %invoice_id,
                        "Invoice with that id wasn't by balance checker"
                    );
                },
                Err(e) => tracing::warn!(
                    %invoice.id,
                    invoice_status = %invoice.status,
                    error = ?e,
                    "Error while trying process expired invoice. It remains with previous status"
                ),
            };
        }
    }

    #[tracing::instrument(skip_all, fields(category = "expiration_detector"))]
    async fn perform(
        self,
        token: CancellationToken,
    ) {
        let mut interval = interval(Duration::from_millis(
            EXPIRATION_CHECK_INTERVAL_MILLIS,
        ));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.handle_expirations().await;
                }
                () = token.cancelled() => {
                    tracing::info!(
                        "Expiration detector received shutdown signal, finishing pending tasks before shutting down"
                    );

                    break
                }
            }
        }
    }

    pub fn ignite(
        self,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.perform(token).await;
        })
    }
}
