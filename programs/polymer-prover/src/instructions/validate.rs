use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};
use eco_svm_std::CHAIN_ID;

use crate::event::{self, evm_address_to_bytes32};
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
    // every `validate_event`, so this is this proof's outcome and no other. A
    // `validate` without a fresh `load_proof` aborts inside Polymer's frame
    // rather than replaying the previous result; pinned by
    // `validate_twice_without_reload_fail`.
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

    // Topics first, payload second: a wrong source chain is reported as such
    // even when the payload is also malformed (spec section 3.4, steps 3-4).
    let source = event::parse_source(&result.topics)?;
    require!(source == CHAIN_ID, PolymerProverError::InvalidSourceChain);

    let encoded_proofs = event::decode_encoded_proofs(&result.unindexed_data)?;
    let proof_data =
        ProofData::from_bytes(&encoded_proofs).map_err(|_| PolymerProverError::InvalidEventData)?;
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

/// Largest batch one legacy (no address lookup table) `validate` transaction
/// can carry. The serialized transaction is 450 + 33N bytes (1 signature, 10
/// distinct fixed keys — the 9 `Validate` accounts plus ComputeBudget — then 32
/// bytes of key and 1 byte of index per pair) against the 1232-byte packet:
/// N = 23 fits at 1209, N = 24 is 1242 and is rejected before it reaches the
/// runtime. Above 23, relayers need a v0 transaction with an address lookup
/// table. Pinned by `validate_legacy_transaction_fits_only_23_pairs`.
pub const MAX_PAIRS_PER_VALIDATE_LEGACY_TX: usize = 23;

/// One Proof PDA per pair, all in this instruction: there is no partial-batch or
/// resume mode, so the relayer must drain the whole event in one transaction.
/// Duplicate pairs within one event are permitted and rely on Solana aliasing
/// duplicate accounts in one instruction: the second occurrence reads the Proof
/// the first one wrote and takes the no-op (or `IntentAlreadyProven`) branch.
///
/// That bounds pairs-per-event. The limits, in the order they bind (all pinned
/// in `validate_polymer_prover.rs`):
///
/// 1. The 64-entry instruction trace (`MAX_INSTRUCTION_TRACE_LENGTH`) is the
///    real ceiling: 3 fixed entries (ComputeBudget, `validate`, the
///    `validate_event` CPI) plus 2 per fresh pair (the system `create_account`
///    inside `AccountExt::init` and `emit_cpi!`), so **30 fresh pairs**. A Proof
///    PDA that was pre-funded takes `create_account`'s griefing-resistant
///    `transfer + allocate + assign` path (4 entries per pair when under the
///    rent-exempt minimum, 3 when at or above it), dropping the ceiling to 15 or
///    20 — so an adversary can push a 24-pair batch over the ceiling for a few
///    thousand lamports per account (3 + 2(24 - k) + 4k <= 64 gives k <= 6).
/// 2. A legacy transaction's 1232-byte packet caps it at
///    [`MAX_PAIRS_PER_VALIDATE_LEGACY_TX`] (23) pairs; 24 pairs is 1242 bytes.
///    An inbound batch at `MAX_INTENTS_PER_PROVE` therefore needs a v0
///    transaction with an address lookup table; an ALT-free relayer stays at 23.
/// 3. Compute: measured ~252k CU at 24 pairs through the mock, so the 1.4M
///    transaction limit is not binding but the 200k default is — callers must
///    raise it (`Polymer`'s real `validate_event` sits on top of that; see
///    `polymer_prover_context.rs::VALIDATE_COMPUTE_UNIT_LIMIT`).
/// 4. Account locks (64): 10 fixed keys plus N, so 54 pairs — never binding.
/// 5. Polymer's 3000-byte `unindexed_data` cap: 1632 bytes at 24 pairs (the
///    64-byte ABI header plus `ceil32(8 + 64N)`), ~45 pairs — never binding.
///
/// Operational guidance: keep EVM `Inbox.prove` batches destined for Solana at
/// or below `MAX_INTENTS_PER_PROVE` (24), symmetric with the outbound cap, and
/// deliver a 24-pair event with a v0 transaction plus ALT (a legacy transaction
/// tops out at 23 pairs). An oversized or griefed event is not lost: treat
/// `MaxInstructionTraceLengthExceeded` as "re-prove with a smaller batch" —
/// `Inbox.prove` can be re-called — but the Polymer proof already requested
/// for it is wasted.
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
