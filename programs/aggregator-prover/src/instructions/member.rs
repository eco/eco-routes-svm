use anchor_lang::prelude::*;

use crate::instructions::{validate_member, AggregatorProverError};
use crate::state::Config;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MemberQuery {
    /// Excludes the member's program account.
    pub account_count: u8,
    pub data: Vec<u8>,
}

type MemberAccounts<'accounts, 'info, 'query> = (
    &'accounts AccountInfo<'info>,
    &'accounts [AccountInfo<'info>],
    &'query MemberQuery,
);

pub(super) fn member_accounts<'accounts, 'info, 'query>(
    config: &Config,
    accounts: &'accounts [AccountInfo<'info>],
    queries: &'query [MemberQuery],
) -> Result<Vec<MemberAccounts<'accounts, 'info, 'query>>> {
    let (members, remaining) = queries.iter().try_fold(
        (Vec::<MemberAccounts>::new(), accounts),
        |(mut members, remaining), query| {
            let (prover, remaining) = remaining
                .split_first()
                .ok_or(AggregatorProverError::InvalidProverSet)?;
            validate_member(config, prover)?;
            require!(
                members
                    .iter()
                    .all(|(member, _, _)| member.key != prover.key),
                AggregatorProverError::InvalidProverSet
            );
            let (accounts, remaining) = remaining
                .split_at_checked(query.account_count.into())
                .ok_or(AggregatorProverError::InvalidProverSet)?;
            members.push((prover, accounts, query));

            Ok::<_, Error>((members, remaining))
        },
    )?;
    require!(
        remaining.is_empty(),
        AggregatorProverError::InvalidProverSet
    );

    Ok(members)
}
