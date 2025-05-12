use anchor_lang::prelude::*;

use crate::{
    encoding,
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(Accounts)]
pub struct Handle<'info> {
    #[account(address = expected_process_authority() @ EcoRoutesError::InvalidProcessAuthority)]
    pub prover_process_authority: Signer<'info>,
}

pub fn expected_process_authority() -> Pubkey {
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

pub fn handle<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, Handle<'info>>,
    origin: u32,
    sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    let (intent_hashes, solvers) = encoding::decode_fulfillment_message(&payload)
        .map_err(|_| error!(EcoRoutesError::InvalidHandlePayload))?;

    let mut remaining_accounts = ctx.remaining_accounts.iter();

    for (intent_hash, solver) in intent_hashes.iter().zip(solvers.iter()) {
        let intent_account_info = remaining_accounts
            .next()
            .ok_or_else(|| error!(EcoRoutesError::InvalidIntent))?;

        require_keys_eq!(
            intent_account_info.key(),
            Intent::pda(*intent_hash).0,
            EcoRoutesError::InvalidIntent
        );
        require!(
            intent_account_info.is_writable,
            EcoRoutesError::InvalidIntent
        );

        let mut intent: Account<Intent> = Account::try_from(intent_account_info)?;

        require!(intent.route.inbox == sender, EcoRoutesError::InvalidSender);
        require!(
            intent.route.destination_domain_id == origin,
            EcoRoutesError::InvalidOrigin
        );

        intent.status = IntentStatus::Fulfilled;
        intent.solver = Some(*solver);

        intent.exit(ctx.program_id)?;
    }

    Ok(())
}
