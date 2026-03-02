use std::collections::{
    HashMap,
    HashSet,
};
use std::sync::Arc;

use rust_decimal::Decimal;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::types::{
    ChainType,
    InvoiceWithReceivedAmount,
};

#[derive(Clone)]
pub struct InvoiceRegistry {
    invoices: Arc<RwLock<HashMap<Uuid, InvoiceWithReceivedAmount>>>,
}

impl InvoiceRegistry {
    pub fn new() -> Self {
        InvoiceRegistry {
            invoices: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_invoice(
        &self,
        record: InvoiceWithReceivedAmount,
    ) {
        let mut invoices = self.invoices.write().await;
        invoices.insert(record.invoice.id, record);
    }

    pub async fn add_invoices(
        &self,
        records: Vec<InvoiceWithReceivedAmount>,
    ) {
        let mut invoices_map = self.invoices.write().await;

        for record in records {
            invoices_map.insert(record.invoice.id, record);
        }
    }

    pub async fn remove_invoice(
        &self,
        invoice_id: &Uuid,
    ) -> Option<InvoiceWithReceivedAmount> {
        let mut invoices = self.invoices.write().await;
        invoices.remove(invoice_id)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn remove_invoices(
        &self,
        invoices_ids: &[Uuid],
    ) -> Vec<InvoiceWithReceivedAmount> {
        let mut invoices = self.invoices.write().await;
        let mut removed_invoices = Vec::with_capacity(invoices_ids.len());

        for invoice_id in invoices_ids {
            if let Some(invoice) = invoices.remove(invoice_id) {
                removed_invoices.push(invoice);
            }
        }

        removed_invoices
    }

    pub async fn get_invoice(
        &self,
        invoice_id: &Uuid,
    ) -> Option<InvoiceWithReceivedAmount> {
        let invoices = self.invoices.read().await;
        invoices.get(invoice_id).cloned()
    }

    pub async fn find_invoice_by_address(
        &self,
        address: &str,
        chain: ChainType,
        asset_id: &str,
    ) -> Option<InvoiceWithReceivedAmount> {
        // TODO: if we'll have large amount of invoices and incoming transfers
        // it might be a problem and should be optimized
        let invoices = self.invoices.read().await;

        invoices
            .values()
            .find(|inv| {
                inv.invoice.chain == chain
                    && inv.invoice.payment_address == address
                    && inv.invoice.asset_id == asset_id
            })
            .cloned()
    }

    pub async fn update_filled_amount(
        &self,
        invoice_id: &Uuid,
        new_filled_amount: Decimal,
    ) {
        let mut invoices = self.invoices.write().await;

        if let Some(record) = invoices.get_mut(invoice_id) {
            record.total_received_amount = new_filled_amount;
        }
    }

    pub async fn used_asset_ids(&self) -> HashMap<ChainType, HashSet<String>> {
        let invoices = self.invoices.read().await;
        let mut asset_ids_map: HashMap<_, HashSet<_>> = HashMap::new();

        for record in invoices.values() {
            asset_ids_map
                .entry(record.invoice.chain)
                .or_default()
                .insert(record.invoice.asset_id.clone());
        }

        asset_ids_map
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn invoices_count(&self) -> usize {
        let invoices = self.invoices.read().await;
        invoices.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{
        Invoice,
        default_invoice,
    };

    use super::*;

    #[tokio::test]
    async fn test_invoice_registry() {
        let registry = InvoiceRegistry::new();

        // Registry should be empty by default
        assert_eq!(registry.invoices_count().await, 0);
        assert!(
            registry
                .used_asset_ids()
                .await
                .is_empty()
        );

        let invoice_1_id = Uuid::new_v4();
        let invoice_1_amount = Decimal::ZERO;
        let invoice_1 = Invoice {
            id: invoice_1_id,
            chain: ChainType::PolkadotAssetHub,
            asset_id: "1".to_string(),
            payment_address: "1".to_string(),
            ..default_invoice()
        }
        .with_amount(invoice_1_amount);

        let invoice_2_id = Uuid::new_v4();
        let invoice_2_amount = Decimal::TEN;
        let invoice_2 = Invoice {
            id: invoice_2_id,
            chain: ChainType::PolkadotAssetHub,
            asset_id: "2".to_string(),
            payment_address: "2".to_string(),
            ..default_invoice()
        }
        .with_amount(invoice_2_amount);

        let invoice_3_id = Uuid::new_v4();
        let invoice_3_amount = Decimal::ONE_HUNDRED;
        let invoice_3 = Invoice {
            id: invoice_3_id,
            chain: ChainType::Polygon,
            asset_id: "3".to_string(),
            payment_address: "3".to_string(),
            ..default_invoice()
        }
        .with_amount(invoice_3_amount);

        // Invoice 4 has same payment address and asset id as invoice_1
        // but different chain
        let invoice_4_id = Uuid::new_v4();
        let invoice_4_amount = Decimal::ONE_HUNDRED;
        let invoice_4 = Invoice {
            id: invoice_4_id,
            chain: ChainType::Polygon,
            asset_id: "1".to_string(),
            payment_address: "1".to_string(),
            ..default_invoice()
        }
        .with_amount(invoice_4_amount);

        // Invoice 5 has same payment address and chain as invoice_4
        // but different asset_id
        let invoice_5_id = Uuid::new_v4();
        let invoice_5_amount = Decimal::ONE_HUNDRED;
        let invoice_5 = Invoice {
            id: invoice_5_id,
            chain: ChainType::Polygon,
            asset_id: "5".to_string(),
            payment_address: "1".to_string(),
            ..default_invoice()
        }
        .with_amount(invoice_5_amount);

        // Add one invoice, ensure only this one invoice can be found
        registry
            .add_invoice(invoice_1.clone())
            .await;
        assert_eq!(registry.invoices_count().await, 1);
        assert_eq!(
            registry
                .get_invoice(&invoice_1_id)
                .await,
            Some(invoice_1.clone())
        );
        assert!(
            registry
                .get_invoice(&invoice_2_id)
                .await
                .is_none()
        );
        assert!(
            registry
                .get_invoice(&invoice_3_id)
                .await
                .is_none()
        );
        assert!(
            registry
                .get_invoice(&invoice_4_id)
                .await
                .is_none()
        );
        assert!(
            registry
                .get_invoice(&Uuid::new_v4())
                .await
                .is_none()
        );

        // Add more invoices, ensure they are all returned
        registry
            .add_invoices(vec![
                invoice_2.clone(),
                invoice_3.clone(),
                invoice_4.clone(),
                invoice_5.clone(),
            ])
            .await;
        assert_eq!(registry.invoices_count().await, 5);
        assert_eq!(
            registry
                .get_invoice(&invoice_1_id)
                .await,
            Some(invoice_1.clone())
        );
        assert_eq!(
            registry
                .get_invoice(&invoice_2_id)
                .await,
            Some(invoice_2.clone())
        );
        assert_eq!(
            registry
                .get_invoice(&invoice_3_id)
                .await,
            Some(invoice_3.clone())
        );
        assert_eq!(
            registry
                .get_invoice(&invoice_4_id)
                .await,
            Some(invoice_4.clone())
        );
        assert_eq!(
            registry
                .get_invoice(&invoice_5_id)
                .await,
            Some(invoice_5.clone())
        );
        assert!(
            registry
                .get_invoice(&Uuid::new_v4())
                .await
                .is_none()
        );

        // Find invoices by address
        let found_1 = registry
            .find_invoice_by_address("1", ChainType::PolkadotAssetHub, "1")
            .await;
        assert!(found_1.is_some());
        assert_eq!(found_1, Some(invoice_1.clone()));

        let found_2 = registry
            .find_invoice_by_address("2", ChainType::PolkadotAssetHub, "2")
            .await;
        assert!(found_2.is_some());
        assert_eq!(found_2, Some(invoice_2.clone()));

        let found_3 = registry
            .find_invoice_by_address("3", ChainType::Polygon, "3")
            .await;
        assert!(found_3.is_some());
        assert_eq!(found_3, Some(invoice_3.clone()));

        let found_4 = registry
            .find_invoice_by_address("1", ChainType::Polygon, "1")
            .await;
        assert!(found_4.is_some());
        assert_eq!(found_4, Some(invoice_4.clone()));

        let found_5 = registry
            .find_invoice_by_address("1", ChainType::Polygon, "5")
            .await;
        assert!(found_5.is_some());
        assert_eq!(found_5, Some(invoice_5.clone()));

        let expected_used_asset_ids = HashMap::from([
            (
                ChainType::PolkadotAssetHub,
                HashSet::from(["1".to_string(), "2".to_string()]),
            ),
            (
                ChainType::Polygon,
                HashSet::from(["1".to_string(), "3".to_string(), "5".to_string()]),
            ),
        ]);

        let assets_ids = registry.used_asset_ids().await;
        assert_eq!(assets_ids, expected_used_asset_ids);

        // Update filled amounts for each invoice and check it
        let invoice_1_new_amount = invoice_1_amount + Decimal::TEN;
        registry
            .update_filled_amount(&invoice_1_id, invoice_1_new_amount)
            .await;

        let invoice_2_new_amount = invoice_2_amount + Decimal::TEN;
        registry
            .update_filled_amount(&invoice_2_id, invoice_2_new_amount)
            .await;

        let invoice_3_new_amount = invoice_3_amount + Decimal::TEN;
        registry
            .update_filled_amount(&invoice_3_id, invoice_3_new_amount)
            .await;

        let invoice_4_new_amount = invoice_1_amount + Decimal::TEN;
        registry
            .update_filled_amount(&invoice_4_id, invoice_4_new_amount)
            .await;

        let invoice_5_new_amount = invoice_1_amount + Decimal::TEN;
        registry
            .update_filled_amount(&invoice_5_id, invoice_5_new_amount)
            .await;

        let updated_1 = registry
            .get_invoice(&invoice_1_id)
            .await;
        assert!(updated_1.is_some());
        let updated_1_val = updated_1.unwrap();
        assert_eq!(
            invoice_1_new_amount,
            updated_1_val.total_received_amount
        );
        assert_eq!(invoice_1.invoice, updated_1_val.invoice);

        let updated_2 = registry
            .get_invoice(&invoice_2_id)
            .await;
        assert!(updated_2.is_some());
        let updated_2_val = updated_2.unwrap();
        assert_eq!(
            invoice_2_new_amount,
            updated_2_val.total_received_amount
        );
        assert_eq!(invoice_2.invoice, updated_2_val.invoice);

        let updated_3 = registry
            .get_invoice(&invoice_3_id)
            .await;
        assert!(updated_3.is_some());
        let updated_3_val = updated_3.unwrap();
        assert_eq!(
            invoice_3_new_amount,
            updated_3_val.total_received_amount
        );
        assert_eq!(invoice_3.invoice, updated_3_val.invoice);

        let updated_4 = registry
            .get_invoice(&invoice_4_id)
            .await;
        assert!(updated_4.is_some());
        let updated_4_val = updated_4.unwrap();
        assert_eq!(
            invoice_4_new_amount,
            updated_4_val.total_received_amount
        );
        assert_eq!(invoice_4.invoice, updated_4_val.invoice);

        let updated_5 = registry
            .get_invoice(&invoice_5_id)
            .await;
        assert!(updated_5.is_some());
        let updated_5_val = updated_5.unwrap();
        assert_eq!(
            invoice_5_new_amount,
            updated_5_val.total_received_amount
        );
        assert_eq!(invoice_5.invoice, updated_5_val.invoice);

        // Remove single invoice
        registry
            .remove_invoice(&invoice_1_id)
            .await;

        assert_eq!(registry.invoices_count().await, 4);
        assert!(
            registry
                .get_invoice(&invoice_1_id)
                .await
                .is_none()
        );
        assert!(
            registry
                .get_invoice(&invoice_2_id)
                .await
                .is_some()
        );
        assert!(
            registry
                .get_invoice(&invoice_3_id)
                .await
                .is_some()
        );
        assert!(
            registry
                .get_invoice(&invoice_4_id)
                .await
                .is_some()
        );
        assert!(
            registry
                .get_invoice(&invoice_5_id)
                .await
                .is_some()
        );

        // Remove multiple invocies
        registry
            .remove_invoices(&[
                invoice_1_id, // it's already removed, shouldn't affect the others
                invoice_2_id,
                invoice_3_id,
                invoice_4_id,
                Uuid::new_v4(),
            ])
            .await;

        assert_eq!(registry.invoices_count().await, 1);
        assert!(
            registry
                .get_invoice(&invoice_5_id)
                .await
                .is_some()
        );
    }
}
