use serde::{
    Deserialize,
    Serialize,
};

use strum::{
    Display,
    EnumIter,
    EnumString,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, Display, EnumString,
)]
#[cfg_attr(feature = "sqlx-types", derive(sqlx::Type))]
pub enum ChainType {
    PolkadotAssetHub,
    Polygon,
}
