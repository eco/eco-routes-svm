use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};

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
    _origin: u32,
    sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        ctx.accounts.config.is_whitelisted(&sender.into()),
        HyperProverError::InvalidSender,
    );

    let proof_data = ProofData::from_bytes(&payload)?;

    mark_intent_hashes_proven(&ctx, proof_data)?;

    Ok(())
}

fn mark_intent_hashes_proven<'info>(
    ctx: &Context<'_, '_, '_, 'info, Handle<'info>>,
    proof_data: ProofData,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() == proof_data.intent_hashes_claimants.len(),
        HyperProverError::InvalidProof
    );

    ctx.remaining_accounts
        .iter()
        .zip(proof_data.intent_hashes_claimants)
        .try_for_each(|(proof, intent_hash_claimant)| {
            mark_intent_hash_proven(ctx, proof, proof_data.destination, intent_hash_claimant)
        })?;

    Ok(())
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'_, '_, '_, 'info, Handle<'info>>,
    proof: &AccountInfo<'info>,
    destination: u64,
    intent_hash_claimant: IntentHashClaimant,
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = intent_hash_claimant;
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

    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}
