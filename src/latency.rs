use crate::models::GenericDeploymentInfo;
use crate::{send, Args};
use anyhow::{Context, Result};
use clap::Parser;
use ed25519_dalek::Keypair;
use everscale_rpc_client::RpcClient;
use governor::{Jitter, RateLimiter};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug, Clone)]
pub struct LatencyTestArgs {
    #[clap(short, long)]
    /// Number of transactions to send
    num_txs: usize,

    #[clap(short, long)]
    /// Transactions per second
    rps: u32,

    #[clap(short, long, default_value = "1000000")]
    /// Amount to send in nanotons
    amount: u64,

    #[clap(short, long)]
    /// Csv file to save results
    csv: Option<PathBuf>,
}

pub async fn run(
    latency_args: LatencyTestArgs,
    common_args: Args,
    keypair: &Keypair,
    client: RpcClient,
) -> Result<()> {
    const COST_PER_TRANSACTION: u64 = 8_857_001;

    let base_deployments_path = common_args.project_root.join("deployments");
    let network_deployments_path = if let Some(network_name) = &common_args.network {
        base_deployments_path.join(network_name)
    } else {
        base_deployments_path
    };

    if common_args.network.is_some() && !network_deployments_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Specified network deployment directory not found: {:?}",
            network_deployments_path
        ));
    }

    log::info!("Using deployments path: {:?}", network_deployments_path);

    let wallet = walkdir::WalkDir::new(&network_deployments_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().map(|e| e == "json").unwrap_or(false))
        .find(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().contains("ever-wallet"))
                .unwrap_or(false)
        })
        .context("No factory abi")?;
    let deployment: GenericDeploymentInfo = serde_json::from_slice(&std::fs::read(wallet.path())?)?;
    let sender = deployment.address;

    log::info!("Sender address: {}", sender);

    let initial_balance = client
        .get_contract_state(&sender, None)
        .await?
        .unwrap()
        .account
        .storage
        .balance
        .grams
        .as_u128();

    let required_balance = COST_PER_TRANSACTION * latency_args.num_txs as u64;
    let max_iterations = initial_balance / COST_PER_TRANSACTION as u128;

    log::info!(
        "Starting latency test - sending {} transactions at {} TPS",
        latency_args.num_txs,
        latency_args.rps
    );
    log::info!(
        "Initial balance: {}, required balance: {}, max iterations: {}",
        initial_balance,
        required_balance,
        max_iterations
    );

    let rl = RateLimiter::direct(governor::Quota::per_second(
        std::num::NonZeroU32::new(latency_args.rps).unwrap(),
    ));

    let mut csv_writer = if let Some(csv_path) = &latency_args.csv {
        let mut writer = std::fs::File::create(csv_path)?;
        writeln!(writer, "latency_ns")?;
        Some(writer)
    } else {
        None
    };

    let jitter = Jitter::new(Duration::from_millis(1), Duration::from_millis(50));
    let mut latencies = Vec::with_capacity(latency_args.num_txs);
    let mut success_count = 0;
    let mut error_count = 0;

    for i in 0..std::cmp::min(latency_args.num_txs, max_iterations as usize) {
        rl.until_ready_with_jitter(jitter).await;

        let start = Instant::now();

        match send_test_transaction(&client, keypair, &sender, &sender, latency_args.amount).await {
            Ok(_) => {
                let latency = start.elapsed();
                latencies.push(latency);
                success_count += 1;
                log::debug!("Transaction {} succeeded in {:?}", i, latency);

                if let Some(writer) = &mut csv_writer {
                    writeln!(writer, "{}", latency.as_nanos())?;
                    writer.flush()?;
                }
            }
            Err(e) => {
                error_count += 1;
                log::error!("Transaction {} failed: {}", i, e);
            }
        }
    }

    // Calculate statistics
    if !latencies.is_empty() {
        latencies.sort();
        let total: Duration = latencies.iter().sum();
        let avg = total / latencies.len() as u32;
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[(latencies.len() as f64 * 0.95) as usize];
        let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];

        log::info!("Latency test results:");
        log::info!("Successful transactions: {}", success_count);
        log::info!("Failed transactions: {}", error_count);
        log::info!("Average latency: {:?}", avg);
        log::info!("P50 latency: {:?}", p50);
        log::info!("P95 latency: {:?}", p95);
        log::info!("P99 latency: {:?}", p99);
    }

    Ok(())
}
async fn send_test_transaction(
    client: &RpcClient,
    keypair: &Keypair,
    sender: &ton_block::MsgAddressInt,
    receiver: &ton_block::MsgAddressInt,
    amount: u64,
) -> Result<()> {
    let payload = ton_types::BuilderData::new();
    let state = client.get_contract_state(sender, None).await?.unwrap();
    let balance = state.account.storage.balance.grams.as_u128();
    log::info!("Sender balance: {}", balance);
    let prev_lt = state.account.storage.last_trans_lt;

    send::send(
        client,
        keypair,
        sender.clone(),
        payload,
        receiver.clone(),
        amount,
        &state.account,
    )
    .await?;

    // Wait for the transaction to be included in the block
    loop {
        let state = client.get_contract_state(sender, None).await?.unwrap();
        if state.account.storage.last_trans_lt != prev_lt {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    Ok(())
}
