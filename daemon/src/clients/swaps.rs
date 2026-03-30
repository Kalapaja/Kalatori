mod across;
mod bungee;
mod zeroex;

use serde::de::DeserializeOwned;
use serde::{
    Deserialize,
    Serialize,
};

use kalatori_client::types::ChainType;

use crate::clients::swaps::bungee::BungeeRawTransaction;
use crate::types::{
    CreateSwapData,
    SwapChainType,
    SwapDetails,
    SwapExecutorType,
    SwapQuote,
};

#[cfg(test)]
pub use across::default_across_raw_transaction;
pub use across::{
    AcrossClient,
    AcrossRawTransaction,
};
pub use bungee::BungeeClient;
#[cfg(test)]
pub use zeroex::default_zero_ex_raw_transaction;
pub use zeroex::{
    ZeroExClient,
    ZeroExRawTransaction,
};

pub type TransactionHash = String;

// TODO: use Boxes here?
#[expect(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawSwapDetails {
    Across(AcrossRawTransaction),
    Bungee(BungeeRawTransaction),
    ZeroEx(ZeroExRawTransaction),
}

// TODO: there is also SwapStatus enum in types which identifies status
// of internal database representation of the swap. One should be renamed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutorSwapStatus {
    Pending,
    Executed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapsClientError {
    DirectionIsNotSupported {
        from_chain: SwapChainType,
        to_chain: SwapChainType,
    },
    ChainIsNotSupported {
        chain: SwapChainType,
    },
    SignatureIsNotSet,
    WrongRawTransaction,
    TransactionHashIsNotSet,
    UnknownApiError,
}

impl From<reqwest::Error> for SwapsClientError {
    fn from(_value: reqwest::Error) -> Self {
        Self::UnknownApiError
    }
}

#[expect(dead_code)]
pub trait SwapsClient {
    const EXECUTOR: SwapExecutorType;
    const SUPPORTED_CHAINS: &[ChainType];
    const SUPPORTED_SWAP_CHAINS: &[SwapChainType];
    const SINGLE_CHAIN_SUPPORTED: bool;
    const CROSS_CHAIN_SUPPORTED: bool;
    const GASLESS: bool;

    type GetQuoteParams: From<CreateSwapData>;
    type GetQuoteResponse: Into<SwapQuote>;
    type RawTransactionDetails: TryFrom<RawSwapDetails, Error = SwapsClientError>
        + Serialize
        + DeserializeOwned;
    type SwapStatus: Into<ExecutorSwapStatus>;

    async fn get_quote_internal(
        &self,
        params: Self::GetQuoteParams,
    ) -> Result<Self::GetQuoteResponse, SwapsClientError>;

    async fn get_quote(
        &self,
        data: CreateSwapData,
    ) -> Result<SwapQuote, SwapsClientError> {
        if data.from_chain == data.to_chain && !Self::SINGLE_CHAIN_SUPPORTED {
            return Err(
                SwapsClientError::DirectionIsNotSupported {
                    from_chain: data.from_chain,
                    to_chain: data.to_chain,
                },
            )
        }

        if data.from_chain != data.to_chain && !Self::CROSS_CHAIN_SUPPORTED {
            return Err(
                SwapsClientError::DirectionIsNotSupported {
                    from_chain: data.from_chain,
                    to_chain: data.to_chain,
                },
            )
        }

        if !Self::SUPPORTED_SWAP_CHAINS.contains(&data.from_chain) {
            return Err(SwapsClientError::ChainIsNotSupported {
                chain: data.from_chain,
            })
        }

        if !Self::SUPPORTED_SWAP_CHAINS.contains(&data.to_chain) {
            return Err(SwapsClientError::ChainIsNotSupported {
                chain: data.to_chain,
            })
        }

        let result = self
            .get_quote_internal(data.into())
            .await?;

        Ok(result.into())
    }

    fn extract_signature<'a>(
        &self,
        data: &'a SwapDetails,
    ) -> Result<&'a String, SwapsClientError> {
        data.signature
            .as_ref()
            .ok_or(SwapsClientError::SignatureIsNotSet)
    }

    async fn submit_transaction_internal(
        &self,
        params: &SwapDetails,
    ) -> Result<TransactionHash, SwapsClientError>;

    async fn submit_transaction(
        &self,
        data: &SwapDetails,
    ) -> Result<TransactionHash, SwapsClientError> {
        let result = self
            .submit_transaction_internal(data)
            .await?;

        Ok(result)
    }

    fn extract_transaction_hash<'a>(
        &self,
        data: &'a SwapDetails,
    ) -> Result<&'a TransactionHash, SwapsClientError> {
        data.transaction_hash
            .as_ref()
            .ok_or(SwapsClientError::TransactionHashIsNotSet)
    }

    async fn get_transaction_status_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<Self::SwapStatus, SwapsClientError>;

    async fn get_transaction_status(
        &self,
        data: &SwapDetails,
    ) -> Result<ExecutorSwapStatus, SwapsClientError> {
        let result = self
            .get_transaction_status_internal(data)
            .await?;

        Ok(result.into())
    }
}
