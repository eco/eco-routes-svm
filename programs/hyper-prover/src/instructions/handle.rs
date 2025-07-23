use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashesClaimants, IntentProven, PROOF_SEED};
use eco_svm_std::{Bytes32, CHAIN_ID};

use crate::hyperlane::process_authority_pda;
use crate::instructions::HyperProverError;
use crate::state::{pda_payer_pda, Config, ProofAccount, PDA_PAYER_SEED};

#[event_cpi]
#[derive(Accounts)]
pub struct Handle<'info> {
    #[account(address = process_authority_pda().0 @ HyperProverError::InvalidProcessAuthority)]
    pub process_authority: Signer<'info>,
    #[account(address = Config::pda().0 @ HyperProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(mut)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn handle<'info>(
    ctx: Context<'_, '_, '_, 'info, Handle<'info>>,
    origin: u32,
    sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        ctx.accounts.config.is_whitelisted(&sender.into()),
        HyperProverError::InvalidSender,
    );

    let destination = domain_to_chain(origin);
    let intent_hashes_claimants = IntentHashesClaimants::from_bytes(&payload)?;

    mark_intent_hashes_proven(&ctx, destination, intent_hashes_claimants.into())?;

    Ok(())
}

// TODO: We need to maintain a map here later for the transformation.
// This works now only because we use Hyperlane's domain IDs directly as chain IDs.
fn domain_to_chain(chain: u32) -> u64 {
    chain.into()
}

fn mark_intent_hashes_proven<'info>(
    ctx: &Context<'_, '_, '_, 'info, Handle<'info>>,
    destination: u64,
    intent_hashes_claimants: Vec<(Bytes32, Bytes32)>,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() == intent_hashes_claimants.len(),
        HyperProverError::InvalidProof
    );

    ctx.remaining_accounts
        .iter()
        .zip(intent_hashes_claimants)
        .try_for_each(|(proof, intent_hash_claimant)| {
            mark_intent_hash_proven(ctx, proof, destination, intent_hash_claimant)
        })?;

    Ok(())
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'_, '_, '_, 'info, Handle<'info>>,
    proof: &AccountInfo<'info>,
    destination: u64,
    intent_hash_claimant: (Bytes32, Bytes32),
) -> Result<()> {
    let (intent_hash, claimant) = intent_hash_claimant;
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, bump) = prover::Proof::pda(&intent_hash, &crate::ID);
    require!(proof.key() == proof_pda, HyperProverError::InvalidProof);
    let proof_signer_seeds = [PROOF_SEED, intent_hash.as_ref(), &[bump]];

    let (pda_payer_pda, bump) = pda_payer_pda();
    require!(
        ctx.accounts.pda_payer.key() == pda_payer_pda,
        HyperProverError::InvalidPdaPayer
    );
    let pda_payer_signer_seeds = [PDA_PAYER_SEED, &[bump]];

    ProofAccount::from(prover::Proof::new(destination, claimant))
        .init(
            proof,
            &ctx.accounts.pda_payer,
            &ctx.accounts.system_program,
            &[&pda_payer_signer_seeds, &proof_signer_seeds],
        )
        .map_err(|_| HyperProverError::IntentAlreadyProven)?;

    emit_cpi!(IntentProven::new(intent_hash, CHAIN_ID, destination));

    Ok(())
}
