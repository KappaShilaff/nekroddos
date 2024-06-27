use crate::abi::{dex_pair, token_wallet};
use crate::app_cache::AppCache;
use crate::models::{
    DexPairV9BuildCrossPairExchangePayloadV2, DexPairV9Steps, PayloadInput, PayloadMeta, RouteMeta,
    StepInput, Transfer,
};
use nekoton::utils::{SimpleClock, TrustMe};
use nekoton_abi::{FunctionExt, PackAbiPlain};
use ton_abi::TokenValue;
use ton_block::MsgAddressInt;

pub fn build_double_side_payloads(mut input: PayloadInput, app_cache: &AppCache) -> PayloadMeta {
    let forward_route = build_payload(input.recipient.clone(), input.steps.clone(), app_cache);

    input.steps.reverse();
    input.steps.iter_mut().for_each(|x| {
        std::mem::swap(&mut x.from_currency_address, &mut x.to_currency_address);
    });

    let backward_route = build_payload(input.recipient, input.steps, app_cache);
    PayloadMeta {
        forward_route,
        backward_route,
    }
}

fn build_payload(
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

    let tokens = dbg!(DexPairV9BuildCrossPairExchangePayloadV2 {
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
    })
    .pack();
    log::error!("ADDR: {}", first_pool.pool_address.to_string());

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
        remaining_gas_to: recipient,
        notify: true,
        payload,
    }
    .pack();

    RouteMeta {
        payload: token_wallet()
            .function("transfer")
            .unwrap()
            .encode_internal_input(&transfer_tokens)
            .unwrap(),
        first_pool_address: first_pool.pool_address,
    }
}
