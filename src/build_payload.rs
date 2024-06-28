use crate::abi::{dex_pair, token_root, token_wallet};
use crate::app_cache::AppCache;
use crate::models::{
    DexPairV9BuildCrossPairExchangePayloadV2, DexPairV9Steps, PayloadInput, PayloadMeta, RouteMeta,
    StepInput, Transfer,
};
use everscale_rpc_client::RpcClient;
use nekoton::utils::{SimpleClock, TrustMe};
use nekoton_abi::num_bigint::BigUint;
use nekoton_abi::{BuildTokenValue, FunctionExt, PackAbiPlain};
use ton_abi::{Token, TokenValue, Uint};
use ton_block::MsgAddressInt;

pub async fn build_double_side_payloads(
    mut input: PayloadInput,
    app_cache: &AppCache,
) -> PayloadMeta {
    let forward_route =
        build_payload(input.recipient.clone(), input.steps.clone(), app_cache).await;

    input.steps.reverse();
    input.steps.iter_mut().for_each(|x| {
        std::mem::swap(&mut x.from_currency_address, &mut x.to_currency_address);
    });

    let backward_route = build_payload(input.recipient, input.steps, app_cache).await;
    PayloadMeta {
        forward_route,
        backward_route,
    }
}

async fn build_payload(
    recipient: MsgAddressInt,
    mut steps: Vec<StepInput>,
    app_cache: &AppCache,
) -> RouteMeta {
    let first_pool = steps.remove(0);
    let chain_len = steps.len();

    let steps = steps
        .clone()
        .into_iter()
        .enumerate()
        .map(|(index, x)| DexPairV9Steps {
            amount: 0,
            roots: x.currency_addresses,
            outcoming: x.to_currency_address,
            numerator: 1,
            next_step_indices: if index == chain_len - 1 {
                vec![]
            } else {
                vec![index as u32 + 1]
            },
        })
        .collect();

    let tokens = DexPairV9BuildCrossPairExchangePayloadV2 {
        id: 0,
        deploy_wallet_grams: 0, // todo!
        expected_amount: 0,
        outcoming: first_pool.to_currency_address,
        next_step_indices: vec![0],
        steps,
        recipient: recipient.clone(),
        referrer: MsgAddressInt::default(),
        success_payload: None,
        cancel_payload: None,
    }
    .pack();

    let payload = dex_pair()
        .function("buildCrossPairExchangePayloadV2")
        .trust_me()
        .run_local(
            &SimpleClock,
            app_cache
                .pool_states
                .get(&first_pool.pool_address)
                .cloned()
                .unwrap(),
            &tokens,
        )
        .map_err(|x| {
            log::error!("run_local error {:#?}", x);
            x
        })
        .ok()
        .and_then(|x| {
            if x.tokens.is_none() {
                log::error!("run_local tokens none, result_code: {}", x.result_code);
            }
            x.tokens
        })
        .and_then(|x| x.into_iter().next())
        .and_then(|x| match x.value {
            TokenValue::Cell(x) => Some(x),
            _ => None,
        })
        .unwrap();

    let transfer_tokens = Transfer {
        amount: 100_000, // todo!
        recipient: first_pool.pool_address.clone(),
        deploy_wallet_value: 0,
        remaining_gas_to: recipient.clone(),
        notify: true,
        payload,
    }
    .pack();

    let destination =
        get_wallet_of(&app_cache.tx, &first_pool.from_currency_address, recipient).await;

    RouteMeta {
        payload: token_wallet()
            .function("transfer")
            .unwrap()
            .encode_internal_input(&transfer_tokens)
            .unwrap(),
        destination,
    }
}

async fn get_wallet_of(
    tx: &RpcClient,
    from_token_root: &MsgAddressInt,
    recipient: MsgAddressInt,
) -> MsgAddressInt {
    let res = tx
        .run_local(
            from_token_root,
            token_root().function("walletOf").unwrap(),
            &[
                Token::new(
                    "answerId",
                    TokenValue::Uint(Uint {
                        number: BigUint::from(0_u32),
                        size: 32,
                    }),
                ),
                Token::new("walletOwner", recipient.clone().token_value()),
            ],
        )
        .await
        .unwrap()
        .unwrap()
        .tokens
        .and_then(|x| x.into_iter().next())
        .unwrap();

    let wallet_token = match res.value {
        TokenValue::Address(x) => x,
        _ => {
            panic!("walletOf return not address, recipient: {recipient}, token_root: {from_token_root}")
        }
    };

    wallet_token.clone().to_msg_addr_int().unwrap()
}
