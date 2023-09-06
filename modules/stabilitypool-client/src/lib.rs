mod cli;

use std::ffi;

use async_trait::async_trait;
use common::action::SignedAction;
use common::config::PoolClientConfig;
use common::{
    Action, ActionProposed, ActionStaged, EpochOutcome, PoolCommonGen, PoolModuleTypes,
    ProviderBid, SeekerAction,
};
use fedimint_client::derivable_secret::DerivableSecret;
use fedimint_client::module::init::ClientModuleInit;
use fedimint_client::module::ClientModule;
use fedimint_client::sm::{DynState, ModuleNotifier, OperationId, State, StateTransition};
use fedimint_client::{Client, DynGlobalClientContext};
use fedimint_core::api::{DynGlobalApi, DynModuleApi, FederationApiExt, FederationError};
use fedimint_core::config::FederationId;
use fedimint_core::core::{IntoDynInstance, ModuleInstanceId};
use fedimint_core::db::Database;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::{
    ApiRequestErased, ApiVersion, ExtendsCommonModuleInit, MultiApiVersion,
};
use fedimint_core::{apply, async_trait_maybe_send, BitcoinHash};
use secp256k1_zkp::Secp256k1;
use stabilitypool_common as common;
use stabilitypool_server::api::BalanceResponse;

#[derive(Debug, Clone)]
pub struct PoolClientGen;

impl ExtendsCommonModuleInit for PoolClientGen {
    type Common = PoolCommonGen;
}

#[apply(async_trait_maybe_send!)]
impl ClientModuleInit for PoolClientGen {
    type Module = PoolClientModule;

    async fn init(
        &self,
        _federation_id: FederationId,
        cfg: PoolClientConfig,
        _db: Database,
        _api_version: ApiVersion,
        module_root_secret: DerivableSecret,
        _notifier: ModuleNotifier<DynGlobalClientContext, <Self::Module as ClientModule>::States>,
        _api: DynGlobalApi,
        _module_api: DynModuleApi,
    ) -> anyhow::Result<Self::Module> {
        Ok(PoolClientModule {
            cfg,
            key: module_root_secret.to_secp_key(&Secp256k1::new()),
        })
    }

    fn supported_api_versions(&self) -> MultiApiVersion {
        MultiApiVersion::try_from_iter([ApiVersion { major: 0, minor: 0 }])
            .expect("no version conflicts")
    }
}

#[derive(Debug)]
pub struct PoolClientModule {
    cfg: PoolClientConfig,
    key: secp256k1_zkp::KeyPair,
}

#[async_trait]
impl ClientModule for PoolClientModule {
    type Common = PoolModuleTypes;
    type ModuleStateMachineContext = ();
    type States = PoolClientStates;

    fn context(&self) -> Self::ModuleStateMachineContext {}

    fn input_amount(
        &self,
        _input: &<Self::Common as fedimint_core::module::ModuleCommon>::Input,
    ) -> fedimint_core::module::TransactionItemAmount {
        todo!()
    }

    fn output_amount(
        &self,
        _output: &<Self::Common as fedimint_core::module::ModuleCommon>::Output,
    ) -> fedimint_core::module::TransactionItemAmount {
        todo!()
    }

    async fn handle_cli_command(
        &self,
        client: &Client,
        args: &[ffi::OsString],
    ) -> anyhow::Result<serde_json::Value> {
        let rng = rand::rngs::OsRng;
        let output = cli::handle_cli_args(client, rng, args).await?;
        Ok(serde_json::to_value(output).expect("infallible"))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Decodable, Encodable)]
pub enum PoolClientStates {}

impl IntoDynInstance for PoolClientStates {
    type DynType = DynState<DynGlobalClientContext>;

    fn into_dyn(self, instance_id: ModuleInstanceId) -> Self::DynType {
        DynState::from_typed(instance_id, self)
    }
}

impl State for PoolClientStates {
    type ModuleContext = ();
    type GlobalContext = DynGlobalClientContext;

    fn transitions(
        &self,
        _context: &Self::ModuleContext,
        _global_context: &DynGlobalClientContext,
    ) -> Vec<StateTransition<Self>> {
        unimplemented!()
    }

    fn operation_id(&self) -> OperationId {
        unimplemented!()
    }
}

#[apply(async_trait_maybe_send!)]
pub trait PoolClientExt {
    fn account_key(&self) -> secp256k1_zkp::KeyPair;

    async fn balance(&self) -> Result<BalanceResponse, FederationError>;

    async fn epoch_outcome(&self, epoch_id: u64) -> Result<EpochOutcome, FederationError>;

    async fn staging_epoch(&self) -> Result<u64, FederationError>;

    async fn create_signed_acton<T: Encodable + Send>(
        &self,
        unsigned_action: T,
    ) -> Result<SignedAction<T>, FederationError>;

    async fn propose_seeker_action(&self, action: SeekerAction) -> Result<(), FederationError>;

    async fn propose_provider_action(&self, action: ProviderBid) -> Result<(), FederationError>;

    async fn staged_action(&self) -> Result<ActionStaged, FederationError>;

    async fn state(&self) -> Result<stabilitypool_server::api::State, FederationError>;
}

#[apply(async_trait_maybe_send!)]
impl PoolClientExt for Client {
    fn account_key(&self) -> secp256k1_zkp::KeyPair {
        let (pool_client, _pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_client.key
    }

    async fn balance(&self) -> Result<BalanceResponse, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                "account".to_string(),
                ApiRequestErased::new(self.account_key().x_only_public_key().0),
            )
            .await
    }

    async fn epoch_outcome(&self, epoch_id: u64) -> Result<EpochOutcome, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus("epoch".to_string(), ApiRequestErased::new(epoch_id))
            .await
    }

    async fn staging_epoch(&self) -> Result<u64, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus("epoch_next".to_string(), ApiRequestErased::default())
            .await
    }

    async fn create_signed_acton<T: Encodable + Send>(
        &self,
        unsigned_action: T,
    ) -> Result<SignedAction<T>, FederationError> {
        let kp = self.account_key();
        let sequence = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
        let action = Action {
            epoch_id: self.staging_epoch().await?,
            sequence,
            account_id: kp.x_only_public_key().0,
            body: unsigned_action,
        };

        let digest =
            bitcoin::hashes::sha256::Hash::hash(&action.consensus_encode_to_vec().unwrap());
        let signature = Secp256k1::signing_only().sign_schnorr(&digest.into(), &kp);
        let signed_action = SignedAction { signature, action };
        Ok(signed_action)
    }

    async fn propose_seeker_action(&self, action: SeekerAction) -> Result<(), FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        let signed_action: ActionProposed = self
            .create_signed_acton(action)
            .await
            .expect("TODO: signing should not fail")
            .into();
        pool_instance
            .api
            .request_current_consensus(
                "action_propose".to_string(),
                ApiRequestErased::new(&signed_action),
            )
            .await
    }

    async fn propose_provider_action(&self, action: ProviderBid) -> Result<(), FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        let signed_action: ActionProposed = self
            .create_signed_acton(action)
            .await
            .expect("TODO: signing should not fail")
            .into();
        pool_instance
            .api
            .request_current_consensus(
                "action_propose".to_string(),
                ApiRequestErased::new(&signed_action),
            )
            .await
    }

    async fn staged_action(&self) -> Result<ActionStaged, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                "action".to_string(),
                ApiRequestErased::new(self.account_key().x_only_public_key().0),
            )
            .await
    }

    async fn state(&self) -> Result<stabilitypool_server::api::State, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus("state".to_string(), ApiRequestErased::default())
            .await
    }
}
