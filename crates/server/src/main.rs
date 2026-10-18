#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(arg) = args.first() {
        match arg.as_str() {
            "backup" => return vulse_server::backup_command(&args[1..]).await,
            "--version" | "-V" => {
                println!("vulse {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "vulse {}\n\nUsage:\n  vulse                          Run the server\n  vulse backup [DIR] [--keep N]  Write a backup of the database and uploads to DIR\n                                 (default: VULSE_BACKUP_DIR, then ./backups) and\n                                 keep only the newest N backups there\n\nSettings come from environment variables; see the README.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            other => anyhow::bail!("unknown argument '{other}'; try --help"),
        }
    }
    vulse_server::run().await
}
