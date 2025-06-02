use anchor_lang::prelude::*;
use itertools::izip;

use crate::{encoding, error::EcoRoutesError, state::Intent};

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
    let accounts: Vec<_> = ctx.remaining_accounts.iter().collect();

    require!(
        intent_hashes.len() == solvers.len(),
        EcoRoutesError::InvalidHandlePayload
    );
    require!(
        intent_hashes.len() == accounts.len(),
        EcoRoutesError::InvalidHandlePayload
    );

    for (intent_hash, solver, account) in izip!(intent_hashes, solvers, accounts) {
        require_keys_eq!(
            account.key(),
            Intent::pda(intent_hash).0,
            EcoRoutesError::InvalidIntent
        );
        require!(account.is_writable, EcoRoutesError::InvalidIntent);

        let mut intent: Account<Intent> = Account::try_from(account)?;

        require!(intent.route.inbox == sender, EcoRoutesError::InvalidSender);
        require!(
            intent.route.destination_domain_id == origin,
            EcoRoutesError::InvalidOrigin
        );

        intent.fulfill(solver)?;
        intent.exit(ctx.program_id)?;
    }

    Ok(())
}
