use std::path::PathBuf;
use std::str::FromStr;

use fedimint_core::config::EmptyGenParams;
use fedimint_core::core::ModuleKind;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::plugin_types_trait_impl_config;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::price::{BitMexOracle, MockOracle, OracleClient};
use crate::stability_core::CollateralRatio;
use crate::{FileOracle, PoolCommonGen};

/// The default epoch length is 24hrs (represented in seconds).
// pub const DEFAULT_EPOCH_LENGTH: u64 = 24 * 60 * 60;
pub const DEFAULT_EPOCH_LENGTH: u64 = 40; // TODO: This is just for testing

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Encodable, Decodable)]
pub enum OracleConfig {
    BitMex,
    Mock(String),
    File(String),
}

impl Default for OracleConfig {
    fn default() -> Self {
        OracleConfig::File("./misc/offline_oracle".to_string())
    }
}

impl OracleConfig {
    pub fn oracle_client(&self) -> Box<dyn OracleClient> {
        match self {
            OracleConfig::BitMex => Box::new(BitMexOracle {}),
            OracleConfig::Mock(url) => Box::new(MockOracle {
                url: reqwest::Url::parse(url).expect("invalid Url"),
            }),
            OracleConfig::File(path) => {
                let path = PathBuf::from_str(path).expect("must be valid path");
                Box::new(FileOracle { path })
            }
        }
    }
}

#[derive(Clone, Debug, Hash, Serialize, Deserialize, Encodable, Decodable)]
pub struct EpochConfig {
    pub start_epoch_at: u64,
    pub epoch_length: u64,
    /// Number of peers that have to agree on price before it's used
    pub price_threshold: u32,
    /// The maximum a provider can charge per epoch in parts per million of
    /// locked principal
    pub max_feerate_ppm: u64,
    /// The ratio of seeker position to provider collateral
    pub collateral_ratio: CollateralRatio,
}

impl EpochConfig {
    pub fn epoch_id_for_time(&self, time: OffsetDateTime) -> u64 {
        if time < self.start_epoch_at() {
            0
        } else {
            (time - self.start_epoch_at()).whole_seconds() as u64 / self.epoch_length + 1
        }
    }

    pub fn start_epoch_at(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(self.start_epoch_at as _)
            .expect("must be valid unix timestamp")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolGenParams {
    pub local: EmptyGenParams,
    pub consensus: PoolGenParamsConsensus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolGenParamsConsensus {
    pub important_param: u64,
    #[serde(default)]
    pub start_epoch_at: Option<time::PrimitiveDateTime>,
    /// this is in seconds
    pub epoch_length: u64,
    pub oracle_config: OracleConfig,
    /// The ratio of seeker position to provider collateral
    #[serde(default)]
    pub collateral_ratio: CollateralRatio,
}

impl Default for PoolGenParamsConsensus {
    fn default() -> Self {
        Self {
            important_param: 3,
            start_epoch_at: None,
            epoch_length: DEFAULT_EPOCH_LENGTH,
            oracle_config: OracleConfig::default(),
            collateral_ratio: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfig {
    pub local: PoolConfigLocal,
    /// Configuration that will be encrypted.
    pub private: PoolConfigPrivate,
    /// Configuration that needs to be the same for every federation member.
    pub consensus: PoolConfigConsensus,
}

#[derive(Clone, Debug, Serialize, Deserialize, Encodable, Decodable)]
pub struct PoolConfigLocal;

#[derive(Clone, Debug, Serialize, Deserialize, Encodable, Decodable)]
pub struct PoolConfigConsensus {
    pub epoch: EpochConfig,
    pub oracle: OracleConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfigPrivate;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encodable, Decodable, Hash)]
pub struct PoolClientConfig {
    pub oracle: OracleConfig,
    pub collateral_ratio: CollateralRatio,
}

impl std::fmt::Display for PoolClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PoolClientConfig {}",
            serde_json::to_string(self).map_err(|_e| std::fmt::Error)?
        )
    }
}

plugin_types_trait_impl_config!(
    PoolCommonGen,
    PoolGenParams,
    EmptyGenParams,
    PoolGenParamsConsensus,
    PoolConfig,
    PoolConfigLocal,
    PoolConfigPrivate,
    PoolConfigConsensus,
    PoolClientConfig
);