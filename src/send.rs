use ed25519_dalek::Keypair;
use nekoton::core::ton_wallet::TransferAction;
use nekoton::models::Expiration;
use nekoton_utils::SimpleClock;
use ton_abi::sign_with_signature_id;
use ton_block::{AccountStuff, MsgAddressInt};
use ton_types::{BuilderData, SliceData};

#[allow(clippy::too_many_arguments)]
pub async fn send(
    client: &everscale_rpc_client::RpcClient,
    signer: &Keypair,
    from: MsgAddressInt,
    payload: BuilderData,
    destination: MsgAddressInt,
    amount: u64,
    sign_id: Option<i32>,
    state: &AccountStuff,
) -> anyhow::Result<()> {
    let gift = nekoton::core::ton_wallet::Gift {
        flags: 3,
        bounce: false,
        destination,
        amount,
        body: Some(SliceData::load_builder(payload)?),
        state_init: None,
    };

    let now = nekoton_utils::now_sec_u64() as u32 + 60;

    let message = nekoton::core::ton_wallet::ever_wallet::prepare_transfer(
        &SimpleClock,
        &signer.public,
        state,
        from.clone(),
        vec![gift],
        Expiration::Timestamp(now),
    )?;
    let message = match message {
        TransferAction::DeployFirst => panic!("DeployFirst not supported"),
        TransferAction::Sign(m) => m,
    };
    let signature = sign_with_signature_id(signer, message.hash(), sign_id);
    let signed_message = message.sign(&signature.to_bytes()).unwrap().message;

    client.broadcast_message(signed_message).await?;

    Ok(())
}
