use crate::models::{SendData};
use ton_block::{MsgAddressInt};

mod abi;
mod app_cache;
mod build_payload;
mod models;
mod send;

pub async fn run_test() {
    let pool_addresses = vec![];
    let recipients: Vec<MsgAddressInt> = vec![];

    let tx = everscale_rpc_client::RpcClient::new(vec![], Default::default())
        .await
        .unwrap();

    let app_cache = app_cache::AppCache::default()
        .load_states(&tx, pool_addresses)
        .await
        .load_tokens_and_token_pairs();

    let payloads = recipients
        .into_iter()
        .map(|recipient| {
            let payload_meta = app_cache.generate_payloads(recipient.clone(), 5);
            todo!()
        })
        .collect::<Vec<SendData>>();
}
