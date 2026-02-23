use std::net::{
    IpAddr,
    Ipv4Addr,
};
use std::num::NonZeroU32;

use kalatori_client::types::ChainType;

pub const DEFAULT_CONFIG_DIR_PATH: &str = "configs";

pub const DEFAULT_POLKADOT_ASSET_HUB_ENDPOINTS: &[&str] = &[
    "wss://asset-hub-polkadot-rpc.n.dwellir.com",
    "wss://polkadot-asset-hub-rpc.polkadot.io",
];

pub const DEFAULT_POLYGON_ENDPOINTS: &[&str] = &[
    "wss://polygon-bor-rpc.publicnode.com",
    "wss://polygon.drpc.org",
];

/// Native USDC on Polygon PoS (Circle's official deployment)
pub const DEFAULT_POLYGON_USDC_ADDRESS: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";

pub const DEFAULT_INVOICE_LIFETIME_MILLIS: u64 = 86_400_000; // 24 hours

pub const DEFAULT_ALLOW_INSECURE_ENDPOINTS: bool = false;

pub const DEFAULT_CHAIN: ChainType = ChainType::Polygon;

pub const DEFAULT_ASSET_HUB_ASSET_ID: &str = "1337";

pub const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

pub const DEFAULT_PORT: u16 = 8080;

pub const DEFAULT_DATABASE_DIR: &str = "./database";

pub const DEFAULT_SIGNATURE_MAX_AGE_SECS: u64 = 300; // 5 minutes

pub const DEFAULT_LOG_DIRECTIVES: &str = "kalatori=trace,info";

// Default limit for free account
pub const DEFAULT_ETHERSCAN_LIMIT_PER_SECOND: NonZeroU32 = NonZeroU32::new(3).unwrap();
