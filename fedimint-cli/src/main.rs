use fedimint_cli::FedimintCli;
use stabilitypool_client::PoolClientGen;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    FedimintCli::new()?
        .with_default_modules()
        .with_module(PoolClientGen)
        .run()
        .await;
    Ok(())
}
