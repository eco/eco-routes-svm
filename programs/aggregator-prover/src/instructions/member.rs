use anchor_lang::prelude::*;

use crate::instructions::AggregatorProverError;
use crate::state::Config;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MemberQuery {
    /// Excludes the member's program account.
    pub account_count: u8,
    pub data: Vec<u8>,
}

pub struct Member<'accounts, 'info> {
    pub prover: &'accounts AccountInfo<'info>,
    pub accounts: &'accounts [AccountInfo<'info>],
    pub query: MemberQuery,
}

pub fn member_accounts<'accounts, 'info>(
    config: &Config,
    mut accounts: &'accounts [AccountInfo<'info>],
    queries: Vec<MemberQuery>,
) -> Result<Vec<Member<'accounts, 'info>>> {
    let members = queries
        .into_iter()
        .map(|query| Member::take(&mut accounts, query))
        .collect::<Result<Vec<_>>>()?;

    require!(accounts.is_empty(), AggregatorProverError::InvalidProverSet);
    members.iter().enumerate().try_for_each(|(index, member)| {
        require!(
            member.prover.executable && config.provers.contains(member.prover.key),
            AggregatorProverError::InvalidProver
        );
        require!(
            members[..index]
                .iter()
                .all(|previous| previous.prover.key != member.prover.key),
            AggregatorProverError::InvalidProverSet
        );

        Ok(())
    })?;

    Ok(members)
}

impl<'accounts, 'info> Member<'accounts, 'info> {
    fn take(remaining: &mut &'accounts [AccountInfo<'info>], query: MemberQuery) -> Result<Self> {
        let (prover, accounts) = remaining
            .split_first()
            .ok_or(AggregatorProverError::InvalidProverSet)?;
        let (accounts, rest) = accounts
            .split_at_checked(query.account_count.into())
            .ok_or(AggregatorProverError::InvalidProverSet)?;
        *remaining = rest;

        Ok(Self {
            prover,
            accounts,
            query,
        })
    }
}
