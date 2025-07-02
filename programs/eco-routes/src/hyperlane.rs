use anchor_lang::{
    prelude::{
        borsh::{BorshDeserialize, BorshSerialize},
        *,
    },
    solana_program::{instruction::Instruction, program::invoke_signed},
    system_program,
};

use crate::{
    encoding,
    state::{Reward, Route},
};

pub const DOMAIN_ID: u32 = 1399811149;

pub const MAILBOX_ID: Pubkey = pubkey!("E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi");
pub const MULTISIG_ISM_ID: Pubkey = pubkey!("TrustedRe1ayer1sm11111111111111111111111111");

pub const HANDLE_DISCRIMINATOR: [u8; 8] = [33, 210, 5, 66, 196, 212, 239, 142];
pub const HANDLE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] = [194, 141, 30, 82, 241, 41, 169, 52];
pub const INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR: [u8; 8] = [45, 18, 245, 87, 234, 46, 246, 15];
pub const INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] =
    [190, 214, 218, 129, 67, 97, 4, 76];

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
enum MailboxInstruction {
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

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
struct Init {}

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
struct OutboxDispatch {
    pub sender: Pubkey,
    pub destination_domain: u32,
    pub recipient: [u8; 32],
    pub message_body: Vec<u8>,
}

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
struct InboxProcess {}

pub fn dispatch_fulfillment_message<'info>(
    route: &Route,
    reward: &Reward,
    intent_hash: &[u8; 32],
    claimant: [u8; 32],
    mailbox_program: &UncheckedAccount<'info>,
    outbox_pda: &UncheckedAccount<'info>,
    dispatch_authority: &UncheckedAccount<'info>,
    spl_noop_program: &UncheckedAccount<'info>,
    payer: &Signer<'info>,
    unique_message: &Signer<'info>,
    system_program: &Program<'info, System>,
    dispatched_message_pda: &UncheckedAccount<'info>,
    dispatch_authority_bump: u8,
) -> Result<()> {
    let outbox_dispatch = MailboxInstruction::OutboxDispatch(OutboxDispatch {
        sender: dispatch_authority.key(),
        // Domain id is flipped so the message sends back to the Intent's source chain, but hashes match
        destination_domain: route.source_domain_id,
        recipient: reward.prover,
        message_body: encoding::FulfillMessages::new(vec![*intent_hash], vec![claimant])
            .expect("fulfill messages must be valid with one intent hash and one solver")
            .encode(),
    });

    let ix = Instruction {
        program_id: mailbox_program.key(),
        accounts: vec![
            AccountMeta::new(outbox_pda.key(), false),
            AccountMeta::new_readonly(dispatch_authority.key(), true),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(spl_noop_program.key(), false),
            AccountMeta::new(payer.key(), true),
            AccountMeta::new_readonly(unique_message.key(), true),
            AccountMeta::new(dispatched_message_pda.key(), false),
        ],
        data: outbox_dispatch.try_to_vec()?,
    };

    invoke_signed(
        &ix,
        &[
            outbox_pda.to_account_info(),
            dispatch_authority.to_account_info(),
            system_program.to_account_info(),
            spl_noop_program.to_account_info(),
            payer.to_account_info(),
            unique_message.to_account_info(),
            dispatched_message_pda.to_account_info(),
        ],
        &[&[b"dispatch_authority", &[dispatch_authority_bump]]],
    )?;

    Ok(())
}

/// NOTE: This is used for hyperlane interop only and should not be used anywhere else
/// A ridiculous workaround for `<https://github.com/solana-labs/solana/issues/31391>`,
/// which is a bug where if a simulated transaction's return data ends with zero byte(s),
/// they end up being incorrectly truncated.
/// As a workaround, we can (de)serialize data with a trailing non-zero byte.
#[derive(Debug, AnchorSerialize, AnchorDeserialize)]
pub struct SimulationReturnData<T>
where
    T: AnchorSerialize + AnchorDeserialize,
{
    pub return_data: T,
    trailing_byte: u8,
}

impl<T> SimulationReturnData<T>
where
    T: AnchorSerialize + AnchorDeserialize,
{
    pub fn new(return_data: T) -> Self {
        Self {
            return_data,
            trailing_byte: u8::MAX,
        }
    }
}
