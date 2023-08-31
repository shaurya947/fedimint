use fedimintd::fedimintd::Fedimintd;
use stabilitypool_server::PoolConfigGenerator;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Fedimintd::new()?.with_default_modules().with_module(PoolConfigGenerator).run().await
}
