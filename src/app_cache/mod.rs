use crate::abi::dex_pair;
use crate::build_payload::build_double_side_payloads;
use crate::models::{GetTokenRoots, PayloadInput, PayloadMeta, StepInput};
use everscale_rpc_client::RpcClient;
use nekoton::utils::SimpleClock;
use nekoton_abi::{FunctionExt, UnpackAbiPlain};
use rand::prelude::SliceRandom;
use std::collections::{HashMap, HashSet};
use ton_block::{AccountStuff, MsgAddressInt};

fn build_answer_id_camel() -> ton_abi::Token {
    ton_abi::Token::new(
        "answerId",
        ton_abi::TokenValue::Uint(ton_abi::Uint::new(1337, 32)),
    )
}

#[derive(Default, Clone)]
pub struct AppCache {
    pub pool_states: HashMap<MsgAddressInt, AccountStuff>,
    pub token_pairs: HashMap<(MsgAddressInt, MsgAddressInt), MsgAddressInt>,
    pub tokens: Vec<MsgAddressInt>,
}

impl AppCache {
    pub async fn load_states(mut self, tx: &RpcClient, pool_addresses: Vec<MsgAddressInt>) -> Self {
        let mut res = HashMap::new();
        for address in pool_addresses {
            if let Ok(Some(account)) = tx.get_contract_state(&address, None).await {
                res.insert(address, account.account);
            }
        }

        self.pool_states = res;

        self
    }

    pub fn load_tokens_and_token_pairs(mut self) -> Self {
        let mut token_pairs = HashMap::new();
        let mut tokens = HashSet::new();

        let answer_id = vec![build_answer_id_camel()];

        for (address, account) in self.pool_states.iter() {
            if let Ok(token_pair) = dex_pair().function("getTokenRoots").unwrap().run_local(
                &SimpleClock,
                account.clone(),
                &answer_id,
            ) {
                let token_roots: GetTokenRoots = token_pair.tokens.unwrap().unpack().unwrap();
                tokens.insert(token_roots.left.clone());
                tokens.insert(token_roots.right.clone());
                token_pairs.insert((token_roots.left, token_roots.right), address.clone());
            }
        }

        self.token_pairs = token_pairs;
        self.tokens = tokens.into_iter().collect();

        self
    }

    pub fn generate_payloads(&self, recipient: MsgAddressInt, steps_len: u8) -> PayloadMeta {
        let route = self.generate_route(steps_len);
        build_double_side_payloads(
            PayloadInput {
                steps: route,
                recipient,
                id: 0,
            },
            self,
        )
    }

    fn generate_route(&self, steps_len: u8) -> Vec<StepInput> {
        let mut res = vec![];
        let mut exists_tokens = HashSet::new();
        let mut rng = rand::thread_rng();

        let mut from_token = self.tokens.choose(&mut rng).cloned().unwrap();
        exists_tokens.insert(from_token.clone());

        for _ in 0..steps_len {
            loop {
                let to_token = self.tokens.choose(&mut rng).cloned().unwrap();
                if exists_tokens.contains(&to_token) {
                    continue;
                }

                let (left_token, right_token, pool_address) = if let Some(pool_address) = self
                    .token_pairs
                    .get(&(from_token.clone(), to_token.clone()))
                {
                    (from_token.clone(), to_token.clone(), pool_address.clone())
                } else if let Some(pool_address) = self
                    .token_pairs
                    .get(&(to_token.clone(), from_token.clone()))
                {
                    (to_token.clone(), from_token.clone(), pool_address.clone())
                } else {
                    continue;
                };

                exists_tokens.insert(to_token.clone());

                res.push(StepInput {
                    pool_address,
                    currency_addresses: vec![left_token.clone(), right_token.clone()],
                    from_currency_address: from_token.clone(),
                    to_currency_address: to_token.clone(),
                });

                from_token = to_token;

                break;
            }
        }

        res
    }
}
