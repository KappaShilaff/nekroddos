use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use ed25519_dalek::Keypair;
use everscale_rpc_client::RpcClient;
use tokio::sync::Barrier;
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
    sleep_ms: u64,
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
            let wallet_info: EverWalletInfo =
                serde_json::from_reader(std::fs::File::open(file.path())?)?;
            wallet_nonce.push(wallet_info.create_account_params.nonce);
        }
        if filename.contains("DexPair") {
            let info: GenericDeploymentInfo =
                serde_json::from_reader(std::fs::File::open(file.path())?)?;
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

    let app_cache = app_cache::AppCache::default()
        .load_states(&client, pool_addresses)
        .await
        .load_tokens_and_token_pairs();

    let start = std::time::Instant::now();
    let payloads = recipients
        .into_iter()
        .map(|recipient| {
            let payload_meta = app_cache.generate_payloads(recipient.clone(), 5);
            SendData::new(
                payload_meta,
                Keypair::from_bytes(&keypair.to_bytes()).unwrap(),
                recipient,
            )
        })
        .collect::<Vec<SendData>>();

    log::info!(
        "Generated {} payloads in {:?}",
        payloads.len(),
        start.elapsed()
    );

    let barrier = Barrier::new(payloads.len() + 1);
    let barrier = Arc::new(barrier);

    let num_swaps = args.num_swaps;
    let sleep_duration = Duration::from_millis(args.sleep_ms);
    let counter = Arc::new(AtomicU64::new(0));

    for payload in payloads {
        let client = client.clone();
        let barrier = barrier.clone();
        let counter = counter.clone();
        tokio::spawn(async move {
            process_payload(client, payload, barrier, sleep_duration, num_swaps, counter).await
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
    sleep_duration: Duration,
    num_swaps: usize,
    counter: Arc<AtomicU64>,
) {
    for _ in 0..num_swaps {
        if let Err(e) = send_forward_and_backward(&client, &payload).await {
            log::info!("Failed to send: {:?}", e);
            continue;
        }
        tokio::time::sleep(sleep_duration).await;
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    barrier.wait().await;
}

async fn send_forward_and_backward(client: &RpcClient, payload: &SendData) -> Result<()> {
    let forward_route = &payload.payload_meta.forward_route;
    send::send(
        client,
        &payload.signer,
        payload.sender_addr.clone(),
        forward_route.payload.clone(),
        forward_route.first_pool_address.clone(),
        3_000_000_000,
        None,
    )
    .await?;

    let backward_route = &payload.payload_meta.backward_route;
    send::send(
        client,
        &payload.signer,
        payload.sender_addr.clone(),
        backward_route.payload.clone(),
        backward_route.first_pool_address.clone(),
        3_000_000_000,
        None,
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
