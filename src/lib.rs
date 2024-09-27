use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::dag::DagTestArgs;
use crate::send_tokens::SendTestArgs;
use crate::swap::SwapTestArgs;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use everscale_rpc_client::{ClientOptions, ReliabilityParams, RpcClient};
use url::Url;

mod abi;
mod app_cache;
mod build_payload;
mod models;
mod send;

mod dag;
mod send_tokens;
mod swap;
mod util;

#[derive(Parser, Debug, Clone)]
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

    /// do not fait for the node answer on send message
    #[clap(short, long)]
    no_wait: bool,

    /// Which timediff makes the node dead
    #[clap(long = "dead-seconds", default_value = "120")]
    node_is_dead_seconds: u64,
}

#[derive(Subcommand, Debug, Clone)]
enum Commands {
    Swap(SwapTestArgs),
    Dag(DagTestArgs),
    Send(SendTestArgs),
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
    let keypair = Arc::new(keypair);
    let client = RpcClient::new(
        app_args.endpoints.clone(),
        ClientOptions {
            request_timeout: Duration::from_secs(60),
            choose_strategy: everscale_rpc_client::ChooseStrategy::RoundRobin,
            reliability_params: ReliabilityParams {
                mc_acceptable_time_diff_sec: app_args.node_is_dead_seconds,
                sc_acceptable_time_diff_sec: app_args.node_is_dead_seconds,
                acceptable_blocks_diff: 500,
            },
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
        Commands::Send(args) => {
            send_tokens::run(args.clone(), app_args, keypair, client).await?;
        }
    }

    Ok(())
}
