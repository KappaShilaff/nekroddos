use nekoton_abi::num_bigint::BigUint;
use nekoton_abi::{BuildTokenValue, FunctionExt, PackAbiPlain};
use nekoton_utils::SimpleClock;
use ton_abi::{Token, TokenValue, Uint};
use ton_block::{AccountStuff, MsgAddressInt};

use crate::abi::{dex_pair, token_root, token_wallet};
use crate::app_cache::AppCache;
use crate::models::{
    DexPairV9BuildCrossPairExchangePayloadV2, DexPairV9Steps, PayloadGenerator,
    PayloadGeneratorsData, PayloadInput, PayloadTokens, StepInput, Transfer,
};

pub fn build_double_side_payloads_data(
    mut input: PayloadInput,
    app_cache: &AppCache,
) -> PayloadGeneratorsData {
    let forward_route = build_route_data(input.recipient.clone(), input.steps.clone(), app_cache);

    input.steps.reverse();
    input.steps.iter_mut().for_each(|x| {
        std::mem::swap(&mut x.from_currency_address, &mut x.to_currency_address);
    });

    let backward_route = build_route_data(input.recipient, input.steps, app_cache);
    PayloadGeneratorsData {
        forward: forward_route,
        backward: backward_route,
    }
}

fn build_route_data(
    recipient: MsgAddressInt,
    mut steps: Vec<StepInput>,
    app_cache: &AppCache,
) -> PayloadGenerator {
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

    let swap_tokens = DexPairV9BuildCrossPairExchangePayloadV2 {
        id: 0,
        deploy_wallet_grams: 0,
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

    let transfer_tokens = Transfer {
        amount: 100_000, // todo! conf
        recipient: first_pool.pool_address.clone(),
        deploy_wallet_value: 0,
        remaining_gas_to: recipient.clone(),
        notify: true,
        payload: Default::default(),
    }
    .pack();

    let state = app_cache
        .tokens_states
        .get(&first_pool.from_currency_address)
        .cloned()
        .unwrap();
    let destination = get_wallet_of(state, &first_pool.from_currency_address, recipient);

    PayloadGenerator {
        first_pool_state: app_cache
            .pool_states
            .get(&first_pool.pool_address)
            .cloned()
            .unwrap(),
        swap_fun: dex_pair()
            .function("buildCrossPairExchangePayloadV2")
            .cloned()
            .unwrap(),
        transfer_fun: token_wallet().function("transfer").cloned().unwrap(),
        destination,
        tokens: PayloadTokens {
            swap: swap_tokens,
            transfer: transfer_tokens,
        },
    }
}

fn get_wallet_of(
    state: AccountStuff,
    from_token_root: &MsgAddressInt,
    recipient: MsgAddressInt,
) -> MsgAddressInt {
    let res = token_root()
        .function("walletOf")
        .unwrap()
        .run_local(
            &SimpleClock,
            state,
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
