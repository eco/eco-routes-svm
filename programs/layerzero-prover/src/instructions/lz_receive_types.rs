use anchor_lang::prelude::*;
use eco_svm_std::event_authority_pda;
use eco_svm_std::prover::{Proof, ProofData};

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    endpoint_event_authority, endpoint_settings_pda, nonce_pda, oapp_registry_pda,
    payload_hash_pda, AccountMetaRef, AddressLocator, LzInstruction, LzReceiveParams,
    LzReceiveTypesInfoResult, LzReceiveTypesV2Accounts, LzReceiveTypesV2Result, ENDPOINT_ID,
    EXECUTION_CONTEXT_VERSION_1, LZ_RECEIVE_TYPES_VERSION,
};
use crate::state::{pda_payer_pda, LzReceiveTypesAccount, Store};

/// Accounts `endpoint::clear` takes, as the head of `lz_receive`'s remaining
/// accounts: endpoint program, receiver (store), OApp registry, nonce, payload
/// hash (mut), endpoint settings (mut), endpoint event authority, endpoint program.
pub const CLEAR_ACCOUNTS_LEN: usize = 8;

/// Executor entry point 1: names the accounts `lz_receive_types_v2` takes.
#[derive(Accounts)]
pub struct LzReceiveTypesInfo<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    #[account(address = LzReceiveTypesAccount::pda().0 @ LayerZeroProverError::InvalidLzReceiveTypes)]
    pub lz_receive_types: Account<'info, LzReceiveTypesAccount>,
}

pub fn lz_receive_types_info(
    ctx: Context<LzReceiveTypesInfo>,
    _params: LzReceiveParams,
) -> Result<LzReceiveTypesInfoResult> {
    Ok(LzReceiveTypesInfoResult {
        version: LZ_RECEIVE_TYPES_VERSION,
        accounts: LzReceiveTypesV2Accounts {
            accounts: vec![ctx.accounts.store.key()],
        },
    })
}

/// Executor entry point 2 (simulated): the exact `lz_receive` instruction to
/// build for this message. Must agree with what `lz_receive` validates, or the
/// executor halts — both use [`lz_receive_accounts`].
#[derive(Accounts)]
pub struct LzReceiveTypesV2<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
}

pub fn lz_receive_types_v2(
    ctx: Context<LzReceiveTypesV2>,
    params: LzReceiveParams,
) -> Result<LzReceiveTypesV2Result> {
    let alt = ctx.accounts.store.alt;
    require!(alt != Pubkey::default(), LayerZeroProverError::AltNotSet);
    let proof_data = ProofData::from_bytes(&params.message)?;

    Ok(LzReceiveTypesV2Result {
        context_version: EXECUTION_CONTEXT_VERSION_1,
        alts: vec![alt],
        instructions: vec![LzInstruction::LzReceive {
            accounts: lz_receive_accounts(&params, &proof_data),
        }],
    })
}

/// `lz_receive`'s account list in order: the named accounts (store,
/// pda_payer, system program, then event_cpi's event authority and program),
/// the [`CLEAR_ACCOUNTS_LEN`] `clear` accounts, then one `Proof` PDA per pair.
pub fn lz_receive_accounts(
    params: &LzReceiveParams,
    proof_data: &ProofData,
) -> Vec<AccountMetaRef> {
    let store = Store::pda().0;
    let fixed = [
        (store, false),
        (pda_payer_pda().0, true),
        (anchor_lang::system_program::ID, false),
        (event_authority_pda(&crate::ID).0, false),
        (crate::ID, false),
        (ENDPOINT_ID, false),
        (store, false),
        (oapp_registry_pda(&store).0, false),
        (nonce_pda(&store, params.src_eid, &params.sender).0, false),
        (
            payload_hash_pda(&store, params.src_eid, &params.sender, params.nonce).0,
            true,
        ),
        (endpoint_settings_pda().0, true),
        (endpoint_event_authority().0, false),
        (ENDPOINT_ID, false),
    ];
    let proofs = proof_data
        .intent_hashes_claimants
        .iter()
        .map(|pair| (Proof::pda(&pair.intent_hash, &crate::ID).0, true));

    fixed
        .into_iter()
        .chain(proofs)
        .map(|(pubkey, is_writable)| AccountMetaRef {
            pubkey: AddressLocator::Address(pubkey),
            is_writable,
        })
        .collect()
}
