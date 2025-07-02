use crate::{
    encoding,
    hyperlane::SimulationReturnData,
    state::{EcoRoutes, Intent},
};
use anchor_lang::prelude::*;

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
) -> Result<SimulationReturnData<Vec<SerializableAccountMeta>>> {
    let fulfill_messages = encoding::FulfillMessages::decode(&payload)?;

    let metas: Vec<SerializableAccountMeta> =
        std::iter::once(AccountMeta::new_readonly(EcoRoutes::pda().0, false).into())
            .chain(
                fulfill_messages
                    .intent_hashes()
                    .iter()
                    .map(|hash| AccountMeta::new(Intent::pda(*hash).0, false).into()),
            )
            .collect();

    Ok(SimulationReturnData::new(metas))
}
