use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{IntentProven, Proof, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::instructions::AggregatorProverError;
use crate::state::{Config, ProofAccount};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AggregateArgs {
    pub destination: u64,
    pub intent_hash: Bytes32,
    pub claimant: Bytes32,
    pub route_hash: Bytes32,
    pub reward_hash: Bytes32,
}

#[event_cpi]
#[derive(Accounts)]
pub struct Aggregate<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    pub system_program: Program<'info, System>,
}

pub fn aggregate<'info>(ctx: Context<'info, Aggregate<'info>>, args: AggregateArgs) -> Result<()> {
    let AggregateArgs {
        destination,
        intent_hash,
        claimant,
        route_hash,
        reward_hash,
    } = args;
    let accounts = ctx.remaining_accounts;
    require!(
        accounts.len() == 1 + ctx.accounts.config.provers.len(),
        AggregatorProverError::InvalidProof
    );
    require!(
        portal::types::intent_hash(destination, &route_hash, &reward_hash) == intent_hash,
        AggregatorProverError::InvalidIntentHash
    );
    let selected = select_proof(
        &ctx.accounts.config.provers,
        &accounts[1..],
        &intent_hash,
        destination,
    )?;
    require!(
        claimant == selected.claimant,
        AggregatorProverError::ClaimantMismatch
    );
    let (proof_address, bump) = Proof::pda(&intent_hash, &crate::ID);
    let proof_account = &accounts[0];
    require_keys_eq!(
        proof_account.key(),
        proof_address,
        AggregatorProverError::InvalidProof
    );

    if proof_account.owner == &crate::ID {
        let recorded = ProofAccount::try_deserialize(&mut &proof_account.try_borrow_data()?[..])?;
        require!(
            recorded.0.destination == selected.destination
                && recorded.0.claimant == selected.claimant,
            AggregatorProverError::IntentAlreadyProven
        );
    } else {
        ProofAccount(selected).init(
            proof_account,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &[&[PROOF_SEED, intent_hash.as_ref(), &[bump]]],
        )?;
    }
    emit_cpi!(IntentProven::new(
        intent_hash,
        Pubkey::new_from_array(claimant.into()),
        destination
    ));

    Ok(())
}

fn select_proof(
    provers: &[Pubkey],
    accounts: &[AccountInfo],
    intent_hash: &Bytes32,
    destination: u64,
) -> Result<Proof> {
    // Validate the whole list before selecting: callers cannot hide a higher-priority proof.
    provers
        .iter()
        .zip(accounts)
        .try_for_each(|(prover, account)| {
            require_keys_eq!(
                account.key(),
                Proof::pda(intent_hash, prover).0,
                AggregatorProverError::InvalidProof
            );

            Ok(())
        })?;

    provers
        .iter()
        .zip(accounts)
        .find_map(|(prover, account)| {
            if account.owner != prover {
                return None;
            }
            let data = account.try_borrow_data().ok()?;
            if !data.starts_with(ProofAccount::DISCRIMINATOR) {
                return None;
            }
            let proof = Proof::try_from_account_info(account).ok()??;
            (proof.destination == destination && proof.claimant != Pubkey::default())
                .then_some(proof)
        })
        .ok_or_else(|| AggregatorProverError::NoMatchingProof.into())
}
