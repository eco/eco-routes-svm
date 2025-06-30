use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::events::IntentFulfilled;
use crate::hyperlane::process_authority_pda;
use crate::instructions::HyperProverError;
use crate::state::{pda_payer_pda, Config, ProofAccount, PDA_PAYER_SEED};
use crate::utils::claimant_and_intent_hash;

#[derive(Accounts)]
pub struct Handle<'info> {
    #[account(address = process_authority_pda().0 @ HyperProverError::InvalidProcessAuthority)]
    pub process_authority: Signer<'info>,
    #[account(address = Config::pda().0 @ HyperProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// CHECK: address is validated
    #[account(mut)]
    pub proof: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(mut)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn handle(ctx: Context<Handle>, origin: u32, sender: [u8; 32], payload: Vec<u8>) -> Result<()> {
    require!(
        ctx.accounts.config.is_whitelisted(&sender.into()),
        HyperProverError::InvalidSender,
    );

    let destination_chain = domain_to_chain(origin);
    let (claimant, intent_hash) = claimant_and_intent_hash(payload)?;

    mark_proven(&ctx, destination_chain, &claimant, &intent_hash)?;

    emit!(IntentFulfilled::new(
        intent_hash,
        claimant.to_bytes().into()
    ));

    Ok(())
}

// TODO: We need to maintain a map here later for the transformation.
// This works now only because we use Hyperlane's domain IDs directly as chain IDs.
fn domain_to_chain(chain: u32) -> u64 {
    chain.into()
}

fn mark_proven(
    ctx: &Context<Handle>,
    destination_chain: u64,
    claimant: &Pubkey,
    intent_hash: &Bytes32,
) -> Result<()> {
    let (proof_pda, bump) = prover::Proof::pda(intent_hash, &crate::ID);
    require!(
        ctx.accounts.proof.key() == proof_pda,
        HyperProverError::InvalidProof
    );
    let proof_signer_seeds = [PROOF_SEED, intent_hash.as_ref(), &[bump]];

    let (pda_payer_pda, bump) = pda_payer_pda();
    require!(
        ctx.accounts.pda_payer.key() == pda_payer_pda,
        HyperProverError::InvalidPdaPayer
    );
    let pda_payer_signer_seeds = [PDA_PAYER_SEED, &[bump]];

    ProofAccount::from(prover::Proof::new(destination_chain, *claimant))
        .init(
            &ctx.accounts.proof,
            &ctx.accounts.pda_payer,
            &ctx.accounts.system_program,
            &[&pda_payer_signer_seeds, &proof_signer_seeds],
        )
        .map_err(|_| HyperProverError::IntentAlreadyProven.into())
}
