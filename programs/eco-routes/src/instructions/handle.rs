use anchor_lang::prelude::*;

use crate::{
    encoding,
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(Accounts)]
pub struct Handle<'info> {
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

pub fn handle<'info>(
    ctx: Context<'_, '_, '_, 'info, Handle<'info>>,
    origin: u32,
    sender: [u8; 32],
    payload: Vec<u8>,
) -> Result<()> {
    let prover_process_authority = ctx.accounts.prover_process_authority.key();

    let (intent_hashes, solvers) = encoding::decode_fulfillment_message(&payload)
        .map_err(|_| error!(EcoRoutesError::InvalidHandlePayload))?;

    let mut remaining_accounts = ctx.remaining_accounts.iter();

    for (intent_hash, solver) in intent_hashes.iter().zip(solvers.iter()) {
        let intent = remaining_accounts
            .next()
            .ok_or(EcoRoutesError::InvalidIntent)?;

        if intent.key() != Intent::pda(*intent_hash).0 {
            return Err(EcoRoutesError::InvalidIntent.into());
        }

        if !intent.is_writable {
            return Err(EcoRoutesError::InvalidIntent.into());
        }

        let mut data = intent.try_borrow_mut_data()?;
        let mut intent_state = Intent::try_from_slice(&data)?;

        if intent_state.route.inbox != sender {
            return Err(EcoRoutesError::InvalidSender.into());
        }

        if expected_process_authority() != prover_process_authority {
            return Err(EcoRoutesError::InvalidProver.into());
        }

        if intent_state.route.destination_domain_id != origin {
            return Err(EcoRoutesError::InvalidOrigin.into());
        }

        intent_state.status = IntentStatus::Fulfilled;
        intent_state.solver = *solver;

        intent_state.serialize(&mut *data)?;
    }

    Ok(())
}
