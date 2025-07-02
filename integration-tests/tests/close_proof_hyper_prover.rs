use eco_svm_std::prover::Proof;
use hyper_prover::instructions::HyperProverError;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn close_proof_invalid_portal_proof_closer_fail() {
    let mut ctx = common::Context::default();
    let invalid_proof_closer = ctx.payer.insecure_clone();
    let intent_hash = [1u8; 32].into();
    let claimant = ctx.payer.pubkey();
    let destination_chain = 1u64;
    let proof_pda = Proof::pda(&intent_hash, &hyper_prover::ID).0;

    ctx.set_proof(proof_pda, Proof::new(destination_chain, claimant));

    let result = ctx.hyper_prover_close_proof(&invalid_proof_closer, proof_pda);
    assert!(result.is_err_and(common::is_portal_error(
        HyperProverError::InvalidPortalProofCloser
    )));
}
