use anchor_lang::prelude::*;
use eco_svm_std::event_authority_pda;

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    self, message_lib_info_pda, uln_settings_pda, ExecutorConfig, InitConfigParams,
    SetConfigParams, UlnConfig, CONFIG_TYPE_EXECUTOR, CONFIG_TYPE_RECEIVE_ULN,
    CONFIG_TYPE_SEND_ULN, ENDPOINT_ID, INIT_CONFIG_DISCRIMINATOR, NIL_DVN_COUNT,
    SET_CONFIG_DISCRIMINATOR, ULN_ID,
};
use crate::state::{pda_payer_pda, Store, PDA_PAYER_SEED};

/// The path's full security config. Nothing may resolve to a LayerZero default
/// (count/confirmations 0, executor default), or LayerZero governance would
/// control our DVN set after we finalize.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct PathConfig {
    pub send_uln: UlnConfig,
    pub receive_uln: UlnConfig,
    pub executor: ExecutorConfig,
}

impl PathConfig {
    fn validate(&self) -> Result<()> {
        [&self.send_uln, &self.receive_uln]
            .iter()
            .try_for_each(|uln| {
                require!(
                    uln.confirmations != 0
                        && uln.required_dvn_count != 0
                        && uln.required_dvn_count != NIL_DVN_COUNT
                        && uln.required_dvns.len() == uln.required_dvn_count as usize,
                    LayerZeroProverError::UnpinnedConfig
                );
                Ok(())
            })?;
        require!(
            self.executor.max_message_size != 0 && self.executor.executor != Pubkey::default(),
            LayerZeroProverError::UnpinnedConfig
        );

        Ok(())
    }
}

#[derive(Accounts)]
pub struct SetPathConfig<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: system-owned lamport reserve; the OApp's delegate and ULN config payer
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: seeds validated by the endpoint
    pub oapp_registry: UncheckedAccount<'info>,
    /// CHECK: pinned to the endpoint's record for ULN302 (must stay read-only)
    #[account(address = message_lib_info_pda(&uln_settings_pda().0).0 @ LayerZeroProverError::InvalidUln)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: pinned
    #[account(address = uln_settings_pda().0 @ LayerZeroProverError::InvalidUln)]
    pub uln_settings: UncheckedAccount<'info>,
    /// CHECK: pinned
    #[account(address = ULN_ID @ LayerZeroProverError::InvalidUln)]
    pub uln_program: UncheckedAccount<'info>,
    /// CHECK: created/validated by ULN302
    #[account(mut)]
    pub uln_send_config: UncheckedAccount<'info>,
    /// CHECK: created/validated by ULN302
    #[account(mut)]
    pub uln_receive_config: UncheckedAccount<'info>,
    /// CHECK: validated by ULN302
    pub uln_default_send_config: UncheckedAccount<'info>,
    /// CHECK: validated by ULN302
    pub uln_default_receive_config: UncheckedAccount<'info>,
    /// CHECK: pinned
    #[account(address = event_authority_pda(&ULN_ID).0 @ LayerZeroProverError::InvalidUln)]
    pub uln_event_authority: UncheckedAccount<'info>,
}

pub fn set_path_config(ctx: Context<SetPathConfig>, eid: u32, config: PathConfig) -> Result<()> {
    require!(
        ctx.accounts.store.peer(eid).is_some(),
        LayerZeroProverError::UnknownPeer
    );
    config.validate()?;
    let oapp = ctx.accounts.store.key();
    let delegate = ctx.accounts.pda_payer.key();
    let (_, bump) = pda_payer_pda();
    let seeds: &[&[u8]] = &[PDA_PAYER_SEED, &[bump]];
    let a = &ctx.accounts;
    let endpoint_head = [
        a.endpoint_program.to_account_info(),
        a.pda_payer.to_account_info(),
        a.oapp_registry.to_account_info(),
        a.message_lib_info.to_account_info(),
        a.uln_settings.to_account_info(),
        a.uln_program.to_account_info(),
    ];

    // ULN302 `init_config` tail: [payer, uln, send_config, receive_config, system_program]
    let init_tail = [
        a.pda_payer.to_account_info(),
        a.uln_settings.to_account_info(),
        a.uln_send_config.to_account_info(),
        a.uln_receive_config.to_account_info(),
        a.system_program.to_account_info(),
    ];
    layerzero::invoke(
        ENDPOINT_ID,
        INIT_CONFIG_DISCRIMINATOR,
        &InitConfigParams { oapp, eid },
        &[endpoint_head.as_slice(), &init_tail].concat(),
        &[delegate],
        &[seeds],
    )?;

    // ULN302 `set_config` tail: [uln, send_config, receive_config, default_send,
    // default_receive, uln event_authority, uln program]
    let set_tail = [
        a.uln_settings.to_account_info(),
        a.uln_send_config.to_account_info(),
        a.uln_receive_config.to_account_info(),
        a.uln_default_send_config.to_account_info(),
        a.uln_default_receive_config.to_account_info(),
        a.uln_event_authority.to_account_info(),
        a.uln_program.to_account_info(),
    ];
    let accounts = [endpoint_head.as_slice(), &set_tail].concat();
    [
        (CONFIG_TYPE_SEND_ULN, borsh::to_vec(&config.send_uln)?),
        (CONFIG_TYPE_RECEIVE_ULN, borsh::to_vec(&config.receive_uln)?),
        (CONFIG_TYPE_EXECUTOR, borsh::to_vec(&config.executor)?),
    ]
    .into_iter()
    .try_for_each(|(config_type, config)| {
        layerzero::invoke(
            ENDPOINT_ID,
            SET_CONFIG_DISCRIMINATOR,
            &SetConfigParams {
                oapp,
                eid,
                config_type,
                config,
            },
            &accounts,
            &[delegate],
            &[seeds],
        )
    })
}
