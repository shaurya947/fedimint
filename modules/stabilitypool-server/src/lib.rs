pub mod api;

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use common::action::{self, ActionProposedDb};
use common::config::{
    EpochConfig, PoolClientConfig, PoolConfig, PoolConfigConsensus, PoolConfigLocal,
    PoolConfigPrivate, PoolGenParams,
};
use common::db::AccountBalanceKeyPrefix;
use common::{
    db, epoch, BackOff, OracleClient, PoolCommonGen, PoolConsensusItem, PoolInput, PoolModuleTypes,
    PoolOutput, PoolOutputOutcome,
};
use fedimint_core::config::{
    ConfigGenModuleParams, DkgResult, ServerModuleConfig, ServerModuleConsensusConfig,
    TypedServerModuleConfig, TypedServerModuleConsensusConfig,
};
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::db::{Database, DatabaseVersion, ModuleDatabaseTransaction};
use fedimint_core::module::audit::Audit;
use fedimint_core::module::{
    ApiEndpoint, ConsensusProposal, CoreConsensusVersion, ExtendsCommonModuleInit, InputMeta,
    IntoModuleError, ModuleConsensusVersion, ModuleError, PeerHandle, ServerModuleInit,
    SupportedModuleApiVersions, TransactionItemAmount,
};
use fedimint_core::server::DynServerModule;
use fedimint_core::task::TaskGroup;
use fedimint_core::{NumPeers, OutPoint, PeerId, ServerModule};
pub use stabilitypool_common as common;

// The default global max feerate.
// TODO: Have this actually in config.
pub const DEFAULT_GLOBAL_MAX_FEERATE: u64 = 100_000;

#[derive(Debug, Clone)]
pub struct PoolConfigGenerator;

impl ExtendsCommonModuleInit for PoolConfigGenerator {
    type Common = PoolCommonGen;
}

#[async_trait]
impl ServerModuleInit for PoolConfigGenerator {
    type Params = PoolGenParams;
    const DATABASE_VERSION: DatabaseVersion = DatabaseVersion(1);

    fn versions(&self, _core: CoreConsensusVersion) -> &[ModuleConsensusVersion] {
        &[ModuleConsensusVersion(0)]
    }

    fn supported_api_versions(&self) -> SupportedModuleApiVersions {
        SupportedModuleApiVersions::from_raw(1, 0, &[(0, 0)])
    }

    async fn init(
        &self,
        cfg: ServerModuleConfig,
        _db: Database,
        _task_group: &mut TaskGroup,
    ) -> anyhow::Result<DynServerModule> {
        Ok(StabilityPool::new(cfg.to_typed()?).into())
    }

    fn trusted_dealer_gen(
        &self,
        peers: &[PeerId],
        params: &ConfigGenModuleParams,
    ) -> BTreeMap<PeerId, ServerModuleConfig> {
        let params = params
            .to_typed::<PoolGenParams>()
            .expect("Invalid mint params");

        let mint_cfg: BTreeMap<_, PoolConfig> = peers
            .iter()
            .map(|&peer| {
                let config = PoolConfig {
                    local: PoolConfigLocal,
                    private: PoolConfigPrivate,
                    consensus: PoolConfigConsensus {
                        epoch: EpochConfig {
                            start_epoch_at: params
                                .consensus
                                .start_epoch_at
                                .map(|prim_datetime| prim_datetime.assume_utc())
                                .unwrap_or_else(time::OffsetDateTime::now_utc)
                                .unix_timestamp() as _,
                            epoch_length: params.consensus.epoch_length,
                            price_threshold: peers.threshold() as _,
                            max_feerate_ppm: DEFAULT_GLOBAL_MAX_FEERATE,
                            collateral_ratio: params.consensus.collateral_ratio,
                        },
                        oracle: params.consensus.oracle_config.clone(),
                    },
                };
                (peer, config)
            })
            .collect();

        mint_cfg
            .into_iter()
            .map(|(k, v)| (k, v.to_erased()))
            .collect()
    }

    async fn distributed_gen(
        &self,
        peers: &PeerHandle,
        params: &ConfigGenModuleParams,
    ) -> DkgResult<ServerModuleConfig> {
        let params = params
            .to_typed::<PoolGenParams>()
            .expect("Invalid mint params");

        let server = PoolConfig {
            local: PoolConfigLocal,
            private: PoolConfigPrivate,
            consensus: PoolConfigConsensus {
                epoch: EpochConfig {
                    start_epoch_at: params
                        .consensus
                        .start_epoch_at
                        .map(|prim_datetime| prim_datetime.assume_utc())
                        .unwrap_or_else(time::OffsetDateTime::now_utc)
                        .unix_timestamp() as _,
                    epoch_length: params.consensus.epoch_length,
                    price_threshold: peers.peers.threshold() as _,
                    max_feerate_ppm: DEFAULT_GLOBAL_MAX_FEERATE,
                    collateral_ratio: params.consensus.collateral_ratio,
                },
                oracle: params.consensus.oracle_config,
            },
        };

        Ok(server.to_erased())
    }

    fn validate_config(
        &self,
        _identity: &PeerId,
        config: ServerModuleConfig,
    ) -> anyhow::Result<()> {
        let _ = config.to_typed::<PoolConfig>()?;
        Ok(())
    }

    fn get_client_config(
        &self,
        config: &ServerModuleConsensusConfig,
    ) -> anyhow::Result<PoolClientConfig> {
        let config = PoolConfigConsensus::from_erased(config)?;
        Ok(PoolClientConfig {
            oracle: config.oracle,
            collateral_ratio: config.epoch.collateral_ratio,
        })
    }

    async fn dump_database(
        &self,
        _dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        _prefix_names: Vec<String>,
    ) -> Box<dyn Iterator<Item = (String, Box<dyn erased_serde::Serialize + Send>)> + '_> {
        Box::new(BTreeMap::new().into_iter())
    }
}

#[derive(Debug)]
pub struct StabilityPool {
    pub cfg: PoolConfig,
    pub oracle: Box<dyn OracleClient>,
    pub backoff: BackOff,
    pub proposed_db: ActionProposedDb,
}

#[derive(Debug, Clone)]
pub struct PoolVerificationCache;

impl fedimint_core::server::VerificationCache for PoolVerificationCache {}

impl StabilityPool {
    fn epoch_config(&self) -> &EpochConfig {
        &self.cfg.consensus.epoch
    }

    fn oracle(&self) -> &dyn OracleClient {
        &*self.oracle
    }
}

#[async_trait]
impl ServerModule for StabilityPool {
    type Gen = PoolConfigGenerator;
    type Common = PoolModuleTypes;
    type VerificationCache = PoolVerificationCache;

    // fn versions(&self) -> (ModuleConsensusVersion, &[ApiVersion]) {
    //     (
    //         ModuleConsensusVersion(1),
    //         &[ApiVersion { major: 1, minor: 1 }],
    //     )
    // }

    async fn await_consensus_proposal(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
    ) {
        // This method is `select_all`ed on across all modules.
        // We block until at least one of these happens:
        // * At least one proposed action is available
        // * Duration past requires us to send `PoolConsensusItem::EpochEnd`
        loop {
            if action::can_propose(dbtx, &self.proposed_db).await {
                tracing::info!("can propose: action");
                return;
            }
            if epoch::can_propose(dbtx, &self.backoff, self.epoch_config()).await {
                tracing::info!("can propose: epoch");
                return;
            }

            #[cfg(not(target_family = "wasm"))]
            fedimint_core::task::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn consensus_proposal(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
    ) -> ConsensusProposal<PoolConsensusItem> {
        let mut items = Vec::new();

        items.append(
            &mut epoch::consensus_proposal(dbtx, &self.backoff, self.epoch_config(), self.oracle())
                .await,
        );
        items.append(&mut action::consensus_proposal(dbtx, &self.proposed_db).await);
        ConsensusProposal::Contribute(items)
    }

    async fn process_consensus_item<'a, 'b>(
        &'a self,
        dbtx: &mut ModuleDatabaseTransaction<'b>,
        consensus_item: PoolConsensusItem,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        match consensus_item {
            PoolConsensusItem::ActionProposed(action_proposed) => {
                action::process_consensus_item(
                    dbtx,
                    &self.proposed_db,
                    peer_id,
                    action_proposed,
                    self.epoch_config().price_threshold,
                )
                .await
            }
            PoolConsensusItem::EpochEnd(epoch_end) => {
                epoch::process_consensus_item(dbtx, self.epoch_config(), peer_id, epoch_end).await
            }
        }
    }

    fn build_verification_cache<'a>(
        &'a self,
        _inputs: impl Iterator<Item = &'a PoolInput> + Send,
    ) -> Self::VerificationCache {
        PoolVerificationCache
    }

    async fn process_input<'a, 'b, 'c>(
        &'a self,
        dbtx: &mut ModuleDatabaseTransaction<'c>,
        withdrawal: &'b PoolInput,
        _verification_cache: &Self::VerificationCache,
    ) -> Result<InputMeta, ModuleError> {
        let available = dbtx
            .get_value(&db::AccountBalanceKey(withdrawal.account))
            .await
            .map(|acc| acc.unlocked)
            .unwrap_or(fedimint_core::Amount::ZERO);

        // TODO: we should also deduct seeker/provider actions that are set for the next
        // round

        if available < withdrawal.amount {
            return Err(WithdrawalError::UnavailableFunds {
                amount: withdrawal.amount,
                available,
            })
            .into_module_error_other();
        }

        let meta = InputMeta {
            amount: TransactionItemAmount {
                amount: withdrawal.amount,
                // TODO: Figure out how to do fees later.
                fee: fedimint_core::Amount::ZERO,
            },
            pub_keys: [withdrawal.account].into(),
        };

        tracing::debug!(account = %withdrawal.account, amount = %meta.amount.amount, "Stability pool withdrawal");

        let mut account = dbtx
            .get_value(&db::AccountBalanceKey(withdrawal.account))
            .await
            .unwrap_or_default();

        account.unlocked.msats = account
            .unlocked
            .msats
            .checked_sub(withdrawal.amount.msats)
            .expect("withdrawal amount should already be checked");

        dbtx.insert_entry(&db::AccountBalanceKey(withdrawal.account), &account)
            .await;

        Ok(meta)
    }

    async fn process_output<'a, 'b>(
        &'a self,
        dbtx: &mut ModuleDatabaseTransaction<'b>,
        deposit: &'a PoolOutput,
        out_point: OutPoint,
    ) -> Result<TransactionItemAmount, ModuleError> {
        // TODO: Maybe some checks into minimum deposit amount?

        // check deposit does not result in balance overflow
        if let Some(account) = dbtx
            .get_value(&db::AccountBalanceKey(deposit.account))
            .await
        {
            if !account.can_add_amount(deposit.amount) {
                return Err(StabilityPoolError::DepositTooLarge).into_module_error_other();
            }
        }

        let txo_amount = TransactionItemAmount {
            amount: deposit.amount,
            // TODO: Figure out fee logic
            fee: fedimint_core::Amount::ZERO,
        };

        let mut account = dbtx
            .get_value(&db::AccountBalanceKey(deposit.account))
            .await
            .unwrap_or_default();
        account.unlocked.msats = account
            .unlocked
            .msats
            .checked_add(deposit.amount.msats)
            .expect("already checked overflow");

        dbtx.insert_entry(&db::AccountBalanceKey(deposit.account), &account)
            .await;

        dbtx.insert_new_entry(&db::DepositOutcomeKey(out_point), &deposit.account)
            .await;

        Ok(txo_amount)
    }

    async fn output_status(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        outpoint: OutPoint,
    ) -> Option<PoolOutputOutcome> {
        dbtx.get_value(&db::DepositOutcomeKey(outpoint))
            .await
            .map(PoolOutputOutcome)
    }

    async fn audit(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        audit: &mut Audit,
    ) {
        audit
            .add_items(
                dbtx,
                common::KIND.as_str(),
                &AccountBalanceKeyPrefix,
                |_, v| ((v.unlocked + v.locked.amount()).msats) as i64,
            )
            .await;
    }

    fn api_endpoints(&self) -> Vec<ApiEndpoint<Self>> {
        crate::api::endpoints()
    }
}

impl StabilityPool {
    /// Create new module instance
    pub fn new(cfg: PoolConfig) -> Self {
        let oracle = cfg.consensus.oracle.oracle_client();
        Self {
            cfg,
            oracle,
            backoff: Default::default(),
            proposed_db: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum StabilityPoolError {
    SomethingDummyWentWrong,
    DepositTooLarge,
}

impl std::fmt::Display for StabilityPoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SomethingDummyWentWrong => write!(f, "placeholder error"),
            Self::DepositTooLarge => write!(f, "that deposit pukking big"),
        }
    }
}

impl std::error::Error for StabilityPoolError {}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WithdrawalError {
    UnavailableFunds {
        amount: fedimint_core::Amount,
        available: fedimint_core::Amount,
    },
}

impl std::fmt::Display for WithdrawalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WithdrawalError::UnavailableFunds { amount, available } => write!(
                f,
                "attempted to withdraw {} when only {} was available",
                amount, available
            ),
        }
    }
}

impl std::error::Error for WithdrawalError {}
