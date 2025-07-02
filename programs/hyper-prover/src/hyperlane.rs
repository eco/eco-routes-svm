use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::system_program;
use eco_svm_std::Bytes32;

use crate::instructions::Prove;

#[cfg(feature = "mainnet")]
pub const MAILBOX_ID: Pubkey = pubkey!("E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi");
#[cfg(feature = "mainnet")]
pub const MULTISIG_ISM_MESSAGE_ID: Pubkey = pubkey!("EpAuVN1oc5GccKAk41VMBHTgzJFtB5bftvi92SywQdbS");
#[cfg(not(feature = "mainnet"))]
pub const MAILBOX_ID: Pubkey = pubkey!("75HBBLae3ddeneJVrZeyrDfv6vb7SMC3aCpBucSXS5aR");
#[cfg(not(feature = "mainnet"))]
pub const MULTISIG_ISM_MESSAGE_ID: Pubkey = pubkey!("4GHxwWyKB9exhKG4fdyU2hfLgfFzhHp2WcsSKc2uNR1k");

pub const HANDLE_DISCRIMINATOR: [u8; 8] = [33, 210, 5, 66, 196, 212, 239, 142];
pub const HANDLE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] = [194, 141, 30, 82, 241, 41, 169, 52];
pub const INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR: [u8; 8] = [45, 18, 245, 87, 234, 46, 246, 15];
pub const INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] =
    [190, 214, 218, 129, 67, 97, 4, 76];

pub fn process_authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"hyperlane",
            b"-",
            b"process_authority",
            b"-",
            crate::ID.as_ref(),
        ],
        &MAILBOX_ID,
    )
}

// Hyperlane's instructions copied from their code.
// Even though we are only using OutboxDispatch, it
// is critical to keep the rest because borsh serialization
// is dependent on the enum variant order.
#[derive(BorshSerialize, BorshDeserialize)]
#[allow(dead_code)]
pub enum MailboxInstruction {
    Init(Init),
    InboxProcess(InboxProcess),
    InboxSetDefaultIsm(Pubkey),
    InboxGetRecipientIsm(Pubkey),
    OutboxDispatch(OutboxDispatch),
    OutboxGetCount,
    OutboxGetLatestCheckpoint,
    OutboxGetRoot,
    GetOwner,
    TransferOwnership(Option<Pubkey>),
    ClaimProtocolFees,
    SetProtocolFeeConfig,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Init {}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct OutboxDispatch {
    pub sender: Pubkey,
    pub destination_domain: u32,
    pub recipient: [u8; 32],
    pub message_body: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct InboxProcess {}

pub fn dispatch_msg(
    ctx: &Context<Prove>,
    destination_domain: u32,
    recipient: Bytes32,
    message_body: Vec<u8>,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    let outbox_dispatch = MailboxInstruction::OutboxDispatch(OutboxDispatch {
        sender: ctx.accounts.dispatcher.key(),
        destination_domain,
        recipient: recipient.into(),
        message_body,
    });
    let ix = Instruction {
        program_id: MAILBOX_ID,
        accounts: vec![
            AccountMeta::new(ctx.accounts.outbox_pda.key(), false),
            AccountMeta::new_readonly(ctx.accounts.dispatcher.key(), true),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(ctx.accounts.spl_noop_program.key(), false),
            AccountMeta::new(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.unique_message.key(), true),
            AccountMeta::new(ctx.accounts.dispatched_message_pda.key(), false),
        ],
        data: outbox_dispatch.try_to_vec()?,
    };

    invoke_signed(
        &ix,
        &[
            ctx.accounts.outbox_pda.to_account_info(),
            ctx.accounts.dispatcher.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.spl_noop_program.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.unique_message.to_account_info(),
            ctx.accounts.dispatched_message_pda.to_account_info(),
        ],
        &[signer_seeds],
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_authority_pda_deterministic() {
        goldie::assert_json!(process_authority_pda());
    }
}
