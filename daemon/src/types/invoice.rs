use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use sqlx::FromRow;
use sqlx::types::{
    Json,
    Text,
};
use uuid::Uuid;

use crate::utils::url_validation::ValidatedUrl;

use super::ChainType;

// Re-export types from kalatori_client for consistency
pub use kalatori_client::types::{
    Invoice as PublicInvoice,
    InvoiceCart as PublicInvoiceCart,
    InvoiceCartItem as PublicInvoiceCartItem,
    InvoiceStatus,
};

// TODO: The differences between `Invoice` and `PublicInvoice` (from
// kalatori_client crate) are:
// - `Invoice`` doesn't have `payment_url` field
// - `Invoice` stores only validated urls
// Shall we unify?

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: Uuid,
    // Merchant-provided order ID
    pub order_id: String,
    pub asset_id: String,
    pub asset_name: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub payment_address: String,
    pub status: InvoiceStatus,
    pub cart: InvoiceCart,
    pub redirect_url: ValidatedUrl,
    pub valid_till: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Invoice {
    pub fn with_amount(
        self,
        total_received_amount: Decimal,
    ) -> InvoiceWithReceivedAmount {
        InvoiceWithReceivedAmount {
            invoice: self,
            total_received_amount,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceWithReceivedAmount {
    pub invoice: Invoice,
    pub total_received_amount: Decimal,
}

impl InvoiceWithReceivedAmount {
    pub fn into_public_invoice(
        self,
        payment_url_base: &str,
    ) -> PublicInvoice {
        PublicInvoice {
            id: self.invoice.id,
            order_id: self.invoice.order_id,
            asset_id: self.invoice.asset_id,
            asset_name: self.invoice.asset_name,
            chain: self.invoice.chain,
            amount: self.invoice.amount,
            payment_address: self.invoice.payment_address,
            status: self.invoice.status,
            payment_url: format!(
                "{}/public?invoice_id={}",
                payment_url_base.trim_end_matches('/'),
                self.invoice.id
            ),
            redirect_url: self
                .invoice
                .redirect_url
                .into_inner()
                .to_string(),
            cart: self.invoice.cart.into(),
            valid_till: self.invoice.valid_till,
            created_at: self.invoice.created_at,
            updated_at: self.invoice.updated_at,
            total_received_amount: self.total_received_amount,
            transactions: vec![],
        }
    }
}

#[derive(FromRow)]
pub struct InvoiceRow {
    pub id: Uuid,
    pub order_id: String,
    pub asset_id: String,
    pub asset_name: String,
    pub chain: ChainType,
    pub amount: Text<Decimal>,
    pub payment_address: String,
    pub status: InvoiceStatus,
    pub cart: Json<InvoiceCart>,
    pub redirect_url: ValidatedUrl,
    pub valid_till: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<InvoiceRow> for Invoice {
    fn from(row: InvoiceRow) -> Self {
        Self {
            id: row.id,
            order_id: row.order_id,
            asset_id: row.asset_id,
            asset_name: row.asset_name,
            chain: row.chain,
            amount: row.amount.into_inner(),
            payment_address: row.payment_address,
            status: row.status,
            cart: row.cart.0,
            redirect_url: row.redirect_url,
            valid_till: row.valid_till,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateInvoiceData {
    pub id: Uuid,
    pub order_id: String,
    pub asset_id: String,
    pub asset_name: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub payment_address: String,
    pub cart: InvoiceCart,
    pub redirect_url: ValidatedUrl,
    pub valid_till: DateTime<Utc>,
}

impl From<CreateInvoiceData> for Invoice {
    fn from(data: CreateInvoiceData) -> Self {
        let now = Utc::now();

        Self {
            id: data.id,
            order_id: data.order_id,
            asset_id: data.asset_id,
            asset_name: data.asset_name,
            chain: data.chain,
            amount: data.amount,
            payment_address: data.payment_address,
            status: InvoiceStatus::Waiting,
            cart: data.cart,
            redirect_url: data.redirect_url,
            valid_till: data.valid_till,
            created_at: now,
            updated_at: now,
        }
    }
}

// TODO [sab]: docs
#[derive(Debug)]
pub struct UpdateInvoiceData {
    pub invoice_id: Uuid, // Invoice ID to update
    pub amount: Decimal,
    pub cart: InvoiceCart,
    pub valid_till: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCart {
    pub items: Vec<InvoiceCartItem>,
}

impl InvoiceCart {
    pub fn empty() -> Self {
        Self {
            items: vec![],
        }
    }
}

impl From<InvoiceCart> for PublicInvoiceCart {
    fn from(cart: InvoiceCart) -> Self {
        Self {
            items: cart
                .items
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCartItem {
    pub name: String,
    pub quantity: u32,
    pub price: Decimal, // Price per single item
    pub product_url: Option<ValidatedUrl>,
    pub image_url: Option<ValidatedUrl>,
    pub tax: Option<Decimal>,
    pub discount: Option<Decimal>,
}

impl From<InvoiceCartItem> for PublicInvoiceCartItem {
    fn from(item: InvoiceCartItem) -> Self {
        Self {
            name: item.name,
            quantity: item.quantity,
            price: item.price,
            product_url: item
                .product_url
                .map(|url| url.into_inner().to_string()),
            image_url: item
                .image_url
                .map(|url| url.into_inner().to_string()),
            tax: item.tax,
            discount: item.discount,
        }
    }
}

#[cfg(test)]
pub fn default_invoice() -> Invoice {
    default_create_invoice_data().into()
}

#[cfg(test)]
pub fn default_create_invoice_data() -> CreateInvoiceData {
    let now = Utc::now();
    let id = Uuid::new_v4();

    CreateInvoiceData {
        id,
        order_id: id.to_string(),
        asset_id: 1984.to_string(),
        asset_name: "USDT".to_string(),
        chain: ChainType::PolkadotAssetHub,
        amount: Decimal::new(10000, 2),
        payment_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        cart: InvoiceCart::empty(),
        redirect_url: ValidatedUrl::new_unchecked("http://localhost:8080/thankyou"),
        #[expect(clippy::arithmetic_side_effects)]
        valid_till: now + chrono::Duration::hours(24),
    }
}

#[cfg(test)]
pub fn default_update_invoice_data(invoice_id: Uuid) -> UpdateInvoiceData {
    let now = Utc::now();

    UpdateInvoiceData {
        invoice_id,
        amount: Decimal::new(15000, 2),
        cart: InvoiceCart::empty(),
        #[expect(clippy::arithmetic_side_effects)]
        valid_till: now + chrono::Duration::hours(24),
    }
}
