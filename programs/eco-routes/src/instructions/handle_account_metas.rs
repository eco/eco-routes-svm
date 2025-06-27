use crate::{
    encoding,
    error::EcoRoutesError,
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

impl Into<AccountMeta> for SerializableAccountMeta {
    fn into(self) -> AccountMeta {
        AccountMeta {
            pubkey: self.pubkey,
            is_signer: self.is_signer,
            is_writable: self.is_writable,
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
    let (intent_hashes, _claimants) = encoding::decode_fulfillment_message(&payload)
        .map_err(|_| error!(EcoRoutesError::InvalidHandlePayload))?;

    let mut metas = vec![SerializableAccountMeta::from(AccountMeta::new_readonly(
        EcoRoutes::pda().0,
        false,
    ))];

    for intent_hash in intent_hashes {
        metas.push(SerializableAccountMeta::from(AccountMeta::new(
            Intent::pda(intent_hash).0,
            false,
        )));
    }

    Ok(SimulationReturnData::new(metas))
}
