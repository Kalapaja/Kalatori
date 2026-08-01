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

use super::ChainType;

// Re-export types from kalatori_client for consistency
#[cfg_attr(not(test), expect(unused_imports))]
pub use kalatori_client::types::{
    Invoice as PublicInvoice,
    InvoiceCart,
    InvoiceCartItem,
    InvoiceStatus,
};

// TODO: the main difference between Invoice and PublicInvoice (from
// kalatori_client crate) is that Invoice doesn't have `payment_url` field. Need
// to think how we can unify these types and make Invoice a subset of
// PublicInvoice.
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
    pub redirect_url: String,
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
            redirect_url: self.invoice.redirect_url,
            cart: self.invoice.cart,
            valid_till: self.invoice.valid_till,
            created_at: self.invoice.created_at,
            updated_at: self.invoice.updated_at,
            total_received_amount: self.total_received_amount,
            transactions: vec![],
        }
    }

    /// Returns invoice's unfilled amount or 0 if it's filled or overpaid.
    ///
    /// Uses `saturating_sub`: `Decimal`'s `-` panics on overflow, and this is
    /// called from request handlers. Saturating is safe here because the result
    /// is clamped to the non-negative range anyway — an underflow can only push
    /// it further below zero, and an overflow towards `Decimal::MAX` preserves
    /// the "still owed" meaning rather than inventing a zero balance.
    pub fn unfilled_amount(&self) -> Decimal {
        self.invoice
            .amount
            .saturating_sub(self.total_received_amount)
            .max(Decimal::ZERO)
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
    pub redirect_url: String,
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
    pub redirect_url: String,
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

#[derive(Debug)]
pub struct UpdateInvoiceData {
    pub invoice_id: Uuid, // Invoice ID to update
    pub amount: Decimal,
    pub cart: InvoiceCart,
    pub valid_till: DateTime<Utc>,
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
        asset_id: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(),
        asset_name: "USDC".to_string(),
        chain: ChainType::Polygon,
        amount: Decimal::new(10000, 2),
        payment_address: "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7".to_string(),
        cart: InvoiceCart::empty(),
        redirect_url: "http://localhost:8080/thankyou".to_string(),
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "test fixture: `Utc::now()` plus a fixed 24 hours cannot overflow DateTime<Utc>"
        )]
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
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "test fixture: `Utc::now()` plus a fixed 24 hours cannot overflow DateTime<Utc>"
        )]
        valid_till: now + chrono::Duration::hours(24),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_received(
        amount: Decimal,
        total_received_amount: Decimal,
    ) -> InvoiceWithReceivedAmount {
        let mut invoice = default_invoice();
        invoice.amount = amount;

        InvoiceWithReceivedAmount {
            invoice,
            total_received_amount,
        }
    }

    #[test]
    fn unfilled_amount_reports_the_remainder() {
        assert_eq!(
            with_received(
                Decimal::new(100, 0),
                Decimal::new(40, 0)
            )
            .unfilled_amount(),
            Decimal::new(60, 0)
        );
    }

    #[test]
    fn unfilled_amount_is_zero_when_filled_or_overpaid() {
        assert_eq!(
            with_received(
                Decimal::new(100, 0),
                Decimal::new(100, 0)
            )
            .unfilled_amount(),
            Decimal::ZERO
        );
        assert_eq!(
            with_received(
                Decimal::new(100, 0),
                Decimal::new(250, 0)
            )
            .unfilled_amount(),
            Decimal::ZERO
        );
    }

    #[test]
    fn unfilled_amount_saturates_instead_of_panicking() {
        // `Decimal::MAX - Decimal::MIN` overflows; the plain `-` this used to
        // use would panic inside a request handler.
        assert_eq!(
            with_received(Decimal::MAX, Decimal::MIN).unfilled_amount(),
            Decimal::MAX
        );

        // The opposite direction saturates negative and is then clamped to 0.
        assert_eq!(
            with_received(Decimal::MIN, Decimal::MAX).unfilled_amount(),
            Decimal::ZERO
        );
    }
}
