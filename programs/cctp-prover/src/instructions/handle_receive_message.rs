use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::cctp::{self, HandleReceiveMessageParams};
use crate::instructions::CctpProverError;
use crate::state::{Config, ProofAccount};

/// Shared by both `handle_receive_*` entrypoints. The transmitter has already
/// verified Circle's attestation (which signs `remote_domain` and `sender`),
/// the destination domain, the recipient and the nonce; it passes its
/// authority PDA as signer, then forwards the relayer's remaining accounts
/// with their signer and writable flags: these four, our `#[event_cpi]` pair,
/// then one `Proof` PDA per pair. It does not forward its own payer, so the
/// relayer signs `payer` again in the tail and that signature pays the `Proof`
/// rent. The only CPIs here are system-program creates and our own event, so
/// Circle's self-reentrancy warning does not apply.
#[event_cpi]
#[derive(Accounts)]
#[instruction(params: HandleReceiveMessageParams)]
pub struct HandleReceiveMessage<'info> {
    /// Circle's authority PDA for this program, derived from
    /// `["message_transmitter_authority", crate::ID]` under the transmitter
    /// with the bump it passes. Only the transmitter can sign for it, so any
    /// other caller is rejected before the handler body. Anchor's `bump = ..`
    /// cannot carry a custom error, hence the explicit constraint.
    #[account(
        constraint = cctp::authority_pda_with_bump(&crate::ID, params.authority_bump)
            == Some(authority_pda.key())
            @ CctpProverError::InvalidTransmitterAuthority,
    )]
    pub authority_pda: Signer<'info>,
    #[account(address = Config::pda().0 @ CctpProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// The relayer: funds every new `Proof`, refunded to its claimant on
    /// `close_proof`. Any signer but `authority_pda` will do (checked in the
    /// handler); there is no protocol reserve.
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// `finality_threshold_executed` is not checked: always fast (spec D2) means
/// 1000 and 2000 are both accepted from every configured peer. That is an
/// accepted risk, not an oversight: a source-chain reorg after a 1000
/// attestation leaves a permanent `Proof` for a fill that no longer exists.
/// Circle backs fast USDC burns with its Fast Transfer Allowance, but not
/// generic messages; it leaves their finality to the recipient.
pub fn handle_receive_message<'info>(
    ctx: Context<'info, HandleReceiveMessage<'info>>,
    params: HandleReceiveMessageParams,
) -> Result<()> {
    // The transmitter signs `authority_pda` in its CPI and duplicate accounts
    // share signer flags, so a relayer naming it as `payer` would spend
    // whatever lamports were sent to that PDA.
    require_keys_neq!(
        ctx.accounts.payer.key(),
        ctx.accounts.authority_pda.key(),
        CctpProverError::InvalidPayer
    );
    let peer = *ctx
        .accounts
        .config
        .peer(params.remote_domain)
        .ok_or(CctpProverError::UnknownDomain)?;
    let sender: Bytes32 = params.sender.to_bytes().into();
    require!(peer.sender == sender, CctpProverError::InvalidSender);
    // Circle attests `remote_domain`, so the self-reported body header cannot
    // claim a chain other than the peer's (EVM `_handleCrossChainMessage` rule).
    let proof_data = ProofData::from_bytes(&params.message_body)?;
    let destination = proof_data.destination;
    require!(
        destination == peer.chain_id,
        CctpProverError::ChainIdMismatch
    );
    require!(
        ctx.remaining_accounts.len() == proof_data.intent_hashes_claimants.len(),
        CctpProverError::InvalidProof
    );

    ctx.remaining_accounts
        .iter()
        .zip(proof_data.intent_hashes_claimants)
        .try_for_each(|(proof, pair)| mark_intent_hash_proven(&ctx, proof, destination, pair))
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'info, HandleReceiveMessage<'info>>,
    proof: &AccountInfo<'info>,
    destination: u64,
    pair: IntentHashClaimant,
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = pair;
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, proof_bump) = prover::Proof::pda(&intent_hash, &crate::ID);
    require_keys_eq!(proof.key(), proof_pda, CctpProverError::InvalidProof);

    // A `Proof` can already exist for a duplicate pair earlier in this batch,
    // or from a later message (new nonce, e.g. a re-prove) carrying the same
    // pair. Reaching the recorded state again is a no-op; only a disagreeing
    // state is an error, and it reverts the whole delivery. The event repeats
    // either way.
    match prover::Proof::try_from_account_info(proof)? {
        Some(recorded) => require!(
            recorded.destination == destination && recorded.claimant == claimant,
            CctpProverError::IntentAlreadyProven
        ),
        None => {
            let proof_account: ProofAccount = prover::Proof::new(destination, claimant).into();
            proof_account.init(
                proof,
                &ctx.accounts.payer,
                &ctx.accounts.system_program,
                &[&[PROOF_SEED, intent_hash.as_ref(), &[proof_bump]]],
            )?
        }
    }

    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}
