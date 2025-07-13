use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{IntentProven, Proof, ProveArgs, PROOF_SEED};
use eco_svm_std::{Bytes32, CHAIN_ID};

use crate::instructions::LocalProverError;
use crate::state::ProofAccount;

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: ProveArgs)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda().0 @ LocalProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn prove_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
    args: ProveArgs,
) -> Result<()> {
    let ProveArgs {
        source,
        intent_hashes_claimants,
        ..
    } = args;

    require!(source == CHAIN_ID, LocalProverError::InvalidSource);

    mark_intent_hashes_proven(&ctx, intent_hashes_claimants.into())?;

    Ok(())
}

fn mark_intent_hashes_proven<'info>(
    ctx: &Context<'_, '_, '_, 'info, Prove<'info>>,
    intent_hashes_claimants: Vec<(Bytes32, Bytes32)>,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() == intent_hashes_claimants.len(),
        LocalProverError::InvalidProof
    );

    ctx.remaining_accounts
        .iter()
        .zip(intent_hashes_claimants)
        .try_for_each(|(proof, intent_hash_claimant)| {
            mark_intent_hash_proven(ctx, proof, intent_hash_claimant)
        })?;

    Ok(())
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'_, '_, '_, 'info, Prove<'info>>,
    proof: &AccountInfo<'info>,
    intent_hash_claimant: (Bytes32, Bytes32),
) -> Result<()> {
    let (intent_hash, claimant) = intent_hash_claimant;
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, bump) = Proof::pda(&intent_hash, &crate::ID);
    require!(proof.key == &proof_pda, LocalProverError::InvalidProof);
    let signer_seeds = [PROOF_SEED, intent_hash.as_ref(), &[bump]];

    ProofAccount::from(Proof::new(CHAIN_ID, claimant))
        .init(
            proof,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &[&signer_seeds],
        )
        .map_err(|_| LocalProverError::IntentAlreadyProven)?;

    emit_cpi!(IntentProven::new(intent_hash, CHAIN_ID, CHAIN_ID));

    Ok(())
}
