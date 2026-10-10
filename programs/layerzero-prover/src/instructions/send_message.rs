use anchor_lang::prelude::*;

use crate::constants::lz_receive_gas;
use crate::instructions::LayerZeroProverError;
use crate::layerzero::{self, lz_receive_options, SendParams, ENDPOINT_ID, SEND_DISCRIMINATOR};
use crate::state::{PendingSend, Store, STORE_SEED};

/// Permissionless: only portal's dispatcher can create a `PendingSend`, so this
/// can only ever send a portal-attested batch to its configured peer, with
/// options computed here. Remaining accounts are the endpoint `send` accounts
/// after `[program, sender]`: send library program, send library config,
/// default send library config, send library info (read-only), endpoint
/// settings, nonce (mut), endpoint event authority, endpoint program, then the
/// ULN302 send accounts (payer = the fee payer, signer) and worker accounts.
#[derive(Accounts)]
pub struct SendMessage<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    #[account(mut, close = rent_payer, has_one = rent_payer @ LayerZeroProverError::InvalidRentPayer)]
    pub pending_send: Account<'info, PendingSend>,
    /// CHECK: must equal `pending_send.rent_payer`
    #[account(mut)]
    pub rent_payer: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
}

pub fn send_message<'info>(
    ctx: Context<'info, SendMessage<'info>>,
    max_native_fee: u64,
) -> Result<()> {
    let pending = &ctx.accounts.pending_send;
    let params = SendParams {
        dst_eid: pending.dst_eid,
        receiver: pending.receiver.into(),
        message: pending.payload.clone(),
        options: lz_receive_options(lz_receive_gas(pending.intent_count())),
        native_fee: max_native_fee,
        lz_token_fee: 0,
    };
    let (store, bump) = Store::pda();
    let accounts: Vec<AccountInfo<'info>> = [
        ctx.accounts.endpoint_program.to_account_info(),
        ctx.accounts.store.to_account_info(),
    ]
    .into_iter()
    .chain(ctx.remaining_accounts.iter().cloned())
    .collect();

    layerzero::invoke(
        ENDPOINT_ID,
        SEND_DISCRIMINATOR,
        &params,
        &accounts,
        &[store],
        &[&[STORE_SEED, &[bump]]],
    )
}
