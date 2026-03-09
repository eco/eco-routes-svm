use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;
use eco_svm_std::SerializableAccountMeta;

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

/// Returns an empty list of account metas for the ISM. Because this recipient
/// defers to Hyperlane's default ISM (no custom ISM is configured), no
/// additional accounts are required for the ISM verification step.
pub fn ism_account_metas(_ctx: Context<IsmAccountMetas>) -> Result<()> {
    set_return_data(&Vec::<SerializableAccountMeta>::new().try_to_vec()?);

    Ok(())
}
