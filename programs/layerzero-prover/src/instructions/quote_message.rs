use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::get_return_data;
use eco_svm_std::prover::ProofData;
use eco_svm_std::Bytes32;

use crate::constants::lz_receive_gas;
use crate::instructions::{check_intent_count, LayerZeroProverError};
use crate::layerzero::{
    self, lz_receive_options, MessagingFee, QuoteParams, ENDPOINT_ID, QUOTE_DISCRIMINATOR,
};
use crate::state::Store;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct QuoteMessageArgs {
    pub dst_eid: u32,
    pub receiver: Bytes32,
    pub payload: Vec<u8>,
}

/// Read-only fee quote for a batch with the exact options `send_message`
/// will use; run it with `simulateTransaction` and read the return data.
/// Remaining accounts are the endpoint `quote` accounts (send library program,
/// send library config, default, send library info, endpoint settings, nonce)
/// and the ULN302 quote/worker accounts — all read-only.
#[derive(Accounts)]
pub struct QuoteMessage<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
}

pub fn quote_message<'info>(
    ctx: Context<'info, QuoteMessage<'info>>,
    args: QuoteMessageArgs,
) -> Result<MessagingFee> {
    let peer = *ctx
        .accounts
        .store
        .peer(args.dst_eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    require!(
        args.receiver == peer.address,
        LayerZeroProverError::InvalidReceiver
    );
    let intents = ProofData::from_bytes(&args.payload)?
        .intent_hashes_claimants
        .len();
    check_intent_count(intents)?;

    let params = QuoteParams {
        sender: ctx.accounts.store.key(),
        dst_eid: args.dst_eid,
        receiver: args.receiver.into(),
        message: args.payload,
        options: lz_receive_options(lz_receive_gas(intents)),
        pay_in_lz_token: false,
    };
    let accounts: Vec<AccountInfo<'info>> =
        std::iter::once(ctx.accounts.endpoint_program.to_account_info())
            .chain(ctx.remaining_accounts.iter().cloned())
            .collect();
    layerzero::invoke(
        ENDPOINT_ID,
        QUOTE_DISCRIMINATOR,
        &params,
        &accounts,
        &[],
        &[],
    )?;

    let (program, data) = get_return_data().ok_or(LayerZeroProverError::InvalidQuote)?;
    require_keys_eq!(program, ENDPOINT_ID, LayerZeroProverError::InvalidQuote);
    MessagingFee::try_from_slice(&data).map_err(|_| LayerZeroProverError::InvalidQuote.into())
}
