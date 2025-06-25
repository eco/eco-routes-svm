use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::system_program;
use eco_svm_std::Bytes32;

use crate::instructions::Prove;

#[cfg(feature = "mainnet")]
pub const MAILBOX_ID: Pubkey = pubkey!("E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi");
#[cfg(not(feature = "mainnet"))]
pub const MAILBOX_ID: Pubkey = pubkey!("75HBBLae3ddeneJVrZeyrDfv6vb7SMC3aCpBucSXS5aR");

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
