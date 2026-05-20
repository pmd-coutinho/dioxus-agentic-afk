use agentic_afk_control_plane_server::{ControlPlaneConfig, run_migrate, run_seed_dev, serve};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "agentic-afk", about = "Operate the Local Control Plane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the Local Control Plane server
    Serve,
    /// Apply database migrations
    Migrate,
    /// Seed this repository as a development Project (idempotent)
    SeedDev,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentic_afk_control_plane_server=info,tower_http=info".into()),
        )
        .init();

    let config = ControlPlaneConfig::from_env()?;

    match Cli::parse().command {
        Command::Serve => serve(config).await,
        Command::Migrate => run_migrate(&config.database_url).await,
        Command::SeedDev => {
            let dev_path = std::env::current_dir()?
                .to_string_lossy()
                .to_string();
            run_seed_dev(&config.database_url, &dev_path).await
        }
    }
}

