use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use ed25519_dalek::Keypair;
use everscale_rpc_client::RpcClient;
use futures_util::StreamExt;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Jitter, RateLimiter};
use tokio::sync::Barrier;
use ton_block::AccountStuff;
use url::Url;

use crate::models::{EverWalletInfo, GenericDeploymentInfo, SendData};
use crate::send::compute_contract_address;

mod abi;
mod app_cache;
mod build_payload;
mod models;
mod send;

#[derive(Parser, Debug)]
struct Args {
    #[clap(short, long)]
    project_root: PathBuf,

    #[clap(short, long)]
    endpoint: Url,

    #[clap(short, long)]
    num_swaps: usize,

    #[clap(short, long)]
    rps: u32,
}

pub async fn run_test() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    dotenvy::from_filename(args.project_root.join(".env")).context("Failed to load .env file")?;

    let seed = dotenvy::var("BROXUS_PHRASE").context("SEED is not set")?;
    let keypair =
        nekoton::crypto::derive_from_phrase(&seed, nekoton::crypto::MnemonicType::Labs(0))
            .context("Failed to derive keypair")?;

    let deployments_path = args.project_root.join("deployments");
    let mut wallet_nonce = Vec::new();
    let mut pool_addresses = Vec::new();

    for file in walkdir::WalkDir::new(&deployments_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().map(|e| e == "json").unwrap_or(false))
    {
        let filename = file.file_name().to_string_lossy();
        if filename.contains("commonAccount") {
            let wallet_info: EverWalletInfo = serde_json::from_slice(&std::fs::read(file.path())?)?;
            wallet_nonce.push(wallet_info.create_account_params.nonce);
        }
        if filename.contains("DexPair") {
            let info: GenericDeploymentInfo = serde_json::from_slice(&std::fs::read(file.path())?)?;
            pool_addresses.push(info.address);
        }
    }

    log::info!(
        "Found {} wallets and {} pools",
        wallet_nonce.len(),
        pool_addresses.len()
    );

    let recipients: Vec<_> = wallet_nonce
        .iter()
        .map(|nonce| compute_contract_address(&keypair.public, 0, *nonce))
        .collect();

    let client = RpcClient::new(vec![args.endpoint], Default::default())
        .await
        .unwrap();

    let app_cache = app_cache::AppCache::new(client.clone())
        .load_states(pool_addresses)
        .await
        .load_tokens_and_token_pairs();

    log::info!("Loaded app cache");

    let start = std::time::Instant::now();
    let payloads = futures_util::stream::iter(recipients)
        .filter(|x| {
            let client = client.clone();
            let addr = x.clone();
            async move {
                client
                    .get_contract_state(&addr, None)
                    .await
                    .unwrap()
                    .is_some()
            }
        })
        .map(|recipient| async {
            let payload_meta = app_cache.generate_payloads(recipient.clone(), 5).await;
            SendData::new(
                payload_meta,
                Keypair::from_bytes(&keypair.to_bytes()).unwrap(),
                recipient,
            )
        })
        .buffered(100)
        .collect::<Vec<SendData>>()
        .await;

    log::info!(
        "Generated {} payloads in {:?}",
        payloads.len(),
        start.elapsed()
    );

    let barrier = Barrier::new(payloads.len() + 1);
    let barrier = Arc::new(barrier);

    let num_swaps = args.num_swaps;

    let rps = args.rps / 2; // forward and backward
    let quota = governor::Quota::per_minute(NonZeroU32::new(rps * 60).unwrap())
        .allow_burst(NonZeroU32::new(rps / 10).unwrap());

    let rate_limiter = Arc::new(governor::RateLimiter::direct(quota));
    let counter = Arc::new(AtomicU64::new(0));

    for payload in payloads {
        let client = client.clone();
        let barrier = barrier.clone();
        let counter = counter.clone();
        let rate_limiter = rate_limiter.clone();
        tokio::spawn(async move {
            process_payload(client, payload, barrier, rate_limiter, num_swaps, counter).await
        });
    }
    log::info!("Spawned dudos tasks");
    tokio::spawn(async move {
        print_stats(counter).await;
    });

    barrier.wait().await;
    Ok(())
}

async fn process_payload(
    client: RpcClient,
    payload: SendData,
    barrier: Arc<Barrier>,
    rl: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    num_swaps: usize,
    counter: Arc<AtomicU64>,
) {
    let state = client
        .get_contract_state(&payload.sender_addr, None)
        .await
        .unwrap()
        .unwrap();
    let jitter = Jitter::new(Duration::from_millis(1), Duration::from_millis(50));
    for _ in 0..num_swaps {
        rl.until_ready_with_jitter(jitter).await;
        if let Err(e) = send_forward_and_backward(&client, &payload, &state.account).await {
            log::info!("Failed to send: {:?}", e);
            continue;
        }
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    barrier.wait().await;
}

async fn send_forward_and_backward(
    client: &RpcClient,
    payload: &SendData,
    state: &AccountStuff,
) -> Result<()> {
    let forward_route = &payload.payload_meta.forward_route;
    send::send(
        client,
        &payload.signer,
        payload.sender_addr.clone(),
        forward_route.payload.clone(),
        forward_route.destination.clone(),
        3_000_000_000,
        None,
        state,
    )
    .await?;

    let backward_route = &payload.payload_meta.backward_route;
    send::send(
        client,
        &payload.signer,
        payload.sender_addr.clone(),
        backward_route.payload.clone(),
        backward_route.destination.clone(),
        3_000_000_000,
        None,
        state,
    )
    .await?;

    Ok(())
}

async fn print_stats(counter: Arc<AtomicU64>) {
    let start = std::time::Instant::now();
    loop {
        let count = counter.load(std::sync::atomic::Ordering::Relaxed);
        log::info!("Sent {} transactions in {:?}", count, start.elapsed());
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
