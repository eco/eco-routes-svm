use anchor_lang::prelude::*;
use eco_svm_std::prover::{cpi, ValidateProofArgs};

use crate::instructions::{validate_member, AggregatorProverError};
use crate::state::Config;

#[derive(Accounts)]
pub struct ValidateProof<'info> {
    #[account(address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
}

pub fn validate_proof<'info>(
    ctx: Context<'info, ValidateProof<'info>>,
    args: ValidateProofArgs,
) -> Result<bool> {
    if args.claimant.is_some() {
        let (prover, accounts) = ctx
            .remaining_accounts
            .split_first()
            .ok_or(AggregatorProverError::InvalidProver)?;
        validate_member(&ctx.accounts.config, prover)?;

        return cpi::invoke_validate_proof(prover, accounts, args);
    }

    let provers = &ctx.accounts.config.provers;
    require!(
        ctx.remaining_accounts.len() == provers.len() * 2,
        AggregatorProverError::InvalidProverSet
    );
    ctx.remaining_accounts
        .chunks_exact(2)
        .zip(provers)
        .try_for_each(|(accounts, prover)| {
            require_keys_eq!(
                accounts[0].key(),
                *prover,
                AggregatorProverError::InvalidProverSet
            );
            validate_member(&ctx.accounts.config, &accounts[0])
        })?;
    ctx.remaining_accounts
        .chunks_exact(2)
        .try_fold(false, |found, accounts| {
            if found {
                return Ok(true);
            }

            cpi::invoke_validate_proof(&accounts[0], &accounts[1..], args.clone())
        })
}
