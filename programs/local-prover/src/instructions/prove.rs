use anchor_lang::prelude::*;
use eco_svm_std::prover::{self, IntentFulfilled, ProveArgs, PROOF_SEED};
use eco_svm_std::CHAIN_ID;

use crate::instructions::LocalProverError;
use crate::state::ProofAccount;

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: ProveArgs)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda().0 @ LocalProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
    #[account(
        init,
        payer = payer,
        space = 8 + ProofAccount::INIT_SPACE,
        seeds = [PROOF_SEED, args.intent_hash.as_ref()],
        bump,
    )]
    pub proof: Account<'info, ProofAccount>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        source_chain,
        claimant,
        intent_hash,
        ..
    } = args;
    let claimant = Pubkey::new_from_array(claimant.into());

    require!(
        source_chain == CHAIN_ID,
        LocalProverError::InvalidSourceChain
    );

    *ctx.accounts.proof = prover::Proof::new(CHAIN_ID, claimant).into();

    emit_cpi!(IntentFulfilled::new(
        intent_hash,
        claimant.to_bytes().into()
    ));

    Ok(())
}
