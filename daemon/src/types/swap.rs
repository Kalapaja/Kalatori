use alloy::primitives::Address;
use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    serde_as,
};
use sqlx::Type;
use uuid::Uuid;

use crate::clients::{
    AcrossQuoteDetails,
    AcrossRawTransaction,
    BungeeQuoteDetails,
    BungeeRawTransaction,
};

use super::ChainType;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateFrontEndSwapParams {
    pub invoice_id: Uuid,
    pub from_amount_units: u128,
    pub from_chain_id: u32,
    pub from_asset_id: Address,
    pub transaction_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontEndSwap {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub from_amount_units: u128,
    pub from_chain_id: u32,
    pub from_asset_id: Address,
    pub transaction_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSwapParams {
    pub invoice_id: Uuid,
    pub from_chain_id: u64,
    pub from_asset_id: String,
    pub from_address: String,
    #[serde_as(as = "DisplayFromStr")]
    pub from_amount_units: u128,
    #[serde(default)]
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub expected_to_amount_units: Option<u128>, // if None, will calculate invoice's unpaid amount
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum SwapExecutorType {
    Across,
    Bungee,
}

impl std::fmt::Display for SwapExecutorType {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            Self::Across => write!(f, "Across"),
            Self::Bungee => write!(f, "Bungee"),
        }
    }
}

impl std::str::FromStr for SwapExecutorType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Across" => Ok(Self::Across),
            "Bungee" => Ok(Self::Bungee),
            _ => Err("Unknown swap executor type: {s}".to_string()),
        }
    }
}

impl SwapExecutorType {
    // Returns None if such direction is unsupported
    pub fn detect(
        from_chain: SwapChainType,
        to_chain: SwapChainType,
        _direction: SwapDirection,
    ) -> Option<SwapExecutorType> {
        if from_chain == to_chain {
            Some(Self::Bungee)
        } else {
            Some(Self::Across)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum SwapChainType {
    Arbitrum,
    Base,
    Blast,
    BnbSmartChain,
    Ethereum,
    HyperEvm,
    Ink,
    Lens,
    Linea,
    Lisk,
    MegaEth,
    Mode,
    Monad,
    Optimism,
    Plasma,
    Polygon,
    Scroll,
    Soneium,
    Solana,
    Unichain,
    WorldChain,
    ZkSync,
    Zora,
}

impl From<ChainType> for SwapChainType {
    fn from(value: ChainType) -> Self {
        match value {
            ChainType::Polygon => SwapChainType::Polygon,
            ChainType::PolkadotAssetHub => {
                unimplemented!("Polkadot Asset Hub can not be used with swaps enabled")
            },
        }
    }
}

impl TryFrom<u64> for SwapChainType {
    type Error = u64;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        use SwapChainType::*;

        match value {
            42161 => Ok(Arbitrum),
            8453 => Ok(Base),
            81457 => Ok(Blast),
            56 => Ok(BnbSmartChain),
            1 => Ok(Ethereum),
            999 => Ok(HyperEvm),
            57073 => Ok(Ink),
            232 => Ok(Lens),
            59144 => Ok(Linea),
            1135 => Ok(Lisk),
            4326 => Ok(MegaEth),
            34443 => Ok(Mode),
            143 => Ok(Monad),
            10 => Ok(Optimism),
            9745 => Ok(Plasma),
            137 => Ok(Polygon),
            534352 => Ok(Scroll),
            1868 => Ok(Soneium),
            // TODO: This is not actual chain id, soloana doesn't have one at all.
            // Need to "wrap" it somehow for Across specifically and also check other
            // chains and ids
            34268394551451 => Ok(Solana),
            130 => Ok(Unichain),
            480 => Ok(WorldChain),
            323 => Ok(ZkSync),
            7777777 => Ok(Zora),
            _ => Err(value),
        }
    }
}

impl std::fmt::Display for SwapChainType {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        use SwapChainType::*;

        match self {
            Arbitrum => write!(f, "Arbitrum"),
            Base => write!(f, "Base"),
            Blast => write!(f, "Blast"),
            BnbSmartChain => write!(f, "BnbSmartChain"),
            Ethereum => write!(f, "Ethereum"),
            HyperEvm => write!(f, "HyperEvm"),
            Ink => write!(f, "Ink"),
            Lens => write!(f, "Lens"),
            Linea => write!(f, "Linea"),
            Lisk => write!(f, "Lisk"),
            MegaEth => write!(f, "MegaEth"),
            Mode => write!(f, "Mode"),
            Monad => write!(f, "Monad"),
            Optimism => write!(f, "Optimism"),
            Plasma => write!(f, "Plasma"),
            Polygon => write!(f, "Polygon"),
            Scroll => write!(f, "Scroll"),
            Soneium => write!(f, "Soneium"),
            Solana => write!(f, "Solana"),
            Unichain => write!(f, "Unichain"),
            WorldChain => write!(f, "WorldChain"),
            ZkSync => write!(f, "ZkSync"),
            Zora => write!(f, "Zora"),
        }
    }
}

impl std::str::FromStr for SwapChainType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Arbitrum" => Ok(Self::Arbitrum),
            "Base" => Ok(Self::Base),
            "Blast" => Ok(Self::Blast),
            "BnbSmartChain" => Ok(Self::BnbSmartChain),
            "Ethereum" => Ok(Self::Ethereum),
            "HyperEvm" => Ok(Self::HyperEvm),
            "Ink" => Ok(Self::Ink),
            "Lens" => Ok(Self::Lens),
            "Linea" => Ok(Self::Linea),
            "Lisk" => Ok(Self::Lisk),
            "MegaEth" => Ok(Self::MegaEth),
            "Mode" => Ok(Self::Mode),
            "Monad" => Ok(Self::Monad),
            "Optimism" => Ok(Self::Optimism),
            "Plasma" => Ok(Self::Plasma),
            "Polygon" => Ok(Self::Polygon),
            "Scroll" => Ok(Self::Scroll),
            "Soneium" => Ok(Self::Soneium),
            "Solana" => Ok(Self::Solana),
            "Unichain" => Ok(Self::Unichain),
            "WorldChain" => Ok(Self::WorldChain),
            "ZkSync" => Ok(Self::ZkSync),
            "Zora" => Ok(Self::Zora),
            _ => Err(format!("Unknown swap chain: {s}")),
        }
    }
}

impl SwapChainType {
    pub fn chain_id(&self) -> u64 {
        use SwapChainType::*;

        match self {
            Arbitrum => 42161,
            Base => 8453,
            Blast => 81457,
            BnbSmartChain => 56,
            Ethereum => 1,
            HyperEvm => 999,
            Ink => 57073,
            Lens => 232,
            Linea => 59144,
            Lisk => 1135,
            MegaEth => 4326,
            Mode => 34443,
            Monad => 143,
            Optimism => 10,
            Plasma => 9745,
            Polygon => 137,
            Scroll => 534352,
            Soneium => 1868,
            Solana => 34268394551451,
            Unichain => 130,
            WorldChain => 480,
            ZkSync => 323,
            Zora => 7777777,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum SwapStatus {
    /// An order has been created but not submitted
    Created,
    /// An order has been submitted
    Submitted,
    /// An order has been created and waiting for execution
    Pending,
    /// An order has been executed successfully
    Completed,
    /// An order has failed/canceled/refunded
    Failed,
    /// Swap has been requested but not approved/sent
    Abandoned,
}

impl std::fmt::Display for SwapStatus {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "Created"),
            Self::Submitted => write!(f, "Submitted"),
            Self::Pending => write!(f, "Pending"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed => write!(f, "Failed"),
            Self::Abandoned => write!(f, "Abandoned"),
        }
    }
}

impl std::str::FromStr for SwapStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Created" => Ok(Self::Created),
            "Submitted" => Ok(Self::Submitted),
            "Pending" => Ok(Self::Pending),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Abandoned" => Ok(Self::Abandoned),
            _ => Err(format!("Unknown swap status: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum SwapDirection {
    Incoming,
    Outgoing,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSwapData {
    pub invoice_id: Uuid,
    pub swap_executor: SwapExecutorType,
    pub from_chain: SwapChainType,
    pub to_chain: SwapChainType,
    pub from_token_address: String,
    pub to_token_address: String,
    #[serde_as(as = "DisplayFromStr")]
    pub from_amount_units: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub expected_to_amount_units: u128,
    pub from_address: String,
    pub to_address: String,
    pub direction: SwapDirection,
}

#[cfg(test)]
pub fn default_create_swap_data(invoice_id: Uuid) -> CreateSwapData {
    CreateSwapData {
        invoice_id,
        swap_executor: SwapExecutorType::Across,
        from_chain: SwapChainType::Base,
        to_chain: SwapChainType::Polygon,
        from_token_address: "".to_string(),
        to_token_address: "".to_string(),
        from_amount_units: 10_100_000,        // 10.1
        expected_to_amount_units: 10_000_000, // 10
        from_address: "".to_string(),
        to_address: "".to_string(),
        direction: SwapDirection::Incoming,
    }
}

#[expect(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InternalQuoteDetails {
    Across(AcrossQuoteDetails),
    Bungee(BungeeQuoteDetails),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapQuote {
    pub swap_executor: SwapExecutorType,
    pub id: String,
    pub estimated_to_amount_units: u128,
    pub estimated_to_amount: Decimal,
    pub valid_till: DateTime<Utc>,
    pub quote_details: InternalQuoteDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossSwapDetails {
    pub id: String,
    pub raw_transaction: AcrossRawTransaction,
    pub transaction_hash: Option<String>, // hash of the sent transaction
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BungeeSwapDetails {
    pub id: String,
    pub raw_transaction: BungeeRawTransaction,
    pub signature: Option<String>,
    pub transaction_hash: Option<String>,
}

#[expect(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InternalSwapDetails {
    Across(AcrossSwapDetails),
    Bungee(BungeeSwapDetails),
}

impl From<SwapQuote> for InternalSwapDetails {
    fn from(value: SwapQuote) -> Self {
        match value.quote_details {
            InternalQuoteDetails::Across(details) => {
                InternalSwapDetails::Across(AcrossSwapDetails {
                    id: value.id,
                    raw_transaction: details,
                    transaction_hash: None,
                })
            },
            InternalQuoteDetails::Bungee(details) => {
                InternalSwapDetails::Bungee(BungeeSwapDetails {
                    id: value.id,
                    raw_transaction: details,
                    signature: None,
                    transaction_hash: None,
                })
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    pub id: Uuid,
    pub request: CreateSwapData,
    pub status: SwapStatus,
    pub estimated_to_amount: Decimal, // calculated by swap executor
    pub swap_details: InternalSwapDetails,
    pub created_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub valid_till: DateTime<Utc>,
    pub error_message: Option<String>,
}

impl Swap {
    pub fn new(
        request: CreateSwapData,
        quote: SwapQuote,
    ) -> Self {
        let valid_till = quote.valid_till;

        Self {
            id: Uuid::new_v4(),
            request,
            status: SwapStatus::Created,
            estimated_to_amount: quote.estimated_to_amount,
            swap_details: quote.into(),
            created_at: Utc::now(),
            submitted_at: None,
            finished_at: None,
            valid_till,
            error_message: None,
        }
    }

    // useful for API functions
    pub fn into_public(self) -> PublicSwap {
        self.into()
    }

    #[cfg(test)]
    pub fn trunc_timestamps(&mut self) {
        use chrono::SubsecRound;

        self.created_at = self.created_at.trunc_subsecs(0);
        self.submitted_at = self
            .submitted_at
            .map(|dt| dt.trunc_subsecs(0));
        self.finished_at = self
            .finished_at
            .map(|dt| dt.trunc_subsecs(0));
    }
}

#[cfg(test)]
pub fn default_swap(invoice_id: Uuid) -> Swap {
    Swap {
        id: Uuid::new_v4(),
        request: default_create_swap_data(invoice_id),
        status: SwapStatus::Created,
        estimated_to_amount: Decimal::new(10, 0),
        swap_details: InternalSwapDetails::Across(AcrossSwapDetails {
            id: "".to_string(),
            raw_transaction: crate::clients::default_across_raw_transaction(),
            transaction_hash: None,
        }),
        created_at: Utc::now(),
        submitted_at: None,
        finished_at: None,
        valid_till: Utc::now(),
        error_message: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSwap {
    pub id: Uuid,
    #[serde(flatten)]
    pub request: CreateSwapData,
    pub from_chain_id: u64,
    pub to_chain_id: u64,
    pub status: SwapStatus,
    pub estimated_to_amount: Decimal, // calculated by swap executor
    pub swap_details: InternalSwapDetails,
    pub created_at: DateTime<Utc>,
    pub valid_till: DateTime<Utc>,
}

impl From<Swap> for PublicSwap {
    fn from(value: Swap) -> Self {
        Self {
            id: value.id,
            from_chain_id: value.request.from_chain.chain_id(),
            to_chain_id: value.request.to_chain.chain_id(),
            request: value.request,
            status: value.status,
            estimated_to_amount: value.estimated_to_amount,
            swap_details: value.swap_details,
            created_at: value.created_at,
            valid_till: value.valid_till,
        }
    }
}

// Some swaps should be submitted on front-end while the others on backend
// (depending on swaps executor). For the swaps which were submitted on
// front-end we'd like to know their transaction hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedSwapParams {
    pub swap_id: Uuid,
    pub swap_executor: SwapExecutorType,
    pub transaction_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapSignatureParams {
    pub swap_id: Uuid,
    pub swap_executor: SwapExecutorType,
    pub signature: String,
}
