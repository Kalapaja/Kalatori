//! Deaf and dumb object definitions

use std::ops::{Deref, Sub};

use serde::Deserialize;

#[expect(dead_code)]
pub type Version = u64;
#[expect(dead_code)]
pub type Nonce = u32;

pub type Entropy = Vec<u8>; // TODO: maybe enforce something here

#[derive(Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Chain {
    pub name: String,
    pub endpoints: Vec<String>,
    #[serde(flatten)]
    pub native_token: Option<NativeToken>,
    #[serde(default)]
    pub asset: Vec<AssetInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NativeToken {
    #[serde(rename = "native-token")]
    pub name: String,
    pub decimals: api_v2::Decimals,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AssetInfo {
    pub name: String,
    pub id: api_v2::AssetId,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Balance(pub u128);

impl Deref for Balance {
    type Target = u128;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Sub for Balance {
    type Output = Self;

    fn sub(self, r: Self) -> Self {
        // TODO: it's better to replace u128 with Decimal
        #[expect(clippy::arithmetic_side_effects)]
        Balance(self.0 - r.0)
    }
}

impl Balance {
    pub fn format(&self, decimals: api_v2::Decimals) -> f64 {
        #[expect(clippy::cast_precision_loss)]
        let float = **self as f64;

        float / decimal_exponent_product(decimals)
    }

    pub fn parse(float: f64, decimals: api_v2::Decimals) -> Self {
        let parsed_float = (float * decimal_exponent_product(decimals)).round();

        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self(parsed_float as _)
    }
}

pub fn decimal_exponent_product(decimals: api_v2::Decimals) -> f64 {
    10f64.powi(decimals.into())
}

/// Self-sufficient schemas used by Api v2.0.0
pub mod api_v2 {
    use std::collections::HashMap;

    use codec::{Decode, Encode};
    use serde::{Deserialize, Serialize, Serializer};

    pub const AMOUNT: &str = "amount";
    pub const CURRENCY: &str = "currency";
    pub const CALLBACK: &str = "callback";
    pub type AssetId = u32;
    pub type Decimals = u8;
    pub type BlockNumber = u32;
    pub type ExtrinsicIndex = u32;

    #[derive(Encode, Decode, Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct Timestamp(pub u64);

    #[derive(Debug, Serialize)]
    pub struct InvalidParameter {
        pub parameter: String,
        pub message: String,
    }

    #[derive(Debug)]
    pub struct OrderQuery {
        pub order: String,
        pub amount: f64,
        pub callback: String,
        pub currency: String,
    }

    #[derive(Debug, Serialize)]
    pub enum OrderResponse {
        NewOrder(OrderStatus),
        FoundOrder(OrderStatus),
        ModifiedOrder(OrderStatus),
        CollidedOrder(OrderStatus),
        NotFound,
    }

    #[derive(Debug, Serialize)]
    pub struct OrderStatus {
        pub order: String,
        pub message: String,
        pub recipient: String,
        pub server_info: ServerInfo,
        #[serde(flatten)]
        pub order_info: OrderInfo,
        pub payment_page: String,
        pub redirect_url: String,
    }

    #[derive(Clone, Debug, Serialize, Encode, Decode)]
    pub struct OrderInfo {
        pub withdrawal_status: WithdrawalStatus,
        pub payment_status: PaymentStatus,
        pub amount: f64,
        pub currency: CurrencyInfo,
        pub callback: String,
        pub transactions: Vec<TransactionInfo>,
        pub payment_account: String,
        pub death: Timestamp,
    }

    impl OrderInfo {
        pub fn new(
            query: OrderQuery,
            currency: CurrencyInfo,
            payment_account: String,
            death: Timestamp,
        ) -> Self {
            OrderInfo {
                withdrawal_status: WithdrawalStatus::Waiting,
                payment_status: PaymentStatus::Pending,
                amount: query.amount,
                currency,
                callback: query.callback,
                transactions: Vec::new(),
                payment_account,
                death,
            }
        }
    }

    pub enum OrderCreateResponse {
        New(OrderInfo),
        Modified(OrderInfo),
        Collision(OrderInfo),
    }

    #[derive(Clone, Debug, Serialize, Decode, Encode, PartialEq)]
    #[serde(rename_all = "lowercase")]
    pub enum PaymentStatus {
        Pending,
        Paid,
    }

    #[derive(Clone, Debug, Serialize, Decode, Encode, PartialEq)]
    #[serde(rename_all = "lowercase")]
    pub enum WithdrawalStatus {
        Waiting,
        Failed,
        Forced,
        Completed,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct ServerStatus {
        pub server_info: ServerInfo,
        pub supported_currencies: HashMap<std::string::String, CurrencyProperties>,
    }

    #[derive(Debug, Serialize)]
    pub struct ServerHealth {
        pub server_info: ServerInfo,
        pub connected_rpcs: Vec<RpcInfo>,
        pub status: Health,
    }

    #[derive(Debug, Serialize, Clone)]
    pub struct RpcInfo {
        pub rpc_url: String,
        pub chain_name: String,
        pub status: Health,
    }

    #[derive(Debug, Serialize, Clone, PartialEq, Copy)]
    #[serde(rename_all = "lowercase")]
    pub enum Health {
        Ok,
        Degraded,
        Critical,
    }

    #[derive(Clone, Debug, Serialize, Decode, Encode)]
    pub struct CurrencyInfo {
        pub currency: String,
        pub chain_name: String,
        pub kind: TokenKind,
        pub decimals: Decimals,
        pub rpc_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub asset_id: Option<AssetId>,
        // #[serde(skip_serializing)]
        pub ss58: u16,
    }

    impl CurrencyInfo {
        #[expect(dead_code)]
        pub fn properties(&self) -> CurrencyProperties {
            CurrencyProperties {
                chain_name: self.chain_name.clone(),
                kind: self.kind,
                decimals: self.decimals,
                rpc_url: self.rpc_url.clone(),
                asset_id: self.asset_id,
                ss58: self.ss58,
            }
        }
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct CurrencyProperties {
        pub chain_name: String,
        pub kind: TokenKind,
        pub decimals: Decimals,
        pub rpc_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub asset_id: Option<AssetId>,
        // #[serde(skip_serializing)]
        pub ss58: u16,
    }

    impl CurrencyProperties {
        pub fn info(&self, currency: String) -> CurrencyInfo {
            CurrencyInfo {
                currency,
                chain_name: self.chain_name.clone(),
                kind: self.kind,
                decimals: self.decimals,
                rpc_url: self.rpc_url.clone(),
                asset_id: self.asset_id,
                ss58: self.ss58,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Serialize, Decode, Encode, Deserialize, PartialEq)]
    #[serde(rename_all = "lowercase")]
    pub enum TokenKind {
        Asset,
        Native,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct ServerInfo {
        pub version: String,
        pub instance_id: String,
        pub debug: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub kalatori_remark: Option<String>,
    }

    #[derive(Clone, Debug, Serialize, Decode, Encode)]
    pub struct TransactionInfo {
        #[serde(skip_serializing_if = "Option::is_none", flatten)]
        pub finalized_tx: Option<FinalizedTx>, // Clearly undefined in v2.1 - TODO
        pub transaction_bytes: String,
        pub sender: String,
        pub recipient: String,
        #[serde(serialize_with = "amount_serializer")]
        pub amount: Amount,
        pub currency: CurrencyInfo,
        pub status: TxStatus,
    }

    #[derive(Clone, Debug, Serialize, Decode, Encode)]
    pub struct FinalizedTx {
        pub block_number: BlockNumber,
        pub position_in_block: ExtrinsicIndex,
        pub timestamp: String,
    }

    // TODO: `Encode` macro generates some code which cast usize to u8 and trigger clippy.
    // It seems to be old issue happened again, https://github.com/paritytech/parity-scale-codec/issues/713
    // Check for updates periodically and remove this module when problem is fixed
    #[expect(clippy::cast_possible_truncation)]
    mod amount {
        use super::{Decode, Encode};

        #[derive(Clone, Debug, Decode, Encode)]
        pub enum Amount {
            All,
            Exact(f64),
        }
    }

    pub use amount::Amount;

    fn amount_serializer<S: Serializer>(amount: &Amount, serializer: S) -> Result<S::Ok, S::Error> {
        match amount {
            Amount::All => serializer.serialize_str("all"),
            Amount::Exact(exact) => exact.serialize(serializer),
        }
    }

    #[derive(Clone, Debug, Serialize, Decode, Encode)]
    #[serde(rename_all = "lowercase")]
    pub enum TxStatus {
        Pending,
        Finalized,
        Failed,
    }
}

#[cfg(test)]
#[test]
#[expect(
    clippy::inconsistent_digit_grouping,
    clippy::unreadable_literal,
    clippy::float_cmp
)]
fn balance_insufficient_precision() {
    const DECIMALS: api_v2::Decimals = 10;

    let float = 931395.862219815_3;
    let parsed = Balance::parse(float, DECIMALS);

    assert_eq!(*parsed, 931395_862219815_2);
    assert_eq!(parsed.format(DECIMALS), 931395.862219815_1);
}
