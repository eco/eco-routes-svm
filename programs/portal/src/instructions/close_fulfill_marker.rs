use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::events::FulfillMarkerClosed;
use crate::instructions::PortalError;
use crate::state::FulfillMarker;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseFulfillMarkerArgs {
    pub intent_hash: Bytes32,
}

/// Closing the marker destroys the claimant, and `prove` has no other source
/// for it — an intent closed before it is proven can never be proven, so the
/// solver keeps the fulfillment and never claims the reward. The deadline is
/// necessary but not sufficient: it only retires the double-fulfill guard
/// (`fulfill` requires `route.deadline >= now`), and says nothing about
/// whether the intent has been proven, since `prove` runs against the
/// source-side `reward.deadline`, which is later. Prove-timing is therefore
/// the closer's own liability, which is why only `payer` may close.
#[derive(Accounts)]
#[instruction(args: CloseFulfillMarkerArgs)]
pub struct CloseFulfillMarker<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        close = payer,
        address = FulfillMarker::pda(&args.intent_hash).0 @ PortalError::InvalidFulfillMarker,
        has_one = payer @ PortalError::InvalidFulfillMarkerPayer,
    )]
    pub fulfill_marker: Account<'info, FulfillMarker>,
}

pub fn close_fulfill_marker(
    ctx: Context<CloseFulfillMarker>,
    args: CloseFulfillMarkerArgs,
) -> Result<()> {
    let CloseFulfillMarkerArgs { intent_hash } = args;

    require!(
        ctx.accounts.fulfill_marker.deadline
            < Clock::get()?
                .unix_timestamp
                .try_into()
                .expect("timestamp must fit in u64"),
        PortalError::RouteNotExpired
    );

    emit!(FulfillMarkerClosed::new(
        intent_hash,
        ctx.accounts.payer.key()
    ));

    Ok(())
}
