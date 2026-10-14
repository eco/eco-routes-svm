use anchor_lang::AnchorDeserialize;
use cctp_prover::instructions::CctpProverError;
use eco_svm_std::prover::{GetProofArgs, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::{claimant, Bytes32};
use portal::types::intent_hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{evm_sender, peers, BASE_CHAIN_ID, BASE_DOMAIN};

pub mod common;

fn answer(context: &mut common::Context, hash: Bytes32, members: &[Pubkey]) -> Option<Proof> {
    let result = context
        .aggregator_prover()
        .get_proof(GetProofArgs::new(hash, BASE_CHAIN_ID, vec![]), members)
        .unwrap();
    Option::<Proof>::try_from_slice(&result.return_data.data).unwrap()
}

/// The aggregator reads a CCTP-delivered proof through cctp-prover's
/// `[proof]` query tail, after a member (hyper) that has none.
#[test]
fn aggregator_returns_cctp_delivered_proof() {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.cctp_prover().init(&authority, peers()).unwrap();
    context.aggregator_prover().install(authority.pubkey());
    let members = vec![hyper_prover::ID, cctp_prover::ID];
    context
        .aggregator_prover()
        .init(&authority, members.clone())
        .unwrap();
    let hash: Bytes32 = [41; 32].into();
    let claimant = Pubkey::new_unique();

    assert_eq!(answer(&mut context, hash, &members), None);

    context
        .cctp_prover()
        .deliver(
            BASE_DOMAIN,
            evm_sender(0xba),
            1000,
            ProofData::new(
                BASE_CHAIN_ID,
                vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
            ),
        )
        .unwrap();

    assert_eq!(
        answer(&mut context, hash, &members),
        Some(Proof::new(BASE_CHAIN_ID, claimant))
    );
}

/// The aggregator forwards cctp-prover's cleanup tail with its signer flags,
/// so a cancelled CCTP proof closed through it still pays only a signing
/// closer.
#[test]
fn aggregator_cleanup_of_cancelled_cctp_proof_needs_a_signing_closer() {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.aggregator_prover().install(authority.pubkey());
    context
        .aggregator_prover()
        .init(&authority, vec![hyper_prover::ID, cctp_prover::ID])
        .unwrap();
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = aggregator_prover::ID;
    let route_hash: Bytes32 = [42; 32].into();
    let hash = intent_hash(BASE_CHAIN_ID, &route_hash, &reward.hash());
    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, claimant::cancelled()),
        cctp_prover::ID,
    );
    context.warp_to_timestamp(reward.deadline.try_into().unwrap());
    let signed = context
        .aggregator_prover()
        .cleanup_accounts(&hash, &[cctp_prover::ID]);
    let recipient = signed.accounts.last().unwrap();
    assert_eq!(recipient.pubkey, context.payer.pubkey());
    assert!(recipient.is_signer);

    let mut unsigned = signed.clone();
    *unsigned.accounts.last_mut().unwrap() =
        solana_sdk::instruction::AccountMeta::new(Pubkey::new_unique(), false);
    let result = context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward.clone(), unsigned);
    assert!(result.is_err_and(common::is_program_error(
        cctp_prover::ID,
        CctpProverError::InvalidRentRecipient
    )));
    assert!(context.get_account(&proof).is_some());

    context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward, signed)
        .unwrap();
    assert!(context.get_account(&proof).is_none());
}
