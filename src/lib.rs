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
    /// total swaps per wallet
    num_swaps: usize,

    #[clap(short, long)]
    rps: u32,

    #[clap(short, long, default_value = "5")]
    /// swap depth
    depth: u8,
}

pub async fn run_test() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    if args.depth < 2 {
        panic!("Depth should be at least 2");
    }
    dotenvy::from_filename(args.project_root.join(".env")).context("Failed to load .env file")?;

    let seed = dotenvy::var("BROXUS_PHRASE").context("SEED is not set")?;
    let keypair =
        nekoton::crypto::derive_from_phrase(&seed, nekoton::crypto::MnemonicType::Labs(0))
            .context("Failed to derive keypair")?;

    let deployments_path = args.project_root.join("deployments");
    let mut recipients = Vec::new();
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
            recipients.push(wallet_info.address);
        }
        if filename.contains("DexPair") {
            let info: GenericDeploymentInfo = serde_json::from_slice(&std::fs::read(file.path())?)?;
            pool_addresses.push(info.address);
        }
    }

    log::info!(
        "Found {} wallets and {} pools",
        recipients.len(),
        pool_addresses.len()
    );

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
            let payload_meta = app_cache
                .generate_payloads(recipient.clone(), args.depth)
                .await;
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
    let rps = args.rps;

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
    mut send_data: SendData,
    barrier: Arc<Barrier>,
    rl: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    num_swaps: usize,
    counter: Arc<AtomicU64>,
) {
    let state = client
        .get_contract_state(&send_data.sender_addr, None)
        .await
        .unwrap()
        .unwrap();
    let jitter = Jitter::new(Duration::from_millis(1), Duration::from_millis(50));
    for _ in 0..num_swaps {
        rl.until_ready_with_jitter(jitter).await;
        if let Err(e) =
            send_forward_and_backward(&client, &mut send_data, &state.account, &rl, jitter).await
        {
            log::info!("Failed to send: {:?}", e);
            continue;
        }
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    barrier.wait().await;
}

async fn send_forward_and_backward(
    client: &RpcClient,
    payload: &mut SendData,
    state: &AccountStuff,
    rl: &RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
    jitter: Jitter,
) -> Result<()> {
    let forward_meta = &payload.payload_generators.forward.generate_payload_meta();
    rl.until_ready_with_jitter(jitter).await;
    send::send(
        client,
        &payload.signer,
        payload.sender_addr.clone(),
        forward_meta.payload.clone(),
        forward_meta.destination.clone(),
        3_000_000_000,
        None,
        state,
    )
    .await?;

    let backward_meta = &payload.payload_generators.backward.generate_payload_meta();
    rl.until_ready_with_jitter(jitter).await;
    send::send(
        client,
        &payload.signer,
        payload.sender_addr.clone(),
        backward_meta.payload.clone(),
        backward_meta.destination.clone(),
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
