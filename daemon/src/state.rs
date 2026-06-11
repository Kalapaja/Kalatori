#[cfg(feature = "dev_api")]
mod dev_api;
mod swaps;

use std::collections::HashMap;
use std::io::Cursor;

use chrono::{
    Duration,
    Utc,
};
use rust_decimal::Decimal;
use secrecy::{
    ExposeSecret,
    SecretString,
};
use uuid::Uuid;

use kalatori_client::types::{
    CreateInvoiceParams,
    Invoice as PublicInvoice,
    InvoiceStatus,
    UpdateInvoiceParams,
};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::chain::InvoiceRegistry;
use crate::chain::utils::to_base58_string;
use crate::chain_client::{
    GenerateAddressData,
    KeyringClient,
};
use crate::clients::{
    GithubClient,
    GithubClientError,
};
use crate::configs::{
    PaymentsConfig,
    ShopConfig,
    ShopMetaConfig,
};
use crate::dao::{
    DAO,
    DaoChangesError,
    DaoInterface,
    DaoInvoiceError,
    DaoPayoutError,
    DaoSwapError,
    DaoTransactionError,
    DaoTransactionInterface,
};
use crate::swaps::SwapsExecutor;
use crate::types::{
    ChainType,
    ChangesResponse,
    CreateFrontEndSwapParams,
    CreateInvoiceData,
    FrontEndSwap,
    InvoiceChanges,
    InvoiceEventType,
    InvoiceWithReceivedAmount,
    KalatoriEventExt,
    KalatoriIntegrationSettings,
    KalatoriSettings,
    ListInvoicesParams,
    ListPayoutsParams,
    ListSwapsParams,
    ListTransactionsParams,
    PaginatedResponse,
    Payout,
    PayoutChanges,
    PublicAssetDescription,
    PublicChangesResponse,
    PublicSwap,
    PublicTransaction,
    RefundChanges,
    ShopPlatform,
    Swap,
    Transaction,
    TransferDestinationParams,
    UpdateInvoiceData,
};

pub use swaps::SwapRequestError;

fn validate_metadata(metadata: Option<&serde_json::Value>) -> Result<(), DaoInvoiceError> {
    let Some(value) = metadata else {
        return Ok(());
    };
    // Metadata is a key/value bag: reject scalars/arrays so consumers can rely
    // on object shape (e.g. `metadata ->> 'key'`). Matches the API spec.
    if !value.is_object() {
        return Err(DaoInvoiceError::MetadataNotObject);
    }
    if value.to_string().len() > crate::types::MAX_INVOICE_METADATA_BYTES {
        return Err(DaoInvoiceError::MetadataTooLarge);
    }
    Ok(())
}

pub struct AppState<D: DaoInterface = DAO> {
    keyring: KeyringClient,
    dao: D,
    registry: InvoiceRegistry,
    swaps_executor: SwapsExecutor<D>,
    github_client: GithubClient,
    asset_names_map: HashMap<String, String>,
    payments_config: PaymentsConfig,
    shop_config: ShopConfig,
    api_secret_key: SecretString,
}

impl<D: DaoInterface> AppState<D> {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        keyring: KeyringClient,
        dao: D,
        registry: InvoiceRegistry,
        swaps_executor: SwapsExecutor<D>,
        asset_names_map: HashMap<String, String>,
        payments_config: PaymentsConfig,
        shop_config: ShopConfig,
        api_secret_key: SecretString,
    ) -> Self {
        let github_client = GithubClient::new();

        Self {
            keyring,
            dao,
            registry,
            swaps_executor,
            github_client,
            asset_names_map,
            payments_config,
            shop_config,
            api_secret_key,
        }
    }

    pub fn invoice_to_public_invoice(
        &self,
        invoice: InvoiceWithReceivedAmount,
    ) -> PublicInvoice {
        invoice.into_public_invoice(&self.payments_config.payment_url_base)
    }

    #[tracing::instrument(skip_all)]
    pub async fn get_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<InvoiceWithReceivedAmount>, DaoInvoiceError> {
        // TODO: try to search invoice in registry first?
        self.dao
            .get_invoice_with_received_amount_by_id(invoice_id)
            .await
    }

    #[expect(clippy::arithmetic_side_effects, clippy::cast_possible_wrap)]
    #[tracing::instrument(skip_all)]
    pub async fn create_invoice(
        &self,
        params: CreateInvoiceParams,
    ) -> Result<InvoiceWithReceivedAmount, DaoInvoiceError> {
        validate_metadata(params.metadata.as_ref())?;

        let id = Uuid::new_v4();
        // Later we can extend CreateInvoiceParams to include optional chain and
        // asset_id
        let chain = self.payments_config.default_chain;

        let asset_id = self
            .payments_config
            .default_asset_id
            .get(&chain)
            .unwrap()
            .clone();

        let asset_name = self
            .asset_names_map
            .get(&asset_id)
            .cloned()
            // This should never happen, but just in case
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let valid_till = Utc::now()
            + Duration::milliseconds(
                self.payments_config
                    .invoice_lifetime_millis as i64,
            );

        let payment_address = match chain {
            ChainType::PolkadotAssetHub => {
                let derivation_params = vec![id.to_string()];

                let account_id = self
                    .keyring
                    .generate_asset_hub_address(derivation_params.into())
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error.category = "create_invoice",
                            error.operation = "generate_asset_hub_address",
                            error.source = ?e,
                            "Failed to generate payment address for new invoice",
                        );
                        // TODO: replace error
                        DaoInvoiceError::DatabaseError
                    })?;

                to_base58_string(account_id.0, 0)
            },
            ChainType::Polygon => {
                let derivation_params = vec![id.to_string()];

                let address = self
                    .keyring
                    .generate_polygon_address(GenerateAddressData::from(
                        derivation_params,
                    ))
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error.category = "create_invoice",
                            error.operation = "generate_polygon_address",
                            error.source = ?e,
                            "Failed to generate Polygon payment address for new invoice",
                        );
                        // TODO: replace error
                        DaoInvoiceError::DatabaseError
                    })?;

                // Return checksummed address
                address.to_checksum(None)
            },
        };

        let data = CreateInvoiceData {
            order_id: params.order_id,
            amount: params.amount,
            cart: params.cart,
            metadata: params.metadata,
            redirect_url: params.redirect_url,
            id,
            asset_id,
            asset_name,
            chain,
            payment_address,
            valid_till,
        };

        // TODO: handle errors properly
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;

        let invoice = dao_transaction
            .create_invoice(data)
            .await?;

        let invoice_with_amount = invoice.with_amount(Decimal::ZERO);
        let event = self
            .invoice_to_public_invoice(invoice_with_amount.clone())
            .build_event(InvoiceEventType::Created)
            .into();

        dao_transaction
            .create_webhook_event(event)
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;
        dao_transaction
            .commit()
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;

        tracing::info!(
            invoice_id = %invoice_with_amount.invoice.id,
            payment_address = %invoice_with_amount.invoice.payment_address,
            "Created new invoice",
        );

        self.registry
            .add_invoice(invoice_with_amount.clone())
            .await;

        Ok(invoice_with_amount)
    }

    #[expect(clippy::arithmetic_side_effects, clippy::cast_possible_wrap)]
    pub async fn update_invoice(
        &self,
        params: UpdateInvoiceParams,
    ) -> Result<InvoiceWithReceivedAmount, DaoInvoiceError> {
        validate_metadata(params.metadata.as_ref())?;

        let data = UpdateInvoiceData {
            invoice_id: params.invoice_id,
            amount: params.amount,
            cart: params.cart,
            metadata: params.metadata,
            valid_till: Utc::now()
                + Duration::milliseconds(
                    self.payments_config
                        .invoice_lifetime_millis as i64,
                ),
        };

        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;

        let result = dao_transaction
            .update_invoice_data(data)
            .await?;
        let invoice_with_amount = result
            .clone()
            .with_amount(Decimal::ZERO);
        let event = self
            .invoice_to_public_invoice(invoice_with_amount)
            .build_event(InvoiceEventType::Updated)
            .into();

        dao_transaction
            .create_webhook_event(event)
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;
        dao_transaction
            .commit()
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;

        tracing::info!(
            invoice_id = %result.id,
            "Invoice has been updated",
        );

        // We allow to update only unpaid invoices, so the received amount is zero
        let result = result.with_amount(Decimal::ZERO);

        Ok(result)
    }

    pub async fn cancel_invoice_admin(
        &self,
        invoice_id: Uuid,
    ) -> Result<InvoiceWithReceivedAmount, DaoInvoiceError> {
        // TODO: if invoice has been partially paid, we need to also handle refunds
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_| DaoInvoiceError::DatabaseError)?;

        // TODO: refactor it. If invoice not in registry, it probably has non-active
        // status and can not be canceled anymore
        let result = if let Some(invoice_with_amount) = self
            .registry
            .remove_invoice(&invoice_id)
            .await
        {
            let result = self
                .dao
                .update_invoice_status(invoice_id, InvoiceStatus::AdminCanceled)
                .await?;

            let invoice_with_amount = result.with_amount(invoice_with_amount.total_received_amount);
            let event = self
                .invoice_to_public_invoice(invoice_with_amount.clone())
                .build_event(InvoiceEventType::AdminCanceled)
                .into();

            dao_transaction
                .create_webhook_event(event)
                .await
                .map_err(|_| DaoInvoiceError::DatabaseError)?;
            dao_transaction
                .commit()
                .await
                .map_err(|_| DaoInvoiceError::DatabaseError)?;
            invoice_with_amount
        } else {
            let result = self
                .dao
                .update_invoice_status(invoice_id, InvoiceStatus::AdminCanceled)
                .await?;

            let invoice_with_amount = result.with_amount(Decimal::ZERO);
            let event = self
                .invoice_to_public_invoice(invoice_with_amount.clone())
                .build_event(InvoiceEventType::AdminCanceled)
                .into();

            dao_transaction
                .create_webhook_event(event)
                .await
                .map_err(|_| DaoInvoiceError::DatabaseError)?;
            dao_transaction
                .commit()
                .await
                .map_err(|_| DaoInvoiceError::DatabaseError)?;
            invoice_with_amount
        };

        tracing::info!(
            invoice_id = %invoice_id,
            "Invoice has been canceled by admin",
        );

        Ok(result)
    }

    #[tracing::instrument(skip_all)]
    pub async fn list_invoices(
        &self,
        params: &ListInvoicesParams,
    ) -> Result<PaginatedResponse<PublicInvoice>, DaoInvoiceError> {
        let (invoices, total) = tokio::join!(
            self.dao.get_invoices_paginated(params),
            self.dao.count_invoices(params),
        );

        let items = invoices?
            .into_iter()
            .map(|inv| self.invoice_to_public_invoice(inv))
            .collect();

        Ok(PaginatedResponse::new(
            items,
            total?,
            params.pagination.validated_page(),
            params.pagination.validated_per_page(),
        ))
    }

    pub async fn get_payout(
        &self,
        payout_id: Uuid,
    ) -> Result<Option<Payout>, DaoPayoutError> {
        self.dao
            .get_payout_by_id(payout_id)
            .await
    }

    #[tracing::instrument(skip_all)]
    pub async fn list_payouts(
        &self,
        params: &ListPayoutsParams,
    ) -> Result<PaginatedResponse<Payout>, DaoPayoutError> {
        let (payouts, total) = tokio::join!(
            self.dao.get_payouts_paginated(params),
            self.dao.count_payouts(params),
        );

        Ok(PaginatedResponse::new(
            payouts?,
            total?,
            params.pagination.validated_page(),
            params.pagination.validated_per_page(),
        ))
    }

    #[tracing::instrument(skip_all)]
    pub async fn initiate_payout(
        &self,
        invoice_id: Uuid,
    ) -> Result<Payout, DaoInvoiceError> {
        let invoice = self
            .dao
            .get_invoice_by_id(invoice_id)
            .await?
            .ok_or(DaoInvoiceError::NotFound {
                invoice_id,
            })?;

        if invoice.status.is_active() {
            return Err(DaoInvoiceError::UpdateNotAllowed {
                invoice_id,
                current_status: invoice.status,
            })
        }

        let destination_address = self
            .payments_config
            .recipient
            .get(&invoice.chain)
            .unwrap()
            .clone();

        let destination_params = TransferDestinationParams {
            destination_chain: invoice.chain.into(),
            destination_asset_id: invoice.asset_id.clone(),
            destination_address,
        };

        let payout = Payout::from_invoice(
            invoice,
            destination_params,
            Decimal::new(21, 2),
        );

        self.dao
            .create_payout(payout)
            .await
            .map_err(|_e| DaoInvoiceError::DatabaseError)
    }

    pub async fn get_transaction(
        &self,
        transaction_id: Uuid,
    ) -> Result<Option<Transaction>, DaoTransactionError> {
        self.dao
            .get_transaction_by_id(transaction_id)
            .await
    }

    #[tracing::instrument(skip_all)]
    pub async fn list_transactions(
        &self,
        params: &ListTransactionsParams,
    ) -> Result<PaginatedResponse<PublicTransaction>, DaoTransactionError> {
        let (transactions, total) = tokio::join!(
            self.dao
                .get_transactions_paginated(params),
            self.dao.count_transactions(params),
        );

        let items = transactions?
            .into_iter()
            .map(PublicTransaction::from)
            .collect();

        Ok(PaginatedResponse::new(
            items,
            total?,
            params.pagination.validated_page(),
            params.pagination.validated_per_page(),
        ))
    }

    pub async fn get_swap(
        &self,
        swap_id: Uuid,
    ) -> Result<Option<Swap>, DaoSwapError> {
        self.dao.get_swap_by_id(swap_id).await
    }

    #[tracing::instrument(skip_all)]
    pub async fn list_swaps(
        &self,
        params: &ListSwapsParams,
    ) -> Result<PaginatedResponse<PublicSwap>, DaoSwapError> {
        let (swaps, total) = tokio::join!(
            self.dao.get_swaps_paginated(params),
            self.dao.count_swaps(params),
        );

        let items = swaps?
            .into_iter()
            .map(PublicSwap::from)
            .collect();

        Ok(PaginatedResponse::new(
            items,
            total?,
            params.pagination.validated_page(),
            params.pagination.validated_per_page(),
        ))
    }

    pub async fn get_invoice_transactions(
        &self,
        invoice_id: Uuid,
    ) -> Result<Vec<Transaction>, DaoTransactionError> {
        self.dao
            .get_invoice_transactions(invoice_id)
            .await
    }

    #[tracing::instrument(skip_all)]
    pub async fn get_invoice_changes(
        &self,
        since: Option<chrono::DateTime<Utc>>,
    ) -> Result<PublicChangesResponse, DaoChangesError> {
        let mut internal_response = if let Some(since) = since {
            self.dao
                .get_invoice_changes(since)
                .await?
        } else {
            let sync_timestamp = Utc::now();

            let invoices = self
                .dao
                .get_all_invoices()
                .await
                .map_err(|_| DaoChangesError::DatabaseError)?;

            let transactions = self
                .dao
                .get_all_transactions()
                .await
                .map_err(|_| DaoChangesError::DatabaseError)?;

            let mut transactions_by_invoice_id = HashMap::<_, Vec<_>>::new();

            for transaction in transactions {
                transactions_by_invoice_id
                    .entry(transaction.invoice_id)
                    .or_default()
                    .push(transaction);
            }

            let payouts = self
                .dao
                .get_all_payouts()
                .await
                .map_err(|_| DaoChangesError::DatabaseError)?;

            let mut payouts_by_invoice_id = HashMap::<_, Vec<_>>::new();

            for payout in payouts {
                let transactions = transactions_by_invoice_id
                    .get(&payout.invoice_id)
                    .map(|trans| {
                        trans
                            .iter()
                            .filter(|trans| trans.origin.payout_id == Some(payout.id))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();

                let changes = PayoutChanges {
                    payout,
                    transactions,
                };

                payouts_by_invoice_id
                    .entry(changes.payout.invoice_id)
                    .or_default()
                    .push(changes);
            }

            let refunds = self
                .dao
                .get_all_refunds()
                .await
                .map_err(|_| DaoChangesError::DatabaseError)?;

            let mut refunds_by_invoice_id = HashMap::<_, Vec<_>>::new();

            for refund in refunds {
                let transactions = transactions_by_invoice_id
                    .get(&refund.invoice_id)
                    .map(|trans| {
                        trans
                            .iter()
                            .filter(|trans| trans.origin.refund_id == Some(refund.id))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();

                let changes = RefundChanges {
                    refund,
                    transactions,
                };

                refunds_by_invoice_id
                    .entry(changes.refund.invoice_id)
                    .or_default()
                    .push(changes);
            }

            let swaps = self
                .dao
                .get_all_front_end_swaps()
                .await
                .map_err(|_| DaoChangesError::DatabaseError)?;

            let mut swaps_by_invoice_id = HashMap::<_, Vec<_>>::new();

            for swap in swaps {
                swaps_by_invoice_id
                    .entry(swap.invoice_id)
                    .or_default()
                    .push(swap);
            }

            let invoices_response: Vec<_> = invoices
                .into_iter()
                .map(|invoice| {
                    let payouts = payouts_by_invoice_id
                        .remove(&invoice.id)
                        .unwrap_or_default();
                    let refunds = refunds_by_invoice_id
                        .remove(&invoice.id)
                        .unwrap_or_default();
                    let swaps = swaps_by_invoice_id
                        .remove(&invoice.id)
                        .unwrap_or_default();

                    let transactions = transactions_by_invoice_id
                        .remove(&invoice.id)
                        .map(|trans| {
                            trans
                                .into_iter()
                                .filter(|t| t.is_incoming())
                                .collect()
                        })
                        .unwrap_or_default();

                    InvoiceChanges {
                        invoice,
                        payouts,
                        refunds,
                        swaps,
                        transactions,
                    }
                })
                .collect();

            ChangesResponse {
                invoices: invoices_response,
                sync_timestamp,
            }
        };

        internal_response
            .invoices
            .sort_by_key(|i| i.invoice.updated_at);

        Ok(internal_response.into_public(&self.payments_config.payment_url_base))
    }

    pub fn get_shop_meta(&self) -> ShopMetaConfig {
        self.shop_config.meta.clone()
    }

    pub fn get_kalatori_settings(&self) -> KalatoriSettings {
        let assets_description = self
            .asset_names_map
            .iter()
            .map(|(asset_id, asset_name)| {
                (
                    asset_id.clone(),
                    PublicAssetDescription {
                        asset_id: asset_id.clone(),
                        asset_name: asset_name.clone(),
                    },
                )
            })
            .collect();

        KalatoriSettings {
            shop_url: self.shop_config.meta.shop_url.clone(),
            shop_name: self.shop_config.meta.shop_name.clone(),
            logo_url: self.shop_config.meta.logo_url.clone(),
            recipient_addresses: self.payments_config.recipient.clone(),
            invoice_lifetime_millis: self
                .payments_config
                .invoice_lifetime_millis,
            default_chain: self.payments_config.default_chain,
            default_asset_id: self
                .payments_config
                .default_asset_id
                .clone(),
            payment_url_base: self
                .payments_config
                .payment_url_base
                .clone(),
            slippage_params: self
                .payments_config
                .slippage_params
                .clone(),
            assets_description,
        }
    }

    pub fn get_kalatori_integration_settings(&self) -> KalatoriIntegrationSettings {
        KalatoriIntegrationSettings {
            invoices_webhook_url: self
                .shop_config
                .invoices_webhook_url
                .clone(),
            signature_max_age_secs: self.shop_config.signature_max_age_secs,
            private_api_base_url: self
                .shop_config
                .private_api_base_url
                .as_ref()
                .unwrap_or(&self.payments_config.payment_url_base)
                .clone(),
            api_secret_key: self
                .api_secret_key
                .expose_secret()
                .to_string(),
            supported_platforms: ShopPlatform::all(),
            shop_platform: self.shop_config.shop_platform.clone(),
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_shop_plugin(
        &self,
        platform: ShopPlatform,
    ) -> Result<Vec<u8>, GithubClientError> {
        let plugin = self
            .github_client
            .find_and_fetch_plugin(
                platform.plugin_repo(),
                platform.supported_versions(),
                platform.plugin_asset_name(),
            )
            .await?;

        let mut archive = ZipWriter::new_append(Cursor::new(plugin.to_vec())).map_err(|error| {
            tracing::error!(
                ?error,
                "Error while trying to open plugin archive"
            );

            GithubClientError::UnknownApiError
        })?;

        let options = SimpleFileOptions::default();

        archive
            .start_file(platform.config_file_name(), options)
            .map_err(|error| {
                tracing::error!(
                    ?error,
                    "Error while trying to append file to plugin archive"
                );

                GithubClientError::UnknownApiError
            })?;

        let url = self
            .shop_config
            .private_api_base_url
            .as_ref()
            .unwrap_or(&self.payments_config.payment_url_base)
            .clone();

        let admin_url = format!("{url}/admin");

        let config = platform.build_config_file(
            self.api_secret_key
                .expose_secret()
                .to_string(),
            url,
            admin_url,
        );

        serde_json::to_writer(&mut archive, &config).map_err(|error| {
            tracing::error!(
                ?error,
                "Error while trying to write config to plugin archive"
            );

            GithubClientError::UnknownApiError
        })?;

        let finished = archive.finish().map_err(|error| {
            tracing::error!(
                ?error,
                "Error while trying to finish archive after config write"
            );

            GithubClientError::UnknownApiError
        })?;

        let result = finished.into_inner();

        Ok(result)
    }

    pub async fn create_front_end_swap(
        &self,
        data: CreateFrontEndSwapParams,
    ) -> Result<FrontEndSwap, DaoSwapError> {
        self.dao
            .create_front_end_swap(data)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use mockall::predicate::eq;

    use crate::chain_client::KeyringError;
    use crate::dao::{
        MockDaoInterface,
        MockDaoTransactionInterface,
    };
    use crate::types::{
        DetectedShopPlatform,
        Invoice,
        InvoiceCart,
        default_invoice,
    };

    use super::*;

    async fn setup_app_state() -> AppState<MockDaoInterface> {
        let asset_names_map = HashMap::from([
            (1337.to_string(), "USDC".to_string()),
            (1984.to_string(), "USDt".to_string()),
        ]);

        let config = PaymentsConfig {
            default_chain: ChainType::PolkadotAssetHub,
            default_asset_id: HashMap::from([(
                ChainType::PolkadotAssetHub,
                1337.to_string(),
            )]),
            invoice_lifetime_millis: 600_000,
            recipient: HashMap::from([(
                ChainType::PolkadotAssetHub,
                "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
            )]),
            payment_url_base: "https://payments.example.com".to_string(),
            slippage_params: HashMap::new(),
        };

        let meta = ShopMetaConfig {
            shop_name: "Mega shop".to_string(),
            shop_url: "mega.shop".to_string(),
            logo_url: None,
            reown_project_id: "test".to_string(),
            ankr_api_token: None,
        };

        let shop_config = ShopConfig {
            invoices_webhook_url: Some("http://test.com/webhook".to_string()),
            signature_max_age_secs: 300,
            private_api_base_url: None,
            meta,
            shop_platform: DetectedShopPlatform::Unknown,
        };

        let keyring = KeyringClient::default();
        let dao = MockDaoInterface::default();
        let registry = InvoiceRegistry::new();
        let swaps_executor = SwapsExecutor::default();

        AppState::new(
            keyring,
            dao,
            registry,
            swaps_executor,
            asset_names_map,
            config,
            shop_config,
            SecretString::from("secret"),
        )
    }

    fn compare_create_invoice_data(
        expected: &CreateInvoiceData,
        actual: &CreateInvoiceData,
    ) -> bool {
        // We don't compare IDs here, as they are generated randomly
        expected.order_id == actual.order_id
            && expected.amount == actual.amount
            && expected.cart == actual.cart
            && expected.metadata == actual.metadata
            && expected.redirect_url == actual.redirect_url
            && expected.asset_id == actual.asset_id
            && expected.chain == actual.chain
            && expected.payment_address == actual.payment_address
            // It might be off by a few milliseconds, so we compare timestamps.
            // It still might fail if the test runs too slow, but it's unlikely.
            && expected.valid_till.timestamp() == actual.valid_till.timestamp()
    }

    // TODO: we'll replace expected id with returned one (already done) and add
    // method for invoice to strip dates (hidden behind `#[cfg(test)]`) and
    // we'll be able to compare them using Eq trait and get rid of this function
    fn compare_created_invoice(
        expected: &Invoice,
        actual: &Invoice,
    ) -> bool {
        expected.id == actual.id
            && expected.order_id == actual.order_id
            && expected.asset_id == actual.asset_id
            && expected.asset_name == actual.asset_name
            && expected.chain == actual.chain
            && expected.amount == actual.amount
            && expected.payment_address == actual.payment_address
            && expected.status == actual.status
            && expected.cart == actual.cart
            && expected.metadata == actual.metadata
            && expected.redirect_url == actual.redirect_url
            // It might be off by a few milliseconds, so we compare timestamps.
            // It still might fail if the test runs too slow, but it's unlikely.
            && expected.valid_till.timestamp() == actual.valid_till.timestamp()
            && expected.created_at.timestamp() == actual.created_at.timestamp()
            && expected.updated_at.timestamp() == actual.updated_at.timestamp()
    }

    #[tokio::test]
    async fn test_get_invoice() {
        let mut app_state = setup_app_state().await;
        let invoice_id = Uuid::new_v4();

        // Test case 1: Invoice found
        let invoice = Invoice {
            id: invoice_id,
            ..default_invoice()
        }
        .with_amount(Decimal::ONE);

        let returning_invoice = invoice.clone();

        app_state
            .dao
            .expect_get_invoice_with_received_amount_by_id()
            .once()
            .with(eq(invoice_id))
            .returning(move |_| Ok(Some(returning_invoice.clone())));

        let result = app_state
            .get_invoice(invoice_id)
            .await
            .unwrap();

        assert_eq!(result, Some(invoice));

        // Test case 2: Invoice not found
        let invoice_id = Uuid::new_v4();

        app_state
            .dao
            .expect_get_invoice_with_received_amount_by_id()
            .once()
            .with(eq(invoice_id))
            .returning(|_| Ok(None));

        let result = app_state
            .get_invoice(invoice_id)
            .await
            .unwrap();

        assert_eq!(result, None);

        // Test case 3: Database error
        let invoice_id = Uuid::new_v4();

        app_state
            .dao
            .expect_get_invoice_with_received_amount_by_id()
            .once()
            .with(eq(invoice_id))
            .returning(|_| Err(DaoInvoiceError::DatabaseError));

        let result = app_state.get_invoice(invoice_id).await;

        assert!(matches!(
            result,
            Err(DaoInvoiceError::DatabaseError)
        ));
    }

    #[tokio::test]
    async fn test_create_invoice() {
        let mut app_state = setup_app_state().await;

        let uri = subxt_signer::SecretUri::from_str("//Bob").unwrap();
        let keypair = subxt_signer::sr25519::Keypair::from_uri(&uri).unwrap();
        let account_id = keypair.public_key().to_account_id();
        // Multiple clones to move into closures
        let bob_account_id_1 = account_id.clone();
        let bob_account_id_2 = account_id.clone();

        // Test case 1: Successful invoice creation
        // Expected:
        // - KeyringClient called to generate address
        // - Asset ID replaced with default value (not provided in params)
        // - DAO called to create invoice
        // - Registry updated with new invoice
        let params = CreateInvoiceParams {
            order_id: "order123".to_string(),
            amount: Decimal::new(1000, 2), // 10.00
            cart: InvoiceCart::empty(),
            metadata: Some(serde_json::json!({"external_ref": "abc-123"})),
            redirect_url: "https://redirect.url".to_string(),
            include_transactions: false,
        };

        app_state
            .keyring
            .expect_generate_asset_hub_address()
            .once()
            .withf(|data| {
                data.derivation_params.len() == 1
                    && Uuid::from_str(&data.derivation_params[0]).is_ok()
            })
            .returning(move |_| Ok(bob_account_id_1.clone()));

        let expected_create_invoice_data = {
            CreateInvoiceData {
                id: Uuid::new_v4(), // We can't predict this, so we'll match fields except ID
                order_id: params.order_id.clone(),
                amount: params.amount,
                cart: params.cart.clone(),
                metadata: params.metadata.clone(),
                redirect_url: params.redirect_url.clone(),
                asset_id: 1337.to_string(),
                asset_name: "USDC".to_string(),
                chain: ChainType::PolkadotAssetHub,
                payment_address: to_base58_string(account_id.0, 0),
                valid_till: Utc::now()
                    + Duration::milliseconds(
                        app_state
                            .payments_config
                            .invoice_lifetime_millis as i64,
                    ),
            }
        };

        let expected_invoice: Invoice = expected_create_invoice_data
            .clone()
            .into();

        let mut expected_invoice_with_amount = expected_invoice.with_amount(Decimal::ZERO);
        let mut dao_transaction = MockDaoTransactionInterface::default();

        dao_transaction
            .expect_create_invoice()
            .once()
            .withf(move |data| compare_create_invoice_data(&expected_create_invoice_data, data))
            .returning(|data| Ok(data.into()));

        dao_transaction
            .expect_create_webhook_event()
            // we can not compare event here because of entity ID which we don't know at this point
            .once()
            .returning(Ok);

        dao_transaction
            .expect_commit()
            .once()
            .returning(|| Ok(()));

        app_state
            .dao
            .expect_begin_transaction()
            .once()
            .return_once(move || Ok(dao_transaction));

        let result = app_state
            .create_invoice(params.clone())
            .await
            .unwrap();

        expected_invoice_with_amount.invoice.id = result.invoice.id; // Set the ID to match for comparison
        assert!(compare_created_invoice(
            &expected_invoice_with_amount.invoice,
            &result.invoice
        ));

        let registry_record = app_state
            .registry
            .get_invoice(&result.invoice.id)
            .await
            .unwrap();
        assert_eq!(registry_record, result);
        assert!(
            registry_record
                .total_received_amount
                .is_zero()
        );

        // Test case 2: Keyring error
        // Expected:
        // - KeyringClient called to generate address
        // - Error propagated
        // - DAO not called
        // - Registry not updated
        let params = CreateInvoiceParams {
            order_id: "order456".to_string(),
            amount: Decimal::new(5000, 2), // 50.00
            cart: InvoiceCart::empty(),
            metadata: None,
            redirect_url: "https://redirect.url".to_string(),
            include_transactions: false,
        };

        app_state
            .keyring
            .expect_generate_asset_hub_address()
            .once()
            .withf(|data| {
                data.derivation_params.len() == 1
                    && Uuid::from_str(&data.derivation_params[0]).is_ok()
            })
            .returning(move |_| Err(KeyringError::InvalidSeed));

        let result = app_state
            .create_invoice(params.clone())
            .await;

        assert!(matches!(
            result,
            Err(DaoInvoiceError::DatabaseError)
        ));
        let registry_records_count = app_state
            .registry
            .invoices_count()
            .await;
        assert_eq!(registry_records_count, 1); // Only the previous successful invoice is present

        // Test case 3: DAO error
        // Expected:
        // - KeyringClient called to generate address
        // - DAO called to create invoice
        // - Error propagated
        // - Registry not updated
        // - Previous registry entries remain
        let params = CreateInvoiceParams {
            order_id: "order789".to_string(),
            amount: Decimal::new(7500, 2), // 75.00
            cart: InvoiceCart::empty(),
            metadata: None,
            redirect_url: "https://redirect.url".to_string(),
            include_transactions: false,
        };

        let expected_create_invoice_data = {
            CreateInvoiceData {
                id: Uuid::new_v4(), // We can't predict this, so we'll match fields except ID
                order_id: params.order_id.clone(),
                amount: params.amount,
                cart: params.cart.clone(),
                metadata: None,
                redirect_url: params.redirect_url.clone(),
                asset_id: 1337.to_string(),
                asset_name: "USDC".to_string(),
                chain: ChainType::PolkadotAssetHub,
                payment_address: to_base58_string(account_id.0, 0),
                valid_till: Utc::now()
                    + Duration::milliseconds(
                        app_state
                            .payments_config
                            .invoice_lifetime_millis as i64,
                    ),
            }
        };

        app_state
            .keyring
            .expect_generate_asset_hub_address()
            .once()
            .withf(|data| {
                data.derivation_params.len() == 1
                    && Uuid::from_str(&data.derivation_params[0]).is_ok()
            })
            .returning(move |_| Ok(bob_account_id_2.clone()));

        let mut dao_transaction = MockDaoTransactionInterface::default();

        dao_transaction
            .expect_create_invoice()
            .once()
            .withf(move |data| compare_create_invoice_data(&expected_create_invoice_data, data))
            .returning(|_| Err(DaoInvoiceError::DatabaseError));

        app_state
            .dao
            .expect_begin_transaction()
            .once()
            .return_once(|| Ok(dao_transaction));

        let result = app_state.create_invoice(params).await;

        assert!(matches!(
            result,
            Err(DaoInvoiceError::DatabaseError)
        ));
        let registry_records_count = app_state
            .registry
            .invoices_count()
            .await;
        assert_eq!(registry_records_count, 1); // Only the first successful invoice is present
    }

    #[tokio::test]
    async fn test_create_invoice_metadata_too_large() {
        // Oversized metadata is rejected before any keyring or DAO calls
        // (mocks have no expectations set, so any call would panic)
        let app_state = setup_app_state().await;

        let params = CreateInvoiceParams {
            order_id: "order-oversized-metadata".to_string(),
            amount: Decimal::new(1000, 2),
            cart: InvoiceCart::empty(),
            metadata: Some(serde_json::json!({
                "blob": "x".repeat(crate::types::MAX_INVOICE_METADATA_BYTES),
            })),
            redirect_url: "https://redirect.url".to_string(),
            include_transactions: false,
        };

        let result = app_state.create_invoice(params).await;

        assert!(matches!(
            result,
            Err(DaoInvoiceError::MetadataTooLarge)
        ));
    }

    #[tokio::test]
    async fn test_create_invoice_metadata_not_object() {
        // Non-object metadata (array/scalar) is rejected before any DAO calls.
        let app_state = setup_app_state().await;

        let params = CreateInvoiceParams {
            order_id: "order-array-metadata".to_string(),
            amount: Decimal::new(1000, 2),
            cart: InvoiceCart::empty(),
            metadata: Some(serde_json::json!([
                "not", "an", "object"
            ])),
            redirect_url: "https://redirect.url".to_string(),
            include_transactions: false,
        };

        let result = app_state.create_invoice(params).await;

        assert!(matches!(
            result,
            Err(DaoInvoiceError::MetadataNotObject)
        ));
    }

    #[test]
    fn test_validate_metadata_boundaries() {
        // None is always valid.
        assert!(validate_metadata(None).is_ok());

        // An object exactly at the cap is accepted; one byte over is rejected.
        // Build an object whose compact serialization length we control.
        let filler = "x".repeat(crate::types::MAX_INVOICE_METADATA_BYTES);
        let at_or_over = serde_json::json!({ "k": filler });
        let serialized_len = at_or_over.to_string().len();
        assert!(serialized_len > crate::types::MAX_INVOICE_METADATA_BYTES);
        assert!(matches!(
            validate_metadata(Some(&at_or_over)),
            Err(DaoInvoiceError::MetadataTooLarge)
        ));

        // Trim the filler so the whole object serializes to exactly the cap.
        let overshoot = serialized_len - crate::types::MAX_INVOICE_METADATA_BYTES;
        let exact = serde_json::json!({ "k": "x".repeat(filler.len() - overshoot) });
        assert_eq!(
            exact.to_string().len(),
            crate::types::MAX_INVOICE_METADATA_BYTES
        );
        assert!(validate_metadata(Some(&exact)).is_ok());

        // Scalars and arrays are rejected.
        assert!(matches!(
            validate_metadata(Some(&serde_json::json!("string"))),
            Err(DaoInvoiceError::MetadataNotObject)
        ));
        assert!(matches!(
            validate_metadata(Some(&serde_json::json!([1, 2, 3]))),
            Err(DaoInvoiceError::MetadataNotObject)
        ));
    }
}
