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
        // TODO: fetch partially paid expired invoices as well and return them together

        self.dao
            .get_expired_invoices()
            .await
            .inspect_err(|_| {
                tracing::warn!("Failed to fetch expired invoices from database");
            })
            .unwrap_or_default()
    }

    async fn update_invoice_expired(
        &self,
        invoice: InvoiceWithReceivedAmount,
    ) -> Result<(), ExpirationDetectorError> {
        let invoice_id = invoice.invoice.id;

        let event = invoice
            .into_public_invoice(&self.config.payment_url_base)
            .build_event(InvoiceEventType::Expired)
            .into();

        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        dao_transaction
            .create_webhook_event(event)
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        // TODO: we currently handle only unpaid expired invoices, will need also handle
        // partially paid expired
        dao_transaction
            .update_invoice_status(invoice_id, InvoiceStatus::UnpaidExpired)
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        dao_transaction
            .commit()
            .await
            .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

        self.registry
            .remove_invoice(&invoice_id)
            .await;

        tracing::info!(
            %invoice_id,
            "Invoice has been marked as expired"
        );

        Ok(())
    }

    // 1. Update statuses in the database for expired and partially paid expired
    //    invoices
    // 2. Notify tracker, it should remove them from tracking
    // 3. Check balances one last time, if it doesn't match with recorded received
    //    amount, fetch transactions from indexer and update status
    // 4. Schedule webhooks for expired invoices
    async fn handle_expirations(&self) {
        // TODO: we fetch only waiting invoices here, need to also fetch partially paid
        // in future
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
