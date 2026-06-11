use std::fmt::Display;

use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use uuid::Uuid;

use super::InvoiceCart;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub category: String,
    pub code: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl Display for ApiError {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(
            f,
            "{} ({}): {}",
            self.code, self.category, self.message
        )
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApiResultStructured<T> {
    Ok { result: T },
    Err { error: ApiError },
}

pub type ApiResult<T> = Result<T, ApiError>;

impl<T> From<ApiResultStructured<T>> for ApiResult<T> {
    fn from(value: ApiResultStructured<T>) -> Self {
        match value {
            ApiResultStructured::Ok {
                result,
            } => Ok(result),
            ApiResultStructured::Err {
                error,
            } => Err(error),
        }
    }
}

fn default_include_transactions() -> bool {
    false
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInvoiceParams {
    pub order_id: String,
    pub amount: Decimal,
    #[serde(default = "InvoiceCart::empty")]
    #[serde(skip_serializing_if = "InvoiceCart::is_empty")]
    pub cart: InvoiceCart,
    /// Opaque metadata stored with the invoice and echoed back verbatim in
    /// API responses and webhook payloads
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub redirect_url: String,
    #[serde(default = "default_include_transactions")]
    pub include_transactions: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetInvoiceParams {
    pub invoice_id: Uuid,
    #[serde(default = "default_include_transactions")]
    pub include_transactions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInvoiceParams {
    pub invoice_id: Uuid,
    pub amount: Decimal,
    #[serde(default = "InvoiceCart::empty")]
    #[serde(skip_serializing_if = "InvoiceCart::is_empty")]
    pub cart: InvoiceCart,
    /// Replaces the stored metadata when provided. Unlike `cart`, metadata is
    /// sticky: omitting the field keeps the previously stored value. To clear
    /// it, send an empty object `{}`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(default = "default_include_transactions")]
    pub include_transactions: bool,
}

pub type CancelInvoiceParams = GetInvoiceParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventEntity {
    Invoice,
    // Refund,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceEventType {
    Created,
    Updated,
    AdminCanceled,
    CustomerCanceled,
    Paid,
    PartiallyPaid,
    Expired,
}

pub trait KalatoriEventExt: Serialize + Sized {
    type EventType: Serialize + for<'de> Deserialize<'de> + Copy + Eq + std::fmt::Debug;

    const ENTITY: EventEntity;

    fn build_event(
        self,
        event_type: Self::EventType,
    ) -> GenericEvent<Self> {
        GenericEvent {
            id: Uuid::new_v4(),
            event_entity: Self::ENTITY,
            event_type,
            payload: self,
            timestamp: Utc::now(),
        }
    }

    fn entity_id(&self) -> Uuid;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GenericEvent<T: KalatoriEventExt> {
    pub id: Uuid,
    pub event_entity: EventEntity,
    pub event_type: T::EventType,
    pub payload: T,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KalatoriEvent {
    Invoice(GenericEvent<super::Invoice>),
    // Refund(GenericEvent<Refund>),
}

impl KalatoriEventExt for super::Invoice {
    type EventType = InvoiceEventType;

    const ENTITY: EventEntity = EventEntity::Invoice;

    fn entity_id(&self) -> Uuid {
        self.id
    }
}
