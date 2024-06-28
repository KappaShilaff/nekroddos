use ed25519_dalek::{Keypair, PublicKey};
use nekoton::core::ton_wallet::TransferAction;
use nekoton::models::Expiration;
use nekoton_utils::SimpleClock;
use ton_abi::sign_with_signature_id;
use ton_block::{AccountStuff, GetRepresentationHash, MsgAddressInt};
use ton_types::{BuilderData, IBitstring, SliceData};

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

pub fn compute_contract_address(
    public_key: &PublicKey,
    workchain_id: i8,
    nonce: u32,
) -> MsgAddressInt {
    let hash = make_state_init(public_key, nonce)
        .and_then(|state| state.hash())
        .unwrap();
    MsgAddressInt::AddrStd(ton_block::MsgAddrStd::with_address(
        None,
        workchain_id,
        hash.into(),
    ))
}

pub fn make_state_init(public_key: &PublicKey, nonce: u32) -> anyhow::Result<ton_block::StateInit> {
    let mut data = BuilderData::new();
    data.append_raw(public_key.as_bytes(), 256)?
        .append_u64(0)?
        .append_u32(nonce)?;
    let data = data.into_cell()?;

    Ok(ton_block::StateInit {
        code: Some(nekoton_contracts::wallets::code::ever_wallet()),
        data: Some(data),
        ..Default::default()
    })
}

#[cfg(test)]
mod test {
    use ed25519_dalek::PublicKey;

    #[test]
    fn test() {
        let pubkey =
            hex::decode("6f4a7a2cf5f799dc1493c117dd87d074f3c37f21552e604ff5fbae649d58c96b")
                .unwrap();
        let pubkey = PublicKey::from_bytes(&pubkey).unwrap();
        let addr = super::compute_contract_address(&pubkey, 0, 6489);
        assert_eq!(
            addr.to_string(),
            "0:26c84e441ed5353a391018409c8f623486974f7022f822548f394e3236dd1aac"
        );
    }
}
