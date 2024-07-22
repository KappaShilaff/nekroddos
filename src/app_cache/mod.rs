use std::collections::HashMap;
use std::time::Instant;

use everscale_rpc_client::RpcClient;
use futures_util::stream::{FuturesOrdered, StreamExt};
use indexmap::IndexMap;
use itertools::Itertools;
use nekoton::utils::SimpleClock;
use nekoton_abi::{FunctionExt, UnpackAbiPlain};
use ton_block::{AccountStuff, MsgAddressInt};

use crate::abi::dex_pair;
use crate::build_payload::build_double_side_payloads_data;
use crate::models::{GetTokenRoots, PairInfo, PayloadGeneratorsData, PayloadInput, StepInput};

fn build_answer_id_camel() -> ton_abi::Token {
    ton_abi::Token::new(
        "answerId",
        ton_abi::TokenValue::Uint(ton_abi::Uint::new(1337, 32)),
    )
}

#[derive(Clone)]
pub struct AppCache {
    pub pool_states: IndexMap<MsgAddressInt, (AccountStuff, String)>,
    token_pairs: HashMap<(MsgAddressInt, MsgAddressInt), Pair>,
    pub tx: RpcClient,
    relations: Vec<MsgAddressInt>,
}

#[derive(Clone)]
struct Pair {
    left: MsgAddressInt,
    right: MsgAddressInt,
    pool: MsgAddressInt,
}

impl AppCache {
    pub fn new(tx: RpcClient) -> Self {
        Self {
            pool_states: Default::default(),
            token_pairs: HashMap::new(),
            tx,
            relations: vec![],
        }
    }

    pub async fn load_states(mut self, pool_addresses: Vec<PairInfo>) -> Self {
        let start = Instant::now();

        let futures = pool_addresses.into_iter().map(|pair| {
            let tx = self.tx.clone();
            async move {
                tx.get_contract_state(&pair.address, None)
                    .await
                    .ok()
                    .flatten()
                    .map(|account| (pair.address, (account.account, pair.filename)))
            }
        });

        self.pool_states = FuturesOrdered::from_iter(futures)
            .filter_map(|x| async move { x })
            .collect()
            .await;
        log::info!(
            "Loaded {} states in {:?}",
            self.pool_states.len(),
            start.elapsed()
        );

        self
    }

    pub fn load_tokens_and_token_pairs(mut self) -> Self {
        let start = Instant::now();
        let mut token_pairs = HashMap::new();

        let answer_id = vec![build_answer_id_camel()];

        let mut relations = Vec::new();

        log::info!("Loading tokens and token pairs");
        log::info!("Pool states: {:?}", self.pool_states.len());

        for (address, account) in self.pool_states.iter() {
            if let Ok(token_pair) = dex_pair().function("getTokenRoots").unwrap().run_local(
                &SimpleClock,
                account.0.clone(),
                &answer_id,
            ) {
                let token_roots: GetTokenRoots = token_pair.tokens.unwrap().unpack().unwrap();

                log::info!(
                    "Token roots: l: {}, r: {}",
                    token_roots.left,
                    token_roots.right
                );

                if relations.is_empty() {
                    relations.push(token_roots.left.clone());
                    relations.push(token_roots.right.clone());
                } else {
                    let last = relations.last().unwrap();

                    if last == &token_roots.left {
                        if !relations.contains(&token_roots.right) {
                            relations.push(token_roots.right.clone());
                        }
                    } else if last == &token_roots.right {
                        if !relations.contains(&token_roots.left) {
                            relations.push(token_roots.left.clone());
                        }
                    } else {
                        // If neither token matches the last one, we need to check if either matches the first one
                        let first = relations.first().unwrap();
                        if first == &token_roots.left {
                            relations.insert(0, token_roots.right.clone());
                        } else if first == &token_roots.right {
                            relations.insert(0, token_roots.left.clone());
                        } else {
                            // If no match, we can't connect this pair to the existing chain
                            log::warn!(
                                "Cannot connect pair {:?} - {:?} to existing chain",
                                token_roots.left,
                                token_roots.right
                            );
                        }
                    }
                }
                log::info!(
                    "Relations: {:?}",
                    relations.iter().map(|x| x.to_string()).collect_vec()
                );

                let pair = Pair {
                    left: token_roots.left.clone(),
                    right: token_roots.right.clone(),
                    pool: address.clone(),
                };

                token_pairs.insert(
                    (token_roots.left.clone(), token_roots.right.clone()),
                    pair.clone(),
                );
                token_pairs.insert((token_roots.right.clone(), token_roots.left.clone()), pair);
            }
        }

        self.token_pairs = token_pairs;
        self.relations = relations;

        log::info!(
            "Loaded {} token pairs in {:?}",
            self.token_pairs.len(),
            start.elapsed()
        );

        self
    }

    pub async fn generate_payloads(
        &self,
        recipients: impl ExactSizeIterator<Item = MsgAddressInt>,
        steps_len: u8,
    ) -> Vec<PayloadGeneratorsData> {
        let routes = self.gen_routes(steps_len as usize, recipients.len());
        let payloads = routes
            .into_iter()
            .zip(recipients)
            .map(|(route, recipient)| {
                let steps = self.route_to_steps(route);
                PayloadInput { steps, recipient }
            });

        futures_util::stream::iter(payloads)
            .map(|input| build_double_side_payloads_data(input, self))
            .buffered(100)
            .collect::<Vec<_>>()
            .await
    }

    fn gen_routes(&self, len: usize, num_routes: usize) -> Vec<Vec<MsgAddressInt>> {
        self.relations
            .iter()
            .cloned()
            .cycle()
            .chunks(len)
            .into_iter()
            .map(|x| x.collect_vec())
            .take(num_routes)
            .collect_vec()
    }

    fn route_to_steps(&self, route: Vec<MsgAddressInt>) -> Vec<StepInput> {
        route
            .windows(2)
            .map(|pair| {
                let from_currency_address = &pair[0];
                let to_currency_address = &pair[1];

                let pair = self
                    .token_pairs
                    .get(&(from_currency_address.clone(), to_currency_address.clone()));

                if pair.is_none() {
                    log::error!(
                        "Pair not found: {:?} {:?}",
                        from_currency_address,
                        to_currency_address
                    );
                    let all_pairs = self
                        .token_pairs
                        .keys()
                        .map(|(a, b)| (a.to_string(), b.to_string()))
                        .collect::<Vec<_>>();
                    println!("{}", serde_json::to_string_pretty(&all_pairs).unwrap());
                }
                let pair = pair.unwrap();

                StepInput {
                    pool_address: pair.pool.clone(),
                    currency_addresses: vec![pair.left.clone(), pair.right.clone()],
                    from_currency_address: from_currency_address.clone(),
                    to_currency_address: to_currency_address.clone(),
                }
            })
            .collect()
    }
}
