#[tokio::main]
async fn main() -> anyhow::Result<()> {
    vulse_server::run().await
}
