use fedimintd::fedimintd::Fedimintd;
use stabilitypool_server::{common, PoolConfigGenerator};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Fedimintd::new()?
        .with_default_modules()
        .with_module(PoolConfigGenerator)
        .with_extra_module_inits_params(
            3,
            common::KIND,
            common::config::PoolGenParams {
                local: Default::default(),
                consensus: Default::default(),
            },
        )
        .run()
        .await
}
