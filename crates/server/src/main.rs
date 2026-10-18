#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("vulse {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "vulse {}\n\nRuns the Vulse server. Settings come from environment variables; see the README.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            other => anyhow::bail!("unknown argument '{other}'; try --help"),
        }
    }
    vulse_server::run().await
}
