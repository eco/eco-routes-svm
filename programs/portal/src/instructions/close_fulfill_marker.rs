use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::events::FulfillMarkerClosed;
use crate::instructions::{now, PortalError};
use crate::state::{FulfillMarker, FULFILL_MARKER_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseFulfillMarkerArgs {
    pub intent_hash: Bytes32,
}

/// Reclaims a [`FulfillMarker`]'s rent to the payer that funded it.
///
/// # Closing before the reward is settled destroys it
///
/// The marker holds the claimant, and `prove` has no other source for it, so a
/// closed marker is an intent that can never be proven — the solver keeps the
/// fulfillment and never claims the reward.
///
/// The deadline gate here retires the *double-fulfill* guard and nothing else.
/// The gate is `route.deadline` — the marker stores it because `fulfill` sees
/// only the route and an opaque `reward_hash`, so it is the sole deadline
/// available there — and `fulfill` requires `route.deadline >= now`, so past it
/// no second fulfill can land whether or not the marker exists.
///
/// It says nothing about whether the reward has been collected. `route.deadline`
/// is conventionally the earlier of the two deadlines and nothing on-chain
/// relates them, so it routinely passes while the reward is still provable and
/// withdrawable. Gating on the later `reward.deadline` would not fix that: it
/// makes the reward *refundable*, not refunded — the creator may never call
/// `refund`, and the solver can still prove and withdraw until they do. No
/// deadline implies settlement, because `withdraw` is not time-gated at all and
/// `refund` refuses outright while a `Proof` exists
/// (`IntentFulfilledAndNotWithdrawn`), so a proven intent stays claimable
/// indefinitely.
///
/// The only sound signal is a terminal source-chain state for the intent —
/// withdrawn, or refunded. That is off-chain knowledge, which is why the
/// authority is the payer: prove-timing is the closer's own liability.
///
/// # The payer must be solver-controlled
///
/// `payer` is a signer distinct from `solver` on both fulfill paths, and this
/// instruction makes it the sole authority able to destroy an unproven claim,
/// as well as the address the rent returns to. A sponsored or ephemeral
/// fee-payer would therefore hold unilateral power over the solver's reward,
/// and a rotated or discarded one strands the rent permanently. The codebase
/// already assumes `payer` is the solver/caller rather than a sponsored
/// relayer (see the `portal_program` CHECK note in `flash_fulfill`); this
/// instruction makes that assumption load-bearing.
#[derive(Accounts)]
#[instruction(args: CloseFulfillMarkerArgs)]
pub struct CloseFulfillMarker<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    // `bump = fulfill_marker.bump` re-derives with `create_program_address`
    // rather than searching, which is what the stored bump is for.
    #[account(
        mut,
        close = payer,
        seeds = [FULFILL_MARKER_SEED, args.intent_hash.as_ref()],
        bump = fulfill_marker.bump,
        has_one = payer @ PortalError::InvalidFulfillMarkerPayer,
    )]
    pub fulfill_marker: Account<'info, FulfillMarker>,
}

pub fn close_fulfill_marker(
    ctx: Context<CloseFulfillMarker>,
    args: CloseFulfillMarkerArgs,
) -> Result<()> {
    let CloseFulfillMarkerArgs { intent_hash } = args;
    let FulfillMarker {
        claimant, deadline, ..
    } = *ctx.accounts.fulfill_marker;

    require!(deadline < now()?, PortalError::RouteNotExpired);

    emit!(FulfillMarkerClosed::new(
        intent_hash,
        ctx.accounts.payer.key(),
        claimant,
        ctx.accounts.fulfill_marker.get_lamports(),
    ));

    Ok(())
}
