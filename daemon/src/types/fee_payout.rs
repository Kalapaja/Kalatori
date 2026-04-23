use alloy::primitives::Address;
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};

use crate::fee_client::{
    FeeDecision,
    FeeSource,
};

/// Fee applied to a completed payout, stored as columns on `payouts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeePayout {
    pub fee_wallet: Address,
    pub fee_bps: u16,
    pub source: FeeSource,
    pub amount: Decimal,
}

impl FeePayout {
    pub fn from_decision(decision: &FeeDecision, amount: Decimal) -> Self {
        Self {
            fee_wallet: decision.fee_wallet,
            fee_bps: decision.fee_bps,
            source: decision.source,
            amount,
        }
    }
}
