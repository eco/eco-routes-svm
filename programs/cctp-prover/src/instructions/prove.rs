use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::prover::ProveArgs;

use crate::cctp::{self, SendMessageParams, MIN_FINALITY_THRESHOLD, SOLANA_DOMAIN};
use crate::constants::MAX_INTENTS_PER_PROVE;
use crate::instructions::CctpProverError;
use crate::state::{sender_authority_pda, Config, SENDER_AUTHORITY_SEED};

/// Called by `portal::prove` (stack: portal → cctp_prover → transmitter →
/// system = 4 of 5 frames), so the send is one step, with no LayerZero-style
/// pending commit.
#[derive(Accounts)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda(&crate::ID).0 @ CctpProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
    /// Binds `destination_domain` and the recipient to a configured peer.
    #[account(address = Config::pda().0 @ CctpProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// CHECK: checked against `sender_authority_pda` in the handler, which
    /// needs its bump to sign anyway; signs `send_message` via `invoke_signed`
    pub sender_authority: UncheckedAccount<'info>,
    /// Pays the event account's rent; reclaimable after 5 days via Circle's
    /// `reclaim_event_account`.
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: owner and contents are checked by the transmitter
    #[account(mut, address = cctp::message_transmitter_pda().0 @ CctpProverError::InvalidTransmitter)]
    pub message_transmitter: UncheckedAccount<'info>,
    /// A fresh keypair the transmitter initializes as its `MessageSent` account.
    #[account(mut)]
    pub message_sent_event_data: Signer<'info>,
    /// The transmitter's `sender_program`: the message's `sender` becomes this ID.
    pub program: Program<'info, crate::program::CctpProver>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(executable, address = cctp::MESSAGE_TRANSMITTER_V2_ID @ CctpProverError::InvalidTransmitter)]
    pub message_transmitter_program: UncheckedAccount<'info>,
}

pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        domain_id,
        proof_data,
        data,
    } = args;

    require!(
        proof_data.intent_hashes_claimants.len() <= MAX_INTENTS_PER_PROVE,
        CctpProverError::TooManyIntents
    );
    let destination_domain: u32 = domain_id
        .try_into()
        .map_err(|_| CctpProverError::InvalidDomainId)?;
    require!(
        destination_domain != SOLANA_DOMAIN,
        CctpProverError::InvalidDomainId
    );
    let recipient = Pubkey::new_from_array(
        <[u8; 32]>::try_from(data.as_slice()).map_err(|_| CctpProverError::InvalidData)?,
    );
    // A message to an unconfigured domain or another recipient would be
    // attested but never delivered, leaving the batch's rewards locked until
    // a re-prove. The peer's `sender` is its `CCTPProver`, which is also the
    // recipient of our messages.
    let peer = ctx
        .accounts
        .config
        .peer(destination_domain)
        .ok_or(CctpProverError::UnknownDomain)?;
    require_keys_eq!(
        recipient,
        Pubkey::new_from_array(peer.sender.into()),
        CctpProverError::InvalidData
    );
    // Derived once: the address check and the signing bump share it.
    let (sender_authority, bump) = sender_authority_pda();
    require_keys_eq!(
        ctx.accounts.sender_authority.key(),
        sender_authority,
        CctpProverError::InvalidSenderAuthority
    );

    let accounts = &ctx.accounts;
    let instruction = cctp::send_message_instruction(
        &accounts.payer.key(),
        &sender_authority,
        &accounts.message_transmitter.key(),
        &accounts.message_sent_event_data.key(),
        &SendMessageParams {
            destination_domain,
            recipient,
            destination_caller: Pubkey::default(),
            min_finality_threshold: MIN_FINALITY_THRESHOLD,
            message_body: proof_data.to_bytes(),
        },
    )?;

    invoke_signed(
        &instruction,
        &[
            accounts.payer.to_account_info(),
            accounts.sender_authority.to_account_info(),
            accounts.message_transmitter.to_account_info(),
            accounts.message_sent_event_data.to_account_info(),
            accounts.program.to_account_info(),
            accounts.system_program.to_account_info(),
            accounts.message_transmitter_program.to_account_info(),
        ],
        &[&[SENDER_AUTHORITY_SEED, &[bump]]],
    )
    .map_err(Into::into)
}
