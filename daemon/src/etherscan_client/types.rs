use std::fmt;

use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    serde_as,
};
use uuid::Uuid;

use crate::types::{
    ChainType,
    GeneralTransactionId,
    IncomingTransaction,
    TransferInfo,
};
use crate::utils::decimal_from_base_units;

use super::EtherscanClientError;

#[serde_as]
#[expect(dead_code)]
#[derive(Debug, Deserialize)]
pub struct EtherscanResponseData<T> {
    #[serde_as(as = "DisplayFromStr")]
    pub status: u32,
    pub message: String,
    pub result: T,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EtherscanResponse<T> {
    Ok(EtherscanResponseData<T>),
    Err(EtherscanResponseData<String>),
}

#[derive(Serialize)]
pub struct GetAccountTokenTransactionsParams<'a> {
    pub module: &'a str,
    pub action: &'a str,
    pub address: &'a str,
    #[serde(rename = "contractaddress")]
    pub contract_address: &'a str,
    #[serde(rename = "chainid")]
    pub chain_id: u32,
    #[serde(rename = "apikey")]
    pub api_key: &'a str,
}

/// Hand-written so the key cannot reach a log through `?params`. The field has
/// to stay a plain `&str` because `Serialize` puts it in the query string, so
/// the derive is what would leak it.
impl fmt::Debug for GetAccountTokenTransactionsParams<'_> {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.debug_struct("GetAccountTokenTransactionsParams")
            .field("module", &self.module)
            .field("action", &self.action)
            .field("address", &self.address)
            .field(
                "contract_address",
                &self.contract_address,
            )
            .field("chain_id", &self.chain_id)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EtherscanTransaction {
    #[serde_as(as = "DisplayFromStr")]
    pub block_number: u32,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub time_stamp: u64,
    pub hash: String,
    // #[serde(deserialize_with = "deserialize_string_to_u32")]
    // pub nonce: u32,
    // pub block_hash: String,
    pub from: String,
    pub contract_address: String,
    pub to: String,
    #[serde_as(as = "DisplayFromStr")]
    pub value: u128,
    // pub token_name: String,
    pub token_symbol: String,
    #[serde_as(as = "DisplayFromStr")]
    pub token_decimal: u32,
    #[serde_as(as = "DisplayFromStr")]
    pub transaction_index: u32,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub gas: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub gas_price: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub gas_used: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub cumulative_gas_used: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub confirmations: u64,
}

impl EtherscanTransaction {
    /// Convert Etherscan's `value` (base units) plus `tokenDecimal` into a
    /// `Decimal` amount.
    ///
    /// This used to be `Decimal::new(self.value as i64, self.token_decimal)`,
    /// which wrapped a `u128` into an `i64`. Ten units of an 18-decimal token
    /// is 10^19 base units — already past `i64::MAX` — so an ordinary payment
    /// was recorded as a *negative* amount. The conversion is now checked at
    /// every step and cannot panic or wrap.
    fn amount(
        value: u128,
        token_decimal: u32,
    ) -> Option<Decimal> {
        decimal_from_base_units(&value.to_string(), token_decimal)
    }

    pub fn into_incoming_transaction(
        self,
        invoice_id: Uuid,
    ) -> Result<IncomingTransaction, EtherscanClientError> {
        let amount = Self::amount(self.value, self.token_decimal).ok_or(
            EtherscanClientError::UnrepresentableAmount {
                value: self.value,
                decimals: self.token_decimal,
                tx_hash: self.hash.clone(),
                block_number: self.block_number,
                transaction_index: self.transaction_index,
            },
        )?;

        let transfer_info = TransferInfo {
            chain: ChainType::Polygon,
            asset_id: self.contract_address,
            asset_name: self.token_symbol,
            amount,
            source_address: self.from,
            destination_address: self.to,
        };

        let transaction_id = GeneralTransactionId {
            block_number: Some(self.block_number),
            position_in_block: Some(self.transaction_index),
            tx_hash: Some(self.hash),
        };

        Ok(IncomingTransaction {
            id: Uuid::new_v4(),
            invoice_id,
            transfer_info,
            transaction_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use secrecy::SecretString;

    use super::*;
    use crate::configs::EtherscanClientConfig;

    fn transaction(
        value: u128,
        token_decimal: u32,
    ) -> EtherscanTransaction {
        EtherscanTransaction {
            block_number: 1,
            hash: "0xdeadbeef".to_string(),
            from: "0xfrom".to_string(),
            contract_address: "0xcontract".to_string(),
            to: "0xto".to_string(),
            value,
            token_symbol: "USDC".to_string(),
            token_decimal,
            transaction_index: 0,
        }
    }

    #[test]
    fn six_decimal_amount_round_trips() {
        // 1.5 USDC
        let tx = transaction(1_500_000, 6);
        let incoming = tx
            .into_incoming_transaction(Uuid::nil())
            .unwrap();

        assert_eq!(
            incoming.transfer_info.amount,
            Decimal::new(15, 1)
        );
    }

    #[test]
    fn ten_units_of_an_18_decimal_token_is_positive_ten() {
        // Regression: 10 * 10^18 = 10^19 base units overflows i64, and the old
        // `self.value as i64` cast turned this into a NEGATIVE amount.
        let tx = transaction(10_000_000_000_000_000_000, 18);
        let incoming = tx
            .into_incoming_transaction(Uuid::nil())
            .unwrap();

        assert_eq!(
            incoming.transfer_info.amount,
            Decimal::new(10, 0)
        );
        assert!(incoming.transfer_info.amount > Decimal::ZERO);
    }

    #[test]
    fn amount_beyond_i64_but_within_decimal_is_exact() {
        // ~18.45 tokens with 18 decimals: base units are just over i64::MAX.
        let value = u128::try_from(i64::MAX).unwrap() + 1;
        let amount = EtherscanTransaction::amount(value, 18).unwrap();

        assert_eq!(
            amount,
            Decimal::from_str_exact("9.223372036854775808").unwrap()
        );
    }

    #[test]
    fn unrepresentable_amount_is_an_error_not_a_wrap() {
        // u128::MAX has 39 significant digits; Decimal holds at most 28.
        let err = transaction(u128::MAX, 18)
            .into_incoming_transaction(Uuid::nil())
            .unwrap_err();

        assert!(matches!(
            err,
            EtherscanClientError::UnrepresentableAmount { .. }
        ));
    }

    #[test]
    fn scaled_value_with_large_base_unit_mantissa_is_exact() {
        let incoming = transaction(
            1_000_000_000_000_000_000_000_000_000_000,
            18,
        )
        .into_incoming_transaction(Uuid::nil())
        .unwrap();

        assert_eq!(
            incoming.transfer_info.amount,
            Decimal::from(1_000_000_000_000_u64)
        );
    }

    #[test]
    fn scale_above_decimal_maximum_is_an_error_not_a_panic() {
        // `Decimal::new(1, 29)` panics; a bogus `tokenDecimal` must not crash
        // the poller.
        let err = transaction(1, 29)
            .into_incoming_transaction(Uuid::nil())
            .unwrap_err();

        assert!(matches!(
            err,
            EtherscanClientError::UnrepresentableAmount { .. }
        ));
    }

    /// A value that cannot occur by accident, so its absence is evidence.
    const SENTINEL: &str = "SENTINEL-ETHERSCAN-KEY-8f3a1c";

    #[test]
    fn debug_of_request_params_hides_the_api_key() {
        let params = GetAccountTokenTransactionsParams {
            module: "account",
            action: "tokentx",
            address: "0x0000000000000000000000000000000000000000",
            contract_address: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
            chain_id: 137,
            api_key: SENTINEL,
        };

        let rendered = format!("{params:?}");

        assert!(
            !rendered.contains(SENTINEL),
            "api key leaked: {rendered}"
        );
        // The struct is still worth logging -- redaction, not omission.
        assert!(rendered.contains("tokentx"));
    }

    #[test]
    fn debug_of_client_config_hides_the_api_key() {
        let config = EtherscanClientConfig {
            requests_per_second: NonZeroU32::new(3).unwrap(),
            api_key: SecretString::from(SENTINEL.to_string()),
        };

        let rendered = format!("{config:?}");

        assert!(
            !rendered.contains(SENTINEL),
            "api key leaked: {rendered}"
        );
    }
}
