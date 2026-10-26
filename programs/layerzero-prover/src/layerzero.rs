//! Hand-rolled mirror of the pieces of LayerZero V2's Solana programs this
//! prover consumes: the Endpoint (OApp registration, path setup, `clear`,
//! `send`, `quote`), ULN302's config PDAs, and the executor's V2
//! `lz_receive_types` ABI. Kept as constants rather than a crate dependency so
//! LayerZero's Anchor 0.29 pin stays out of our build graph, the same way
//! `hyperlane.rs` and `polymer.rs` mirror their bridges.
//!
//! Pinned to LayerZero-Labs/LayerZero-v2@9c741e7f9790639537b1710a203bcdfd73b0b9ac
//! (`packages/layerzero-v2/solana/programs/{endpoint,uln}`, `libs/oapp`).
//! Every u32/u64 seed is big-endian. Discriminators are
//! `sha256("global:<name>")[..8]`, pinned against Anchor-derived values in
//! `integration-tests/tests/close_proof_layerzero_prover.rs`.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::event_authority_pda;

use crate::instructions::LayerZeroProverError;

pub const ENDPOINT_ID: Pubkey = pubkey!("76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6");
pub const ULN_ID: Pubkey = pubkey!("7a4WjyR8VZ7yZz5XJAKm39BUGn5iT9CKcv2pmG9tdXVH");

/// Solana's LayerZero EIDs, for EVM-side configuration and tests; the program
/// never needs its own EID on-chain.
pub const MAINNET_SOLANA_EID: u32 = 30168;
pub const DEVNET_SOLANA_EID: u32 = 40168;

pub const ENDPOINT_SEED: &[u8] = b"Endpoint";
pub const OAPP_SEED: &[u8] = b"OApp";
pub const NONCE_SEED: &[u8] = b"Nonce";
pub const PENDING_NONCE_SEED: &[u8] = b"PendingNonce";
pub const PAYLOAD_HASH_SEED: &[u8] = b"PayloadHash";
pub const SEND_LIBRARY_CONFIG_SEED: &[u8] = b"SendLibraryConfig";
pub const RECEIVE_LIBRARY_CONFIG_SEED: &[u8] = b"ReceiveLibraryConfig";
pub const MESSAGE_LIB_SEED: &[u8] = b"MessageLib";
pub const SEND_CONFIG_SEED: &[u8] = b"SendConfig";
pub const RECEIVE_CONFIG_SEED: &[u8] = b"ReceiveConfig";
/// The executor derives `[LZ_RECEIVE_TYPES_SEED, store]` under the OApp
/// program itself; it must not change.
pub const LZ_RECEIVE_TYPES_SEED: &[u8] = b"LzReceiveTypes";

pub const REGISTER_OAPP_DISCRIMINATOR: [u8; 8] = [129, 89, 71, 68, 11, 82, 210, 125];
pub const INIT_NONCE_DISCRIMINATOR: [u8; 8] = [204, 171, 16, 214, 182, 191, 27, 196];
pub const INIT_SEND_LIBRARY_DISCRIMINATOR: [u8; 8] = [156, 24, 235, 120, 73, 193, 144, 19];
pub const INIT_RECEIVE_LIBRARY_DISCRIMINATOR: [u8; 8] = [197, 114, 81, 100, 45, 233, 36, 230];
pub const SET_SEND_LIBRARY_DISCRIMINATOR: [u8; 8] = [251, 118, 78, 158, 134, 149, 129, 5];
pub const SET_RECEIVE_LIBRARY_DISCRIMINATOR: [u8; 8] = [223, 172, 180, 105, 165, 161, 147, 228];
pub const INIT_CONFIG_DISCRIMINATOR: [u8; 8] = [23, 235, 115, 232, 168, 96, 1, 231];
pub const SET_CONFIG_DISCRIMINATOR: [u8; 8] = [108, 158, 154, 175, 212, 98, 52, 66];
pub const CLEAR_DISCRIMINATOR: [u8; 8] = [250, 39, 28, 213, 123, 163, 133, 5];
pub const SEND_DISCRIMINATOR: [u8; 8] = [102, 251, 20, 187, 65, 75, 12, 69];
pub const QUOTE_DISCRIMINATOR: [u8; 8] = [149, 42, 109, 247, 134, 146, 213, 123];
/// Hard-coded in LayerZero's executor; our `lz_receive` must hash to it.
pub const LZ_RECEIVE_DISCRIMINATOR: [u8; 8] = [8, 179, 120, 109, 33, 118, 189, 80];

pub const CONFIG_TYPE_EXECUTOR: u32 = 1;
pub const CONFIG_TYPE_SEND_ULN: u32 = 2;
pub const CONFIG_TYPE_RECEIVE_ULN: u32 = 3;
/// `UlnConfig` count meaning "none" (as opposed to 0 = "LayerZero default").
pub const NIL_DVN_COUNT: u8 = u8::MAX;
/// UlnConfig confirmations meaning an explicit zero (as opposed to 0 = "LayerZero default").
pub const NIL_CONFIRMATIONS: u64 = u64::MAX;

pub const LZ_RECEIVE_TYPES_VERSION: u8 = 2;
pub const EXECUTION_CONTEXT_VERSION_1: u8 = 1;

pub fn endpoint_settings_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[ENDPOINT_SEED], &ENDPOINT_ID)
}

pub fn oapp_registry_pda(oapp: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[OAPP_SEED, oapp.as_ref()], &ENDPOINT_ID)
}

pub fn nonce_pda(oapp: &Pubkey, eid: u32, remote: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[NONCE_SEED, oapp.as_ref(), &eid.to_be_bytes(), remote],
        &ENDPOINT_ID,
    )
}

pub fn pending_nonce_pda(oapp: &Pubkey, eid: u32, remote: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            PENDING_NONCE_SEED,
            oapp.as_ref(),
            &eid.to_be_bytes(),
            remote,
        ],
        &ENDPOINT_ID,
    )
}

pub fn payload_hash_pda(
    receiver: &Pubkey,
    src_eid: u32,
    sender: &[u8; 32],
    nonce: u64,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            PAYLOAD_HASH_SEED,
            receiver.as_ref(),
            &src_eid.to_be_bytes(),
            sender,
            &nonce.to_be_bytes(),
        ],
        &ENDPOINT_ID,
    )
}

pub fn send_library_config_pda(oapp: &Pubkey, eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SEND_LIBRARY_CONFIG_SEED, oapp.as_ref(), &eid.to_be_bytes()],
        &ENDPOINT_ID,
    )
}

pub fn default_send_library_config_pda(eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SEND_LIBRARY_CONFIG_SEED, &eid.to_be_bytes()],
        &ENDPOINT_ID,
    )
}

pub fn receive_library_config_pda(oapp: &Pubkey, eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RECEIVE_LIBRARY_CONFIG_SEED,
            oapp.as_ref(),
            &eid.to_be_bytes(),
        ],
        &ENDPOINT_ID,
    )
}

/// The endpoint's record for a message library, keyed by the library's
/// `MessageLib` PDA (for ULN302: [`uln_settings_pda`]), not its program ID.
pub fn message_lib_info_pda(message_lib: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MESSAGE_LIB_SEED, message_lib.as_ref()], &ENDPOINT_ID)
}

pub fn endpoint_event_authority() -> (Pubkey, u8) {
    event_authority_pda(&ENDPOINT_ID)
}

/// ULN302's settings account, which is also the address the endpoint uses to
/// name the library (`new_lib`, `message_lib`).
pub fn uln_settings_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MESSAGE_LIB_SEED], &ULN_ID)
}

pub fn uln_send_config_pda(eid: u32, oapp: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SEND_CONFIG_SEED, &eid.to_be_bytes(), oapp.as_ref()],
        &ULN_ID,
    )
}

pub fn uln_receive_config_pda(eid: u32, oapp: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RECEIVE_CONFIG_SEED, &eid.to_be_bytes(), oapp.as_ref()],
        &ULN_ID,
    )
}

pub fn uln_default_send_config_pda(eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SEND_CONFIG_SEED, &eid.to_be_bytes()], &ULN_ID)
}

pub fn uln_default_receive_config_pda(eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RECEIVE_CONFIG_SEED, &eid.to_be_bytes()], &ULN_ID)
}

pub fn uln_event_authority() -> (Pubkey, u8) {
    event_authority_pda(&ULN_ID)
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct RegisterOAppParams {
    pub delegate: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitNonceParams {
    pub local_oapp: Pubkey,
    pub remote_eid: u32,
    pub remote_oapp: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SetSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SetReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
    pub grace_period: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SetConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
    pub config_type: u32,
    /// Plain Borsh of the inner `UlnConfig` / `ExecutorConfig`, no enum tag.
    pub config: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct ClearParams {
    pub receiver: Pubkey,
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
    pub message: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SendParams {
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct QuoteParams {
    pub sender: Pubkey,
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub pay_in_lz_token: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq)]
pub struct MessagingFee {
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveParams {
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
    pub message: Vec<u8>,
    pub extra_data: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct UlnConfig {
    pub confirmations: u64,
    pub required_dvn_count: u8,
    pub optional_dvn_count: u8,
    pub optional_dvn_threshold: u8,
    pub required_dvns: Vec<Pubkey>,
    pub optional_dvns: Vec<Pubkey>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct ExecutorConfig {
    pub max_message_size: u32,
    pub executor: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveTypesV2Accounts {
    pub accounts: Vec<Pubkey>,
}

/// Borsh-identical to LayerZero's `(u8, LzReceiveTypesV2Accounts)` tuple
/// return; a named struct so the IDL builder can describe it.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveTypesInfoResult {
    pub version: u8,
    pub accounts: LzReceiveTypesV2Accounts,
}

/// Variant order is the Borsh tag and part of LayerZero's ABI.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub enum AddressLocator {
    Address(Pubkey),
    AltIndex(u8, u8),
    Payer,
    Signer(u8),
    Context,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct AccountMetaRef {
    pub pubkey: AddressLocator,
    pub is_writable: bool,
}

/// LayerZero's `Instruction` enum, renamed to avoid the Solana type.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub enum LzInstruction {
    LzReceive {
        accounts: Vec<AccountMetaRef>,
    },
    Standard {
        program_id: Pubkey,
        accounts: Vec<AccountMetaRef>,
        data: Vec<u8>,
    },
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveTypesV2Result {
    pub context_version: u8,
    pub alts: Vec<Pubkey>,
    pub instructions: Vec<LzInstruction>,
}

/// Type-3 executor options carrying one gas-only `lzReceive` option:
/// `u16 type=3 ‖ u8 worker=1 ‖ u16 size=17 ‖ u8 option=1 ‖ u128 gas` (all BE).
pub fn lz_receive_options(gas: u128) -> Vec<u8> {
    [
        3u16.to_be_bytes().as_slice(),
        &[1u8],
        &17u16.to_be_bytes(),
        &[1u8],
        &gas.to_be_bytes(),
    ]
    .concat()
}

/// CPIs `program_id` with `discriminator ‖ borsh(params)`. `accounts[0]` must
/// be the callee program (it is handed to the runtime but not listed); the rest
/// become the instruction's account metas in order, keeping each account's
/// writable flag. `signers` are PDAs this program signs for with
/// `signer_seeds`; every other account keeps the signer flag it arrived with.
pub fn invoke<'info>(
    program_id: Pubkey,
    discriminator: [u8; 8],
    params: &impl AnchorSerialize,
    accounts: &[AccountInfo<'info>],
    signers: &[Pubkey],
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let (program, metas) = accounts
        .split_first()
        .ok_or(LayerZeroProverError::InvalidEndpoint)?;
    require_keys_eq!(
        program.key(),
        program_id,
        LayerZeroProverError::InvalidEndpoint
    );

    let data = discriminator
        .into_iter()
        .chain(anchor_lang::prelude::borsh::to_vec(params)?)
        .collect();
    let metas = metas
        .iter()
        .map(|account| AccountMeta {
            pubkey: account.key(),
            is_signer: account.is_signer || signers.contains(account.key),
            is_writable: account.is_writable,
        })
        .collect();

    invoke_signed(
        &Instruction {
            program_id,
            accounts: metas,
            data,
        },
        accounts,
        signer_seeds,
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz_receive_options_gas_only_layout() {
        let expected = "00030100110100000000000000000000000000030d40";
        let actual: String = lz_receive_options(200_000)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn endpoint_pdas_deterministic() {
        let oapp = Pubkey::new_from_array([7; 32]);
        let remote = [9u8; 32];
        goldie::assert_json!(vec![
            endpoint_settings_pda(),
            oapp_registry_pda(&oapp),
            nonce_pda(&oapp, 30184, &remote),
            pending_nonce_pda(&oapp, 30184, &remote),
            payload_hash_pda(&oapp, 30184, &remote, 1),
            send_library_config_pda(&oapp, 30184),
            default_send_library_config_pda(30184),
            receive_library_config_pda(&oapp, 30184),
            message_lib_info_pda(&uln_settings_pda().0),
            endpoint_event_authority(),
        ]);
    }

    #[test]
    fn uln_pdas_deterministic() {
        let oapp = Pubkey::new_from_array([7; 32]);
        goldie::assert_json!(vec![
            uln_settings_pda(),
            uln_send_config_pda(30184, &oapp),
            uln_receive_config_pda(30184, &oapp),
            uln_default_send_config_pda(30184),
            uln_default_receive_config_pda(30184),
            uln_event_authority(),
        ]);
    }
}
