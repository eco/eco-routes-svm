use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;
use borsh::BorshSerialize;
use eco_svm_std::SerializableAccountMeta;

use crate::hyperlane;

#[derive(Accounts)]
pub struct IsmAccountMetas<'info> {
    /// CHECK: Simulation only
    #[account(
        seeds = [
            b"hyperlane_message_recipient",
            b"-",
            b"interchain_security_module",
            b"-",
            b"account_metas"
        ],
        bump
    )]
    pub ism_account_metas: AccountInfo<'info>,
}

pub fn ism_account_metas(_ctx: Context<IsmAccountMetas>) -> Result<()> {
    let metas = vec![SerializableAccountMeta {
        pubkey: hyperlane::MULTISIG_ISM_MESSAGE_ID,
        is_signer: false,
        is_writable: false,
    }];

    set_return_data(&metas.try_to_vec()?);

    Ok(())
}
