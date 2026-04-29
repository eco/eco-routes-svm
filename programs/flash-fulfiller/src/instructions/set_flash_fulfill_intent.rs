use anchor_lang::prelude::*;
use eco_svm_std::CHAIN_ID;
use portal::types::{intent_hash, Reward, Route};

use crate::instructions::append_flash_fulfill_route_chunk::write_buffer_chunk;
use crate::instructions::init_flash_fulfill_intent::{do_init, serialize_buffer};
use crate::instructions::{FlashFulfillerError, InitFlashFulfillIntentArgs};

/// Args for [`set_flash_fulfill_intent`]: the `(route, reward)` pair to buffer under the intent-hash PDA.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SetFlashFulfillIntentArgs {
    /// Route that will be committed to the buffer.
    pub route: Route,
    /// Reward that will be committed to the buffer.
    pub reward: Reward,
}

/// Accounts for [`set_flash_fulfill_intent`].
#[derive(Accounts)]
pub struct SetFlashFulfillIntent<'info> {
    /// Pays for the buffer account rent and is recorded as the writer.
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address + init handled in handler
    #[account(mut)]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Single-tx convenience that fuses init + a full-payload append for Routes
/// small enough to fit in one tx. Large routes must use explicit
/// `init_flash_fulfill_intent` + `append_flash_fulfill_route_chunk`.
///
/// Composes the shared `do_init` and `write_buffer_chunk` helpers so init
/// and append share identical validation with the multi-tx flow.
pub fn set_flash_fulfill_intent(
    ctx: Context<SetFlashFulfillIntent>,
    args: SetFlashFulfillIntentArgs,
) -> Result<()> {
    let SetFlashFulfillIntentArgs { route, reward } = args;
    let route_bytes = route.try_to_vec().expect("Failed to serialize Route");
    let route_total_size: u32 = route_bytes
        .len()
        .try_into()
        .map_err(|_| FlashFulfillerError::InvalidRouteTotalSize)?;
    let route_hash = route.hash();
    let committed_intent_hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());

    let mut account = do_init(
        &ctx.accounts.writer.to_account_info(),
        &ctx.accounts.flash_fulfill_intent,
        &ctx.accounts.system_program,
        InitFlashFulfillIntentArgs {
            intent_hash: committed_intent_hash,
            route_hash,
            reward,
            route_total_size,
        },
    )?;

    write_buffer_chunk(&mut account, 0, &route_bytes)?;

    serialize_buffer(&ctx.accounts.flash_fulfill_intent, &account)
}
