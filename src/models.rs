use ed25519_dalek::Keypair;
use nekoton_abi::{PackAbi, PackAbiPlain, UnpackAbiPlain};
use nekoton_utils::serde_address;
use serde::{Deserialize, Serialize};
use ton_block::MsgAddressInt;
use ton_types::{BuilderData, Cell};

#[derive(Debug, Clone, PackAbiPlain)]
pub struct DexPairV9BuildExchangePayloadV2 {
    #[abi(name = "_id", uint64)]
    pub id: u64,
    #[abi(name = "_deployWalletGrams", uint128)]
    pub deploy_wallet_grams: u128,
    #[abi(name = "_expectedAmount", uint128)]
    pub expected_amount: u128,
    #[abi(name = "_recipient", address)]
    pub recipient: MsgAddressInt,
    #[abi(name = "_referrer", address)]
    pub referrer: MsgAddressInt,
    #[abi(name = "_successPayload")]
    pub success_payload: Option<ton_types::Cell>,
    #[abi(name = "_cancelPayload")]
    pub cancel_payload: Option<ton_types::Cell>,
    #[abi(name = "_toNative")]
    pub to_native: Option<bool>,
}

#[derive(PackAbiPlain, UnpackAbiPlain, Debug, Clone)]
pub struct Transfer {
    #[abi]
    pub amount: u128,
    #[abi]
    pub recipient: MsgAddressInt,
    #[abi(name = "deployWalletValue")]
    pub deploy_wallet_value: u128,
    #[abi(name = "remainingGasTo")]
    pub remaining_gas_to: MsgAddressInt,
    #[abi]
    pub notify: bool,
    #[abi]
    pub payload: Cell,
}

pub struct PayloadInput {
    pub steps: Vec<StepInput>,
    pub recipient: MsgAddressInt,
    pub id: u64,
}

#[derive(Clone)]
pub struct StepInput {
    pub pool_address: MsgAddressInt,
    pub currency_addresses: Vec<MsgAddressInt>,
    pub from_currency_address: MsgAddressInt,
    pub to_currency_address: MsgAddressInt,
}

#[derive(UnpackAbiPlain, Debug, Clone)]
pub struct GetTokenRoots {
    #[abi(address)]
    pub left: MsgAddressInt,
    #[abi(address)]
    pub right: MsgAddressInt,
    #[abi(address)]
    pub lp: MsgAddressInt,
}

#[derive(Debug, Clone, PackAbiPlain)]
pub struct DexPairV9BuildCrossPairExchangePayloadV2 {
    #[abi(name = "_id", uint64)]
    pub id: u64,
    #[abi(name = "_deployWalletGrams", uint128)]
    pub deploy_wallet_grams: u128,
    #[abi(name = "_expectedAmount", uint128)]
    pub expected_amount: u128,
    #[abi(name = "_outcoming", address)]
    pub outcoming: ton_block::MsgAddressInt,
    #[abi(name = "_nextStepIndices", array)]
    pub next_step_indices: Vec<u32>,
    #[abi(name = "_steps", array)]
    pub steps: Vec<DexPairV9Steps>,
    #[abi(name = "_recipient", address)]
    pub recipient: ton_block::MsgAddressInt,
    #[abi(name = "_referrer", address)]
    pub referrer: ton_block::MsgAddressInt,
    #[abi(name = "_successPayload")]
    pub success_payload: Option<ton_types::Cell>,
    #[abi(name = "_cancelPayload")]
    pub cancel_payload: Option<ton_types::Cell>,
}

#[derive(Debug, Clone, PackAbi, nekoton_abi::KnownParamType)]
pub struct DexPairV9Steps {
    #[abi(uint128)]
    pub amount: u128,
    #[abi(array)]
    pub roots: Vec<ton_block::MsgAddressInt>,
    #[abi(address)]
    pub outcoming: ton_block::MsgAddressInt,
    #[abi(uint128)]
    pub numerator: u128,
    #[abi(name = "nextStepIndices", array)]
    pub next_step_indices: Vec<u32>,
}

pub struct PayloadMeta {
    pub forward_route: RouteMeta,
    pub backward_route: RouteMeta,
}

pub struct RouteMeta {
    pub payload: BuilderData,
    pub first_pool_address: MsgAddressInt,
}

pub struct SendData {
    pub payload_meta: PayloadMeta,
    pub signer: Keypair,
    pub sender_addr: MsgAddressInt,
}

impl SendData {
    pub fn new(payload_meta: PayloadMeta, signer: Keypair, sender_addr: MsgAddressInt) -> Self {
        Self {
            payload_meta,
            signer,
            sender_addr,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct CreateAccountParams {
    pub nonce: u32,
}

#[derive(Serialize, Deserialize)]
pub struct EverWalletInfo {
    #[serde(rename = "createAccountParams")]
    pub create_account_params: CreateAccountParams,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GenericDeploymentInfo {
    #[serde(with = "serde_address")]
    pub address: MsgAddressInt,
}
