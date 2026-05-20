use agentic_afk_control_plane_server::{ControlPlaneConfig, serve};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "agentic-afk", about = "Operate the Local Control Plane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentic_afk_control_plane_server=info,tower_http=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Serve => serve(ControlPlaneConfig::from_env()?).await,
    }
}
