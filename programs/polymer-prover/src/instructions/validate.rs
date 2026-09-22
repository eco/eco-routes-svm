use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};
use eco_svm_std::CHAIN_ID;

use crate::event::{evm_address_to_bytes32, IntentFulfilledFromSource};
use crate::instructions::PolymerProverError;
use crate::polymer::{self, ValidationResult};
use crate::state::{Config, ProofAccount};

#[event_cpi]
#[derive(Accounts)]
pub struct Validate<'info> {
    /// The relayer. Polymer's cache/result PDAs are derived from this key and
    /// it pays the Proof rent.
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(address = Config::pda().0 @ PolymerProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// CHECK: address is validated; Polymer owns it and checks its seeds in the CPI
    #[account(mut, address = polymer::cache_pda(&authority.key()).0 @ PolymerProverError::InvalidCacheAccount)]
    pub cache_account: UncheckedAccount<'info>,
    /// CHECK: address is validated; owner and discriminator are validated after the CPI
    #[account(mut, address = polymer::result_pda(&authority.key()).0 @ PolymerProverError::InvalidResultAccount)]
    pub result_account: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(address = polymer::internal_pda().0 @ PolymerProverError::InvalidInternalAccount)]
    pub internal: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(executable, address = polymer::POLYMER_PROVER_ID @ PolymerProverError::InvalidPolymerProver)]
    pub polymer_prover_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn validate<'info>(ctx: Context<'info, Validate<'info>>) -> Result<()> {
    polymer::validate_event(
        &ctx.accounts.polymer_prover_program,
        &ctx.accounts.authority,
        &ctx.accounts.cache_account,
        &ctx.accounts.result_account,
        &ctx.accounts.internal,
    )?;

    // Read in the same instruction as the CPI: Polymer overwrites the result on
    // every `validate_event`, so this is this proof's outcome and no other.
    let result = ValidationResult::try_from_account_info(&ctx.accounts.result_account)?;
    if !result.is_valid {
        msg!("polymer: {}", result.error_message);
        return Err(PolymerProverError::PolymerProofInvalid.into());
    }

    require!(
        ctx.accounts
            .config
            .is_whitelisted(&evm_address_to_bytes32(result.emitting_contract)),
        PolymerProverError::InvalidEmittingContract
    );

    let event = IntentFulfilledFromSource::parse(&result.topics, &result.unindexed_data)?;
    require!(
        event.source == CHAIN_ID,
        PolymerProverError::InvalidSourceChain
    );

    let proof_data = ProofData::from_bytes(&event.encoded_proofs)
        .map_err(|_| PolymerProverError::InvalidEventData)?;
    require!(
        proof_data.destination == u64::from(result.chain_id),
        PolymerProverError::InvalidDestinationChain
    );
    require!(
        !proof_data.intent_hashes_claimants.is_empty(),
        PolymerProverError::EmptyProofData
    );

    mark_intent_hashes_proven(&ctx, proof_data)
}

/// One Proof PDA per pair, all in this instruction: there is no partial-batch or
/// resume mode, so the relayer must drain the whole event in one transaction.
/// That bounds pairs-per-event by transaction account locks (64 in total, minus
/// the fixed accounts here, so roughly 55 with an address lookup table and
/// roughly 25 in a legacy transaction) and by the 1.4M CU limit, and above both
/// by Polymer's 3000-byte `unindexed_data` cap (~45 pairs). Keep EVM
/// `Inbox.prove` batches destined for Solana at or below
/// `MAX_INTENTS_PER_PROVE` (24), symmetric with the outbound cap; an oversized
/// event is not lost, `Inbox.prove` can be re-called with a smaller batch, but
/// the Polymer proof already requested for it is wasted.
fn mark_intent_hashes_proven<'info>(
    ctx: &Context<'info, Validate<'info>>,
    proof_data: ProofData,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() == proof_data.intent_hashes_claimants.len(),
        PolymerProverError::InvalidProof
    );

    ctx.remaining_accounts
        .iter()
        .zip(proof_data.intent_hashes_claimants)
        .try_for_each(|(proof, intent_hash_claimant)| {
            mark_intent_hash_proven(ctx, proof, proof_data.destination, intent_hash_claimant)
        })
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'info, Validate<'info>>,
    proof: &AccountInfo<'info>,
    destination: u64,
    intent_hash_claimant: IntentHashClaimant,
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = intent_hash_claimant;
    // The 32-byte claimant is an opaque pubkey by construction: a solver that
    // fulfills a Solana-source intent on EVM must supply a real Solana pubkey,
    // an obligation enforced off-chain before it calls EVM `fulfill`. Any other
    // 32 bytes still record a Proof (blocking `refund`) for an address nobody
    // can spend from. PolymerProver.sol's `claimantBytes >> 160 != 0` skip is
    // bytes32->address narrowing for its own leg, not a validation this side
    // lacks; hyper-prover and local-prover behave the same way.
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, bump) = prover::Proof::pda(&intent_hash, &crate::ID);
    require!(proof.key() == proof_pda, PolymerProverError::InvalidProof);
    let proof_signer_seeds = [PROOF_SEED, intent_hash.as_ref(), &[bump]];

    // A Polymer proof can be re-validated any number of times and a later EVM
    // `prove()` may re-include an already-proven hash, so reaching the recorded
    // state again is a no-op; only a state that disagrees is an error. The
    // event repeats either way: it asserts the recorded state, not a transition.
    match prover::Proof::try_from_account_info(proof)? {
        Some(recorded) => require!(
            recorded.destination == destination && recorded.claimant == claimant,
            PolymerProverError::IntentAlreadyProven
        ),
        None => ProofAccount::from(prover::Proof::new(destination, claimant)).init(
            proof,
            &ctx.accounts.authority,
            &ctx.accounts.system_program,
            &[&proof_signer_seeds],
        )?,
    }

    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}
