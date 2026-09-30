use anchor_lang::prelude::*;
use eco_svm_std::prover::{cpi, CloseProofArgs};

use crate::instructions::{member_accounts, AggregatorProverError, MemberQuery};
use crate::state::Config;

#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&args.intent_hash).0 @ AggregatorProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
}

pub fn close_proof<'info>(
    ctx: Context<'info, CloseProof<'info>>,
    args: CloseProofArgs,
) -> Result<()> {
    let CloseProofArgs { intent_hash, data } = args;
    let queries = Vec::<MemberQuery>::try_from_slice(&data)?;
    let members = member_accounts(&ctx.accounts.config, ctx.remaining_accounts, &queries)?;
    let (prover, accounts, query) = members
        .first()
        .ok_or(AggregatorProverError::InvalidProverSet)?;

    cpi::close_proof(
        prover,
        &ctx.accounts.portal_proof_closer,
        accounts,
        CloseProofArgs::new(intent_hash, query.data.clone()),
        &[],
    )
}
