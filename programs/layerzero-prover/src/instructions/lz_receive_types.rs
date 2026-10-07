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
use crate::state::{pda_payer_pda, LzReceiveTypesAccount, Store, PDA_PAYER_SEED, STORE_SEED};

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

/// The static `lz_receive` accounts the executor resolves through the lookup
/// table `lz_receive_types_v2` returns: every account of
/// [`lz_receive_accounts`] that exists before the message does, i.e. all but
/// the per-message `PayloadHash` and `Proof` PDAs, with the Nonce of every
/// configured peer. `set_alt` refuses a table that lacks any of them, because
/// the delivery transaction only fits `MAX_PAIRS_PER_MESSAGE` pairs when these
/// ride in the table rather than as static keys.
pub fn required_alt_addresses(store: &Store) -> Vec<Pubkey> {
    required_alt_addresses_for(&crate::ID, store)
}

/// [`required_alt_addresses`] for a deployment at `program`, which differs from
/// the compiled-in ID when the release deploys to another address.
pub fn required_alt_addresses_for(program: &Pubkey, store: &Store) -> Vec<Pubkey> {
    let store_key = Pubkey::find_program_address(&[STORE_SEED], program).0;
    let pda_payer = Pubkey::find_program_address(&[PDA_PAYER_SEED], program).0;
    let fixed = [
        store_key,
        pda_payer,
        anchor_lang::system_program::ID,
        event_authority_pda(program).0,
        *program,
        ENDPOINT_ID,
        oapp_registry_pda(&store_key).0,
        endpoint_settings_pda().0,
        endpoint_event_authority().0,
    ];
    let nonces = store
        .peers
        .iter()
        .map(|peer| nonce_pda(&store_key, peer.eid, &peer.address).0);

    fixed.into_iter().chain(nonces).collect()
}

#[cfg(test)]
mod tests {
    use eco_svm_std::prover::IntentHashClaimant;

    use super::*;
    use crate::state::Peer;

    /// Everything `lz_receive_accounts` lists except the per-message
    /// `PayloadHash` and `Proof` PDAs must be in the required table contents.
    #[test]
    fn required_alt_addresses_cover_every_static_lz_receive_account() {
        let peer = Peer {
            eid: 30184,
            address: [0xba; 32].into(),
            chain_id: 8453,
        };
        let store = Store::new(vec![peer]).unwrap();
        let params = LzReceiveParams {
            src_eid: peer.eid,
            sender: peer.address.into(),
            nonce: 1,
            guid: [1; 32],
            message: vec![],
            extra_data: vec![],
        };
        let proof_data = ProofData::new(
            8453,
            vec![IntentHashClaimant::new([1; 32].into(), [2; 32].into())],
        );
        let per_message = [
            payload_hash_pda(&Store::pda().0, peer.eid, &params.sender, params.nonce).0,
            Proof::pda(&[1; 32].into(), &crate::ID).0,
        ];
        let required = required_alt_addresses(&store);

        lz_receive_accounts(&params, &proof_data)
            .into_iter()
            .map(|meta| match meta.pubkey {
                AddressLocator::Address(pubkey) => pubkey,
                other => panic!("unexpected locator {other:?}"),
            })
            .filter(|pubkey| !per_message.contains(pubkey))
            .for_each(|pubkey| assert!(required.contains(&pubkey), "{pubkey} missing"));
    }

    #[test]
    fn required_alt_addresses_for_the_compiled_id_equals_required_alt_addresses() {
        let store = Store::new(vec![Peer {
            eid: 30184,
            address: [0xba; 32].into(),
            chain_id: 8453,
        }])
        .unwrap();

        assert_eq!(
            required_alt_addresses_for(&crate::ID, &store),
            required_alt_addresses(&store)
        );
    }

    #[test]
    fn required_alt_addresses_for_another_program_use_that_programs_own_addresses() {
        let store = Store::new(vec![Peer {
            eid: 30184,
            address: [0xba; 32].into(),
            chain_id: 8453,
        }])
        .unwrap();
        let other = Pubkey::new_from_array([7; 32]);
        let own = required_alt_addresses(&store);

        let foreign = required_alt_addresses_for(&other, &store);

        assert!(foreign.contains(&Pubkey::find_program_address(&[STORE_SEED], &other).0));
        assert!(foreign.contains(&other));
        assert!(!foreign.contains(&crate::ID));
        assert!(!foreign.contains(&Store::pda().0));
        assert_eq!(foreign.len(), own.len());
    }
}
