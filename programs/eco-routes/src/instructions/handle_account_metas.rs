use anchor_lang::{prelude::*, solana_program::program::set_return_data, system_program};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{error::EcoRoutesError, state::{DomainRegistry, IntentMarker}};

use super::InboxPayload;

#[derive(Debug, BorshSerialize, BorshDeserialize, Clone)]
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

#[derive(Accounts)]
pub struct HandleAccountMetas<'info> {
    /// CHECK: simulation only
    #[account(
        mut, 
        seeds = [b"hyperlane_message_recipient", b"-", b"handle", b"-", b"account_metas"], 
        bump
    )]
    pub handle_account_metas: AccountInfo<'info>,
}

pub fn handle_account_metas(
    _ctx: Context<HandleAccountMetas>,
    origin: u32,
    _sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    let inbox_payload = InboxPayload::try_from_slice(&payload)
        .map_err(|_| error!(EcoRoutesError::InvalidHandlePayload))?;

    match inbox_payload {
        InboxPayload::Blueprint(blueprint_payload) => {
            let metas = vec![
                SerializableAccountMeta::from(AccountMeta::new(IntentMarker::pda(blueprint_payload.intent_hash), false)),
                SerializableAccountMeta::from(AccountMeta::new_readonly(DomainRegistry::pda(origin), false)),
                SerializableAccountMeta::from(AccountMeta::new(system_program::ID, false))
            ];
            set_return_data(&metas.try_to_vec()?);
        }
        InboxPayload::FulfilledAck(fulfilled_ack_payload) => {
            let metas = vec![
                SerializableAccountMeta::from(AccountMeta::new(IntentMarker::pda(fulfilled_ack_payload.intent_hash), false)),
            ];
            set_return_data(&metas.try_to_vec()?);
        }
    }

    Ok(())
}
