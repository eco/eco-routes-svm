use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;
use borsh::BorshSerialize;

use super::expected_process_authority;
use crate::encoding;
use crate::state::Intent;

#[derive(Debug, AnchorDeserialize, AnchorSerialize, Clone)]
pub struct SerializableAccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl From<AccountMeta> for SerializableAccountMeta {
    fn from(account_meta: AccountMeta) -> Self {
        Self {
            pubkey: account_meta.pubkey,
            is_signer: account_meta.is_signer,
            is_writable: account_meta.is_writable,
        }
    }
}

impl From<SerializableAccountMeta> for AccountMeta {
    fn from(serializable_account_meta: SerializableAccountMeta) -> Self {
        AccountMeta {
            pubkey: serializable_account_meta.pubkey,
            is_signer: serializable_account_meta.is_signer,
            is_writable: serializable_account_meta.is_writable,
        }
    }
}

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
    let fulfill_messages = encoding::FulfillMessages::decode(&payload)?;
    let account_metas: Vec<SerializableAccountMeta> = iter::once(AccountMeta::new_readonly(
        expected_process_authority(),
        true,
    ))
    .chain(
        fulfill_messages
            .intent_hashes()
            .into_iter()
            .map(|intent_hash| AccountMeta::new(Intent::pda(intent_hash).0, false)),
    )
    .map(Into::into)
    .collect();

    set_return_data(&account_metas.try_to_vec()?);

    Ok(())
}
