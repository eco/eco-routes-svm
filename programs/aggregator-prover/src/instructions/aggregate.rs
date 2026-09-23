use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{IntentHashClaimant, IntentProven, Proof, ProofData, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::instructions::AggregatorProverError;
use crate::state::{Config, ProofAccount};

/// One destination-bound preimage per intent, in proof_data order.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct IntentPreimage {
    pub route_hash: Bytes32,
    pub reward_hash: Bytes32,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AggregateArgs {
    pub proof_data: ProofData,
    pub preimages: Vec<IntentPreimage>,
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
        proof_data,
        preimages,
    } = args;
    let intents = proof_data.intent_hashes_claimants;
    require!(
        !intents.is_empty() && preimages.len() == intents.len(),
        AggregatorProverError::InvalidData
    );
    let chunk_size = 1 + ctx.accounts.config.members.len();
    require!(
        ctx.remaining_accounts.len() == intents.len() * chunk_size,
        AggregatorProverError::InvalidProof
    );

    intents
        .into_iter()
        .zip(preimages)
        .zip(ctx.remaining_accounts.chunks_exact(chunk_size))
        .try_for_each(|((intent, preimage), accounts)| {
            record_proof(&ctx, proof_data.destination, intent, preimage, accounts)
        })
}

fn record_proof<'info>(
    ctx: &Context<'info, Aggregate<'info>>,
    destination: u64,
    intent: IntentHashClaimant,
    preimage: IntentPreimage,
    accounts: &[AccountInfo<'info>],
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = intent;
    let IntentPreimage {
        route_hash,
        reward_hash,
    } = preimage;
    require!(
        portal::types::intent_hash(destination, &route_hash, &reward_hash) == intent_hash,
        AggregatorProverError::InvalidIntentHash
    );
    let selected = select_proof(
        &ctx.accounts.config.members,
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
    members: &[Pubkey],
    accounts: &[AccountInfo],
    intent_hash: &Bytes32,
    destination: u64,
) -> Result<Proof> {
    // Validate the whole list before selecting: callers cannot hide a higher-priority proof.
    members
        .iter()
        .zip(accounts)
        .try_for_each(|(member, account)| {
            require_keys_eq!(
                account.key(),
                Proof::pda(intent_hash, member).0,
                AggregatorProverError::InvalidProof
            );

            Ok(())
        })?;

    members
        .iter()
        .zip(accounts)
        .find_map(|(member, account)| {
            if account.owner != member {
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
