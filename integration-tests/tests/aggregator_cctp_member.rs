use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{GetProofArgs, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::Bytes32;
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
