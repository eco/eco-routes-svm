use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::prover::{IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::{Bytes32, CHAIN_ID};
use layerzero_prover::constants::MAX_INTENTS_PER_PROVE;
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::state::{PendingSend, Store};
use solana_sdk::instruction::Instruction;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{peers, BASE_EID};

pub mod common;

fn fulfilled(context: &mut common::Context, count: usize) -> Vec<Bytes32> {
    context
        .fulfill_rand_intents(count, layerzero_prover::ID)
        .iter()
        .map(|intent| intent.intent_hash)
        .collect()
}

#[test]
fn prove_commits_pending_send() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 2);
    let receiver = peers()[0].address;

    context
        .layerzero_prover()
        .prove(hashes.clone(), BASE_EID.into(), receiver.to_vec())
        .unwrap();

    let payload = context.layerzero_prover().payload(&hashes);
    let pending = context
        .account::<PendingSend>(&PendingSend::pda(BASE_EID, &receiver, &payload).0)
        .unwrap();
    assert_eq!(
        pending,
        PendingSend {
            dst_eid: BASE_EID,
            receiver,
            payload: payload.clone(),
            rent_payer: context.payer.pubkey(),
        }
    );
    assert_eq!(
        ProofData::from_bytes(&payload).unwrap().destination,
        CHAIN_ID
    );
}

#[test]
fn reproving_a_pending_batch_is_a_noop() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 2);
    let receiver = peers()[0].address;
    let address = context
        .layerzero_prover()
        .pending_send_for(BASE_EID, &receiver, &hashes);
    context
        .layerzero_prover()
        .prove(hashes.clone(), BASE_EID.into(), receiver.to_vec())
        .unwrap();
    let before = context.get_account(&address).unwrap();
    context.expire_blockhash();

    context
        .layerzero_prover()
        .prove(hashes, BASE_EID.into(), receiver.to_vec())
        .unwrap();

    assert_eq!(context.get_account(&address).unwrap(), before);
}

#[test]
fn prove_rejects_unknown_eid_and_wrong_receiver() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 1);
    let receiver = peers()[0].address;

    let unknown = context
        .layerzero_prover()
        .prove(hashes.clone(), 40_245, receiver.to_vec());
    assert!(unknown.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));

    let other_peer = peers()[1].address;
    let wrong =
        context
            .layerzero_prover()
            .prove(hashes.clone(), BASE_EID.into(), other_peer.to_vec());
    assert!(wrong.is_err_and(common::is_error(LayerZeroProverError::InvalidReceiver)));

    let short = context
        .layerzero_prover()
        .prove(hashes, BASE_EID.into(), receiver[..31].to_vec());
    assert!(short.is_err_and(common::is_error(LayerZeroProverError::InvalidData)));
}

#[test]
fn domain_id_above_u32_rejected() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 1);
    let wrapped = u64::from(BASE_EID) + (1u64 << 32);

    let result = context
        .layerzero_prover()
        .prove(hashes, wrapped, peers()[0].address.to_vec());

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidDomainId)));
}

#[test]
fn prove_rejects_batch_over_cap() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, MAX_INTENTS_PER_PROVE + 1);

    let result =
        context
            .layerzero_prover()
            .prove(hashes, BASE_EID.into(), peers()[0].address.to_vec());

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::TooManyIntents)));
}

/// Without portal's dispatcher signature `prove` is unreachable.
#[test]
fn prove_rejects_non_portal_caller() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let impostor = Keypair::new();
    let receiver = peers()[0].address;
    let proof_data = ProofData::new(
        CHAIN_ID,
        vec![IntentHashClaimant::new([1; 32].into(), [2; 32].into())],
    );
    let pending = PendingSend::pda(BASE_EID, &receiver, &proof_data.clone().to_bytes()).0;
    let instruction = Instruction {
        program_id: layerzero_prover::ID,
        accounts: layerzero_prover::accounts::Prove {
            portal_dispatcher: impostor.pubkey(),
            payer: context.payer.pubkey(),
            store: Store::pda().0,
            pending_send: pending,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: layerzero_prover::instruction::Prove {
            args: ProveArgs::new(BASE_EID.into(), proof_data, receiver.to_vec()),
        }
        .data(),
    };

    let result = context
        .layerzero_prover()
        .send(vec![instruction], &[&impostor]);

    assert!(result.is_err_and(common::is_error(
        LayerZeroProverError::InvalidPortalDispatcher
    )));
}
