use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{IntentProven, Proof, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::instructions::AggregatorProverError;
use crate::state::{Config, ProofAccount};

#[event_cpi]
#[derive(Accounts)]
#[instruction(intent_hash: Bytes32)]
pub struct Aggregate<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// CHECK: the executable program must belong to the configured prover set.
    #[account(executable, constraint = config.provers.contains(&prover.key()) @ AggregatorProverError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
    /// CHECK: canonical prover-owned PDA; its proof data is validated in the handler.
    #[account(address = Proof::pda(&intent_hash, &prover.key()).0 @ AggregatorProverError::InvalidProof, owner = prover.key() @ AggregatorProverError::InvalidProof)]
    pub prover_proof: UncheckedAccount<'info>,
    /// CHECK: canonical aggregate PDA, created or checked in the handler.
    #[account(mut, address = Proof::pda(&intent_hash, &crate::ID).0 @ AggregatorProverError::InvalidProof)]
    pub proof: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn aggregate<'info>(ctx: Context<'info, Aggregate<'info>>, intent_hash: Bytes32) -> Result<()> {
    let prover_proof = read_proof(&ctx.accounts.prover_proof)?;
    let destination = prover_proof.destination;
    let claimant = prover_proof.claimant;
    let (_, bump) = Proof::pda(&intent_hash, &crate::ID);
    let proof = &ctx.accounts.proof;

    if proof.owner == &crate::ID {
        let recorded = ProofAccount::try_deserialize(&mut &proof.try_borrow_data()?[..])?;
        require!(
            recorded.0.destination == prover_proof.destination
                && recorded.0.claimant == prover_proof.claimant,
            AggregatorProverError::IntentAlreadyProven
        );
    } else {
        ProofAccount(prover_proof).init(
            proof,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &[&[PROOF_SEED, intent_hash.as_ref(), &[bump]]],
        )?;
    }
    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}

fn read_proof(account: &AccountInfo) -> Result<Proof> {
    require!(
        account
            .try_borrow_data()?
            .starts_with(ProofAccount::DISCRIMINATOR),
        AggregatorProverError::InvalidProof
    );
    let proof = Proof::try_from_account_info(account)
        .map_err(|_| AggregatorProverError::InvalidProof)?
        .ok_or(AggregatorProverError::InvalidProof)?;
    require!(
        proof.claimant != Pubkey::default(),
        AggregatorProverError::InvalidProof
    );

    Ok(proof)
}
