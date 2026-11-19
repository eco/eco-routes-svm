use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    self, RegisterOAppParams, ENDPOINT_ID, LZ_RECEIVE_TYPES_SEED, REGISTER_OAPP_DISCRIMINATOR,
};
use crate::state::{pda_payer_pda, LzReceiveTypesAccount, Peer, Store, STORE_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitArgs {
    pub peers: Vec<Peer>,
}

#[derive(Accounts)]
pub struct Init<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    /// CHECK: canonical PDA, created here
    #[account(mut, address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: UncheckedAccount<'info>,
    /// CHECK: canonical PDA, created here
    #[account(mut, address = LzReceiveTypesAccount::pda().0 @ LayerZeroProverError::InvalidLzReceiveTypes)]
    pub lz_receive_types: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint, which validates its seeds
    #[account(mut)]
    pub oapp_registry: UncheckedAccount<'info>,
    /// CHECK: validated by the endpoint's event_cpi
    pub endpoint_event_authority: UncheckedAccount<'info>,
}

pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
    let (store, store_bump) = Store::pda();
    let store_seeds: &[&[u8]] = &[STORE_SEED, &[store_bump]];
    Store::new(args.peers)?.init(
        &ctx.accounts.store,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[store_seeds],
    )?;

    let (_, lz_receive_types_bump) = LzReceiveTypesAccount::pda();
    LzReceiveTypesAccount { store }.init(
        &ctx.accounts.lz_receive_types,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[
            LZ_RECEIVE_TYPES_SEED,
            store.as_ref(),
            &[lz_receive_types_bump],
        ]],
    )?;

    let accounts = ctx.accounts;
    layerzero::invoke(
        ENDPOINT_ID,
        REGISTER_OAPP_DISCRIMINATOR,
        &RegisterOAppParams {
            delegate: pda_payer_pda().0,
        },
        &[
            accounts.endpoint_program.to_account_info(),
            accounts.payer.to_account_info(),
            accounts.store.to_account_info(),
            accounts.oapp_registry.to_account_info(),
            accounts.system_program.to_account_info(),
            accounts.endpoint_event_authority.to_account_info(),
            accounts.endpoint_program.to_account_info(),
        ],
        &[store],
        &[store_seeds],
    )
}
