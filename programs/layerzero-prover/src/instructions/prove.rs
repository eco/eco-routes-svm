use anchor_lang::prelude::*;
use eco_svm_std::account::create_account;
use eco_svm_std::prover::ProveArgs;
use eco_svm_std::Bytes32;

use crate::constants::MAX_INTENTS_PER_PROVE;
use crate::instructions::LayerZeroProverError;
use crate::state::{PendingSend, Store, PENDING_SEND_SEED};

#[derive(Accounts)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda(&crate::ID).0 @ LayerZeroProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: derived from the batch and validated in the handler
    #[account(mut)]
    pub pending_send: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn check_intent_count(count: usize) -> Result<()> {
    require!(count > 0, LayerZeroProverError::EmptyProof);
    require!(
        count <= MAX_INTENTS_PER_PROVE,
        LayerZeroProverError::TooManyIntents
    );

    Ok(())
}

/// Commits the batch; `send_message` dispatches it as a top-level
/// instruction because `endpoint::send` nests four frames below its caller and
/// `portal::prove` → here already uses two.
pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        domain_id,
        proof_data,
        data,
    } = args;

    let dst_eid: u32 = domain_id
        .try_into()
        .map_err(|_| LayerZeroProverError::InvalidDomainId)?;
    let peer = *ctx
        .accounts
        .store
        .peer(dst_eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    let receiver: Bytes32 = data
        .try_into()
        .map(|bytes: [u8; 32]| bytes.into())
        .map_err(|_| LayerZeroProverError::InvalidData)?;
    require!(
        receiver == peer.address,
        LayerZeroProverError::InvalidReceiver
    );
    check_intent_count(proof_data.intent_hashes_claimants.len())?;

    let payload = proof_data.to_bytes();
    let key = PendingSend::key(dst_eid, &receiver, &payload);
    let (address, bump) = PendingSend::pda_from_key(&key);
    require_keys_eq!(
        ctx.accounts.pending_send.key(),
        address,
        LayerZeroProverError::InvalidPendingSend
    );
    // The address commits to (dst_eid, receiver, payload): an account already
    // there is this same batch, still waiting for `send_message`.
    if ctx.accounts.pending_send.owner == &crate::ID {
        return Ok(());
    }

    // Sized to the batch rather than `INIT_SPACE` (a full batch), so the
    // solver fronts only the rent this batch needs until `send_message`.
    let mut data = Vec::new();
    PendingSend {
        dst_eid,
        receiver,
        payload,
        rent_payer: ctx.accounts.payer.key(),
    }
    .try_serialize(&mut data)?;
    create_account(
        &ctx.accounts.pending_send,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &crate::ID,
        data.len(),
        &[&[PENDING_SEND_SEED, &key, &[bump]]],
    )?;
    ctx.accounts
        .pending_send
        .try_borrow_mut_data()?
        .copy_from_slice(&data);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_intent_count_bounds() {
        assert!(check_intent_count(0).is_err());
        assert!(check_intent_count(1).is_ok());
        assert!(check_intent_count(MAX_INTENTS_PER_PROVE).is_ok());
        assert!(check_intent_count(MAX_INTENTS_PER_PROVE + 1).is_err());
    }
}
