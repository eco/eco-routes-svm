use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;
use anchor_lang::system_program;
use borsh::BorshSerialize;
use eco_svm_std::prover::Proof;
use eco_svm_std::{event_authority_pda, SerializableAccountMeta};

use crate::state::{pda_payer_pda, Config};
use crate::utils::claimant_and_intent_hash;

#[derive(Accounts)]
pub struct HandleAccountMetas<'info> {
    /// CHECK: simulation only
    #[account(
        seeds = [b"hyperlane_message_recipient", b"-", b"handle", b"-", b"account_metas"],
        bump
    )]
    pub handle_account_metas: AccountInfo<'info>,
}

pub fn handle_account_metas(
    _ctx: Context<HandleAccountMetas>,
    _origin: u32,
    _sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    let (_, intent_hash) = claimant_and_intent_hash(payload)?;

    let account_metas: Vec<SerializableAccountMeta> = vec![
        AccountMeta::new_readonly(Config::pda().0, false),
        AccountMeta::new(Proof::pda(&intent_hash, &crate::ID).0, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new(pda_payer_pda().0, false),
        AccountMeta::new_readonly(event_authority_pda(&crate::ID).0, false),
        AccountMeta::new_readonly(crate::ID, false),
    ]
    .into_iter()
    .map(Into::into)
    .collect();

    set_return_data(&account_metas.try_to_vec()?);

    Ok(())
}
