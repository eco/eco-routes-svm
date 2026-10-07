use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::instructions::{LayerZeroProverError, CLEAR_ACCOUNTS_LEN};
use crate::layerzero::{self, ClearParams, LzReceiveParams, CLEAR_DISCRIMINATOR, ENDPOINT_ID};
use crate::state::{pda_payer_pda, ProofAccount, Store, PDA_PAYER_SEED, STORE_SEED};

/// Permissionless: authenticity comes from `endpoint::clear` (the payload must
/// match a DVN-verified hash, which it then closes) plus our own peer and
/// chain checks. Remaining accounts: the [`CLEAR_ACCOUNTS_LEN`] clear accounts,
/// then one `Proof` PDA per pair (see `lz_receive_accounts`).
#[event_cpi]
#[derive(Accounts)]
pub struct LzReceive<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn lz_receive<'info>(
    ctx: Context<'info, LzReceive<'info>>,
    params: LzReceiveParams,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() >= CLEAR_ACCOUNTS_LEN,
        LayerZeroProverError::InvalidEndpoint
    );
    let (clear_accounts, proofs) = ctx.remaining_accounts.split_at(CLEAR_ACCOUNTS_LEN);
    let store = ctx.accounts.store.key();
    require_keys_eq!(
        clear_accounts[1].key(),
        store,
        LayerZeroProverError::InvalidStore
    );

    // Clear first, as LayerZero requires: it checks the payload against the
    // DVN-verified PayloadHash and closes that account before any state changes.
    let (_, bump) = Store::pda();
    layerzero::invoke(
        ENDPOINT_ID,
        CLEAR_DISCRIMINATOR,
        &ClearParams {
            receiver: store,
            src_eid: params.src_eid,
            sender: params.sender,
            nonce: params.nonce,
            guid: params.guid,
            message: params.message.clone(),
        },
        clear_accounts,
        &[store],
        &[&[STORE_SEED, &[bump]]],
    )?;

    // The endpoint does not check the peer; this is our trust boundary.
    let peer = *ctx
        .accounts
        .store
        .peer(params.src_eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    require!(
        peer.address == Bytes32::from(params.sender),
        LayerZeroProverError::InvalidSender
    );
    // The endpoint authenticates `src_eid`, so the self-reported header cannot
    // claim a chain other than the peer's (EVM `_handleCrossChainMessage` rule).
    let proof_data = ProofData::from_bytes(&params.message)?;
    let destination = proof_data.destination;
    require!(
        destination == peer.chain_id,
        LayerZeroProverError::ChainIdMismatch
    );
    require!(
        proofs.len() == proof_data.intent_hashes_claimants.len(),
        LayerZeroProverError::InvalidProof
    );

    let (_, payer_bump) = pda_payer_pda();
    proofs
        .iter()
        .zip(proof_data.intent_hashes_claimants)
        .try_for_each(|(proof, pair)| {
            mark_intent_hash_proven(&ctx, proof, payer_bump, destination, pair)
        })
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'info, LzReceive<'info>>,
    proof: &AccountInfo<'info>,
    payer_bump: u8,
    destination: u64,
    pair: IntentHashClaimant,
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = pair;
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, proof_bump) = prover::Proof::pda(&intent_hash, &crate::ID);
    require_keys_eq!(proof.key(), proof_pda, LayerZeroProverError::InvalidProof);

    // A `Proof` can already exist for a duplicate pair earlier in this batch,
    // or from a later message (new nonce, e.g. a re-prove) carrying the same
    // pair. Reaching the recorded state again is a no-op; only a disagreeing
    // state is an error. The event repeats either way.
    match prover::Proof::try_from_account_info(proof)? {
        Some(recorded) => require!(
            recorded.destination == destination && recorded.claimant == claimant,
            LayerZeroProverError::IntentAlreadyProven
        ),
        None => ProofAccount::from(prover::Proof::new(destination, claimant)).init(
            proof,
            &ctx.accounts.pda_payer,
            &ctx.accounts.system_program,
            &[
                &[PDA_PAYER_SEED, &[payer_bump]],
                &[PROOF_SEED, intent_hash.as_ref(), &[proof_bump]],
            ],
        )?,
    }

    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}
