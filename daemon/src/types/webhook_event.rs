use chrono::{
    DateTime,
    Utc,
};
use uuid::Uuid;

pub use kalatori_client::types::{
    GenericEvent,
    InvoiceEventType,
    KalatoriEventExt,
};

#[derive(Debug, sqlx::FromRow)]
pub struct WebhookEvent {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub payload: serde_json::Value,
    pub sent: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<T: KalatoriEventExt> From<GenericEvent<T>> for WebhookEvent {
    fn from(event: GenericEvent<T>) -> Self {
        let payload =
            serde_json::to_value(&event).expect("Failed to serialize webhook event payload");

        Self {
            id: event.id,
            entity_id: event.payload.entity_id(),
            payload,
            sent: false,
            created_at: event.timestamp,
            updated_at: event.timestamp,
        }
    }
}

// Helper function to create a test invoice event
#[cfg(test)]
pub fn default_webhook_event(invoice_id: Uuid) -> GenericEvent<super::PublicInvoice> {
    let invoice = super::PublicInvoice {
        id: invoice_id,
        order_id: invoice_id.to_string(),
        asset_name: "USDT".to_string(),
        asset_id: "1984".to_string(),
        chain: kalatori_client::types::ChainType::PolkadotAssetHub,
        amount: rust_decimal::Decimal::new(10000, 2),
        payment_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        status: super::InvoiceStatus::Waiting,
        payment_url: "https://app.kalatori.com/invoice/test".to_string(),
        redirect_url: "https://example.com/thank-you".to_string(),
        cart: kalatori_client::types::InvoiceCart {
            items: vec![],
        },
        metadata: None,
        valid_till: Utc::now() + chrono::Duration::hours(24),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        total_received_amount: rust_decimal::Decimal::ZERO,
        transactions: vec![],
    };

    invoice.build_event(InvoiceEventType::Created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_payload_includes_metadata() {
        let mut event = default_webhook_event(Uuid::new_v4());

        // Absent metadata is omitted from the payload entirely
        let webhook_event = WebhookEvent::from(default_webhook_event(Uuid::new_v4()));
        assert!(
            webhook_event.payload["payload"]
                .get("metadata")
                .is_none()
        );

        let metadata = serde_json::json!({"external_ref": "bridge-42"});
        event.payload.metadata = Some(metadata.clone());

        let webhook_event = WebhookEvent::from(event);
        assert_eq!(
            webhook_event.payload["payload"]["metadata"],
            metadata
        );
    }
}
