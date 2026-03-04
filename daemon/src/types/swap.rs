use alloy::primitives::Address;
use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use rust_decimal::Decimal;
use uuid::Uuid;
use sqlx::Type;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum SwapExecutorType {
    Across,
}

impl std::fmt::Display for SwapExecutorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Across => write!(f, "Across")
        }
    }
}

impl std::str::FromStr for SwapExecutorType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Across" => Ok(Self::Across),
            _ => Err("Unknown swap executor type: {s}".to_string())
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
    ZkSync,
    Zora,
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
            "ZkSync" => Ok(Self::ZkSync),
            "Zora" => Ok(Self::Zora),
            _ => Err(format!("Unknown swap chain: {s}"))
        }
    }
}

impl SwapChainType {
    pub fn chain_id(&self) -> u64 {
        1
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
            _ => Err(format!("Unknown swap status: {s}"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSwapData {
    pub invoice_id: Uuid,
    pub swap_executor: SwapExecutorType,
    pub from_chain: SwapChainType,
    pub to_chain: SwapChainType,
    pub from_token_address: String,
    pub to_token_address: String,
    pub from_amount_units: u128,
    pub expected_to_amount_units: u128,
    pub from_address: String,
    pub to_address: String,
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
        from_amount_units: 10_100_000,  // 10.1
        expected_to_amount_units: 10_000_000, // 10
        from_address: "".to_string(),
        to_address: "".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossSwapDetails {
    pub id: String,
    pub raw_transaction: crate::clients::AcrossRawTransaction,
    pub transaction_hash: Option<String>,  // hash of the sent transaction
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InternalSwapDetails {
    Across(AcrossSwapDetails)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    pub id: Uuid,
    pub request: CreateSwapData,
    pub status: SwapStatus,
    pub estimated_to_amount: Decimal,  // calculated by swap executor
    pub swap_details: InternalSwapDetails,
    pub created_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub valid_till: DateTime<Utc>,
    pub error_message: Option<String>,
}

impl Swap {
    #[cfg(test)]
    pub fn trunc_timestamps(&mut self) {
        use chrono::SubsecRound;

        self.created_at = self.created_at.trunc_subsecs(0);
        self.submitted_at = self.submitted_at.map(|dt| dt.trunc_subsecs(0));
        self.finished_at = self.finished_at.map(|dt| dt.trunc_subsecs(0));
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
