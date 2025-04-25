use anchor_lang::{
    prelude::*,
    solana_program::{instruction::Instruction, program::invoke},
};

use crate::error::EcoRoutesError;

use super::{HandleBlueprintArgs, HandleFulfilledAckArgs};

#[derive(Accounts)]
pub struct Handle<'info> {
    #[account(
        mut,
        address = expected_process_authority_key() @ EcoRoutesError::InvalidProcessAuthority
    )]
    pub process_authority: Signer<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub enum InboxPayload {
    Blueprint(HandleBlueprintArgs),
    FulfilledAck(HandleFulfilledAckArgs),
}

pub fn expected_process_authority_key() -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"hyperlane",
            b"-",
            b"process_authority",
            b"-",
            crate::ID.as_ref(),
        ],
        &crate::hyperlane::MAILBOX_ID,
    )
    .0
}

const HANDLE_BLUEPRINT_DISCRIMINATOR: &[u8] = &[252, 120, 9, 47, 115, 75, 51, 32];
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct HandleBlueprintIx {
    pub args: HandleBlueprintArgs,
    pub origin: u32,
    pub sender: [u8; 32],
}

const HANDLE_FULFILLED_ACK_DISCRIMINATOR: &[u8] = &[37, 145, 126, 63, 222, 217, 74, 224];
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct HandleFulfilledAckIx {
    pub args: HandleFulfilledAckArgs,
    pub origin: u32,
    pub sender: [u8; 32],
}

pub fn handle<'info>(
    ctx: Context<'_, '_, '_, 'info, Handle<'info>>,
    origin: u32,
    sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    let inbox_payload = InboxPayload::try_from_slice(&payload)
        .map_err(|_| error!(EcoRoutesError::InvalidHandlePayload))?;

    match inbox_payload {
        InboxPayload::Blueprint(blueprint_payload) => {
            let instruction = HandleBlueprintIx {
                args: blueprint_payload,
                origin,
                sender,
            };

            let mut account_infos = ctx.remaining_accounts.to_vec();
            account_infos.insert(0, ctx.accounts.process_authority.to_account_info());

            let mut account_metas = account_infos.to_account_metas(None);
            account_metas.insert(
                0,
                AccountMeta::new(ctx.accounts.process_authority.key(), false),
            );

            let mut data = vec![];
            data.extend(HANDLE_BLUEPRINT_DISCRIMINATOR);
            data.extend(instruction.try_to_vec()?);

            invoke(
                &Instruction {
                    program_id: *ctx.program_id,
                    accounts: account_metas,
                    data,
                },
                &account_infos,
            )?;
        }

        InboxPayload::FulfilledAck(fulfilled_ack_payload) => {
            let instruction = HandleFulfilledAckIx {
                args: fulfilled_ack_payload,
                origin,
                sender,
            };

            let mut account_infos = ctx.remaining_accounts.to_vec();
            account_infos.insert(0, ctx.accounts.process_authority.to_account_info());

            let mut account_metas = account_infos.to_account_metas(None);
            account_metas.insert(
                0,
                AccountMeta::new(ctx.accounts.process_authority.key(), false),
            );

            let mut data = vec![];
            data.extend(HANDLE_FULFILLED_ACK_DISCRIMINATOR);
            data.extend(instruction.try_to_vec()?);

            invoke(
                &Instruction {
                    program_id: *ctx.program_id,
                    accounts: account_metas,
                    data,
                },
                &account_infos,
            )?;
        }
    }

    Ok(())
}
