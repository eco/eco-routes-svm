use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    self, message_lib_info_pda, uln_settings_pda, InitNonceParams, InitReceiveLibraryParams,
    InitSendLibraryParams, SetReceiveLibraryParams, SetSendLibraryParams, ENDPOINT_ID,
    INIT_NONCE_DISCRIMINATOR, INIT_RECEIVE_LIBRARY_DISCRIMINATOR, INIT_SEND_LIBRARY_DISCRIMINATOR,
    SET_RECEIVE_LIBRARY_DISCRIMINATOR, SET_SEND_LIBRARY_DISCRIMINATOR,
};
use crate::state::{pda_payer_pda, Store, PDA_PAYER_SEED};

/// Opens one peer's path in both directions and pins ULN302 as its send and
/// receive library. The endpoint validates every PDA it is handed against
/// `store` and `eid`; we pin only what it cannot: the library.
#[derive(Accounts)]
pub struct InitPath<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: system-owned lamport reserve; the OApp's delegate
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: seeds validated by the endpoint
    pub oapp_registry: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub nonce: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub pending_nonce: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub send_library_config: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub receive_library_config: UncheckedAccount<'info>,
    /// CHECK: pinned to the endpoint's record for ULN302
    #[account(address = message_lib_info_pda(&uln_settings_pda().0).0 @ LayerZeroProverError::InvalidUln)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: validated by the endpoint's event_cpi
    pub endpoint_event_authority: UncheckedAccount<'info>,
}

pub fn init_path(ctx: Context<InitPath>, eid: u32) -> Result<()> {
    let peer = *ctx
        .accounts
        .store
        .peer(eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    let store = ctx.accounts.store.key();
    let delegate = ctx.accounts.pda_payer.key();
    let (_, bump) = pda_payer_pda();
    let seeds: &[&[u8]] = &[PDA_PAYER_SEED, &[bump]];
    let new_lib = uln_settings_pda().0;
    let account = &ctx.accounts;
    let endpoint = account.endpoint_program.to_account_info();
    let pda_payer = account.pda_payer.to_account_info();
    let registry = account.oapp_registry.to_account_info();
    let system = account.system_program.to_account_info();
    let event_authority = account.endpoint_event_authority.to_account_info();
    let message_lib_info = account.message_lib_info.to_account_info();

    layerzero::invoke(
        ENDPOINT_ID,
        INIT_NONCE_DISCRIMINATOR,
        &InitNonceParams {
            local_oapp: store,
            remote_eid: eid,
            remote_oapp: peer.address.into(),
        },
        &[
            endpoint.clone(),
            pda_payer.clone(),
            registry.clone(),
            account.nonce.to_account_info(),
            account.pending_nonce.to_account_info(),
            system.clone(),
        ],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        INIT_SEND_LIBRARY_DISCRIMINATOR,
        &InitSendLibraryParams { sender: store, eid },
        &[
            endpoint.clone(),
            pda_payer.clone(),
            registry.clone(),
            account.send_library_config.to_account_info(),
            system.clone(),
        ],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        INIT_RECEIVE_LIBRARY_DISCRIMINATOR,
        &InitReceiveLibraryParams {
            receiver: store,
            eid,
        },
        &[
            endpoint.clone(),
            pda_payer.clone(),
            registry.clone(),
            account.receive_library_config.to_account_info(),
            system,
        ],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        SET_SEND_LIBRARY_DISCRIMINATOR,
        &SetSendLibraryParams {
            sender: store,
            eid,
            new_lib,
        },
        &[
            endpoint.clone(),
            pda_payer.clone(),
            registry.clone(),
            account.send_library_config.to_account_info(),
            message_lib_info.clone(),
            event_authority.clone(),
            endpoint.clone(),
        ],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        SET_RECEIVE_LIBRARY_DISCRIMINATOR,
        &SetReceiveLibraryParams {
            receiver: store,
            eid,
            new_lib,
            grace_period: 0,
        },
        &[
            endpoint.clone(),
            pda_payer,
            registry,
            account.receive_library_config.to_account_info(),
            message_lib_info,
            event_authority,
            endpoint,
        ],
        &[delegate],
        &[seeds],
    )
}
