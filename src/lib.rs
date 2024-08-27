use std::path::PathBuf;
use std::time::Duration;

use crate::dag::DagTestArgs;
use crate::swap::SwapTestArgs;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use everscale_rpc_client::{ClientOptions, RpcClient};
use url::Url;

mod abi;
mod app_cache;
mod build_payload;
mod models;
mod send;

mod dag;
mod swap;
mod util;

#[derive(Parser, Debug)]
struct Args {
    #[command(subcommand)]
    command: Commands,
    #[clap(short, long)]
    project_root: PathBuf,

    #[clap(short, long)]
    endpoints: Vec<Url>,

    /// seed for rng
    /// if you want to run multiple instances of the script with the same seed
    #[clap(short, long)]
    seed: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Swap(SwapTestArgs),
    Dag(DagTestArgs),
}

pub async fn run_test() -> Result<()> {
    env_logger::init();
    let app_args = Args::parse();

    dotenvy::from_filename(app_args.project_root.join(".env"))
        .context("Failed to load .env file")?;

    let seed = dotenvy::var("BROXUS_PHRASE").context("SEED is not set")?;
    let keypair =
        nekoton::crypto::derive_from_phrase(&seed, nekoton::crypto::MnemonicType::Labs(0))
            .context("Failed to derive keypair")?;
    let client = RpcClient::new(
        app_args.endpoints.clone(),
        ClientOptions {
            request_timeout: Duration::from_secs(1),
            choose_strategy: everscale_rpc_client::ChooseStrategy::RoundRobin,
            ..Default::default()
        },
    )
        .await?;

    match &app_args.command {
        Commands::Swap(args) => {
            swap::run(args.clone(), app_args, &keypair, client).await?;
        }
        Commands::Dag(args) => {
            dag::run(args.clone(), app_args, client).await?;
        }
    }

    Ok(())
}
