mod cleanup;
mod config;
mod daemon;
mod db;
mod dictionary;
mod ipc;
mod job;
mod learning;
mod local_cleanup;
mod media;
mod model_catalog;
mod platform;
mod standard_models;
mod text;
mod transcribe;
mod transcript_pipeline;
mod web;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kloyce", about = "Speech-to-text daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the kloyce daemon
    Daemon,
    /// Install systemd service and hyprland binding
    Install,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kloyce=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            let config = config::Config::load();
            tracing::info!("Config: {config:?}");
            let daemon = daemon::Daemon::new(config)?;
            daemon.run().await?;
        }
        Commands::Install => {
            platform::install::install()?;
        }
    }

    Ok(())
}
