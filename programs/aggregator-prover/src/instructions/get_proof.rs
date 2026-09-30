use anchor_lang::prelude::*;
use eco_svm_std::prover::{cpi, GetProofArgs, Proof};

use crate::instructions::{member_accounts, AggregatorProverError, MemberQuery};
use crate::state::Config;

#[derive(Accounts)]
pub struct GetProof<'info> {
    #[account(address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
}

pub fn get_proof<'info>(
    ctx: Context<'info, GetProof<'info>>,
    args: GetProofArgs,
) -> Result<Option<Proof>> {
    let GetProofArgs { intent_hash, data } = args;
    let queries = Vec::<MemberQuery>::try_from_slice(&data)?;
    require!(
        queries.len() == ctx.accounts.config.provers.len(),
        AggregatorProverError::InvalidProverSet
    );
    let members = member_accounts(&ctx.accounts.config, ctx.remaining_accounts, &queries)?;

    members
        .into_iter()
        .try_fold(None, |found, (prover, accounts, query)| {
            if found.is_some() {
                return Ok(found);
            }

            cpi::get_proof(
                prover,
                accounts,
                GetProofArgs::new(intent_hash, query.data.clone()),
            )
        })
}
