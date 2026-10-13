use anchor_lang::{InstructionData, ToAccountMetas};
use cctp_prover::cctp::{self, HandleReceiveMessageParams};
use cctp_prover::instructions::CctpProverError;
use cctp_prover::state::{pda_payer_pda, Config, ProofAccount};
use eco_svm_std::prover::{IntentHashClaimant, IntentProven, Proof, ProofData};
use eco_svm_std::Bytes32;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{
    evm_sender, peers, proof_pdas, BASE_CHAIN_ID, BASE_DOMAIN, ETHEREUM_CHAIN_ID, ETHEREUM_DOMAIN,
    OP_DOMAIN,
};

pub mod common;

const BASE_SENDER: u8 = 0xba;
const ETHEREUM_SENDER: u8 = 0xee;

fn initialized() -> common::Context {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.cctp_prover().init(&authority, peers()).unwrap();
    context
}

fn pair(hash: u8, claimant: u8) -> IntentHashClaimant {
    IntentHashClaimant::new([hash; 32].into(), [claimant; 32].into())
}

fn recorded(context: &common::Context, hash: u8) -> Option<Proof> {
    let hash: Bytes32 = [hash; 32].into();
    context
        .account::<ProofAccount>(&Proof::pda(&hash, &cctp_prover::ID).0)
        .map(|account| account.0)
}

#[test]
fn finalized_message_from_base_records_proofs() {
    let mut context = initialized();
    let payer_before = context.balance(&pda_payer_pda().0);
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(1, 11), pair(2, 12)]);

    let result = context
        .cctp_prover()
        .deliver(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);

    assert!(
        result.is_ok_and(common::contains_cpi_event(IntentProven::new(
            [1; 32].into(),
            Pubkey::new_from_array([11; 32]),
            BASE_CHAIN_ID
        )))
    );
    assert_eq!(
        recorded(&context, 1),
        Some(Proof::new(BASE_CHAIN_ID, Pubkey::new_from_array([11; 32])))
    );
    assert_eq!(
        recorded(&context, 2),
        Some(Proof::new(BASE_CHAIN_ID, Pubkey::new_from_array([12; 32])))
    );
    assert!(context.balance(&pda_payer_pda().0) < payer_before);
}

/// Review Focus 5: Fast attestation (1000) reaches the unfinalized handler
/// and is accepted like 2000 (spec D2), from Ethereum's domain 0.
#[test]
fn unfinalized_message_from_ethereum_is_recorded() {
    let mut context = initialized();
    let data = ProofData::new(ETHEREUM_CHAIN_ID, vec![pair(3, 13)]);

    context
        .cctp_prover()
        .deliver(ETHEREUM_DOMAIN, evm_sender(ETHEREUM_SENDER), 1000, data)
        .unwrap();

    assert_eq!(
        recorded(&context, 3),
        Some(Proof::new(
            ETHEREUM_CHAIN_ID,
            Pubkey::new_from_array([13; 32])
        ))
    );
}

#[test]
fn unknown_domain_is_rejected() {
    let mut context = initialized();
    let data = ProofData::new(10, vec![pair(4, 14)]);

    let result = context
        .cctp_prover()
        .deliver(OP_DOMAIN, evm_sender(BASE_SENDER), 2000, data);

    assert!(result.is_err_and(common::is_error(CctpProverError::UnknownDomain)));
}

#[test]
fn wrong_sender_for_domain_is_rejected() {
    let mut context = initialized();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(5, 15)]);

    let result =
        context
            .cctp_prover()
            .deliver(BASE_DOMAIN, evm_sender(ETHEREUM_SENDER), 2000, data);

    assert!(result.is_err_and(common::is_error(CctpProverError::InvalidSender)));
}

/// The attested domain binds the recorded destination: Base cannot claim Ethereum.
#[test]
fn body_chain_id_must_match_attested_domain() {
    let mut context = initialized();
    let data = ProofData::new(ETHEREUM_CHAIN_ID, vec![pair(6, 16)]);

    let result = context
        .cctp_prover()
        .deliver(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);

    assert!(result.is_err_and(common::is_error(CctpProverError::ChainIdMismatch)));
    assert!(recorded(&context, 6).is_none());
}

#[test]
fn proof_accounts_must_match_pairs() {
    let mut context = initialized();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(7, 17), pair(8, 18)]);
    let mut proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);

    let short = context.cctp_prover().receive(&inbound, &proofs[..1]);
    assert!(short.is_err_and(common::is_error(CctpProverError::InvalidProof)));

    proofs.reverse();
    let swapped = context.cctp_prover().receive(&inbound, &proofs);
    assert!(swapped.is_err_and(common::is_error(CctpProverError::InvalidProof)));
}

/// Review Focus 3.
#[test]
fn later_message_same_pair_is_noop_conflict_reverts_batch() {
    let mut context = initialized();
    let sender = evm_sender(BASE_SENDER);
    context
        .cctp_prover()
        .deliver(
            BASE_DOMAIN,
            sender,
            2000,
            ProofData::new(BASE_CHAIN_ID, vec![pair(9, 19)]),
        )
        .unwrap();

    context
        .cctp_prover()
        .deliver(
            BASE_DOMAIN,
            sender,
            2000,
            ProofData::new(BASE_CHAIN_ID, vec![pair(9, 19)]),
        )
        .unwrap();

    let conflict = context.cctp_prover().deliver(
        BASE_DOMAIN,
        sender,
        2000,
        ProofData::new(BASE_CHAIN_ID, vec![pair(10, 20), pair(9, 99)]),
    );
    assert!(conflict.is_err_and(common::is_error(CctpProverError::IntentAlreadyProven)));
    assert!(recorded(&context, 10).is_none());
    assert_eq!(
        recorded(&context, 9).unwrap().claimant,
        Pubkey::new_from_array([19; 32])
    );
}

/// Review Focus 4.
#[test]
fn duplicate_pair_in_one_message_records_once() {
    let mut context = initialized();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(21, 31), pair(21, 31)]);

    context
        .cctp_prover()
        .deliver(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data)
        .unwrap();

    assert_eq!(
        recorded(&context, 21).unwrap().claimant,
        Pubkey::new_from_array([31; 32])
    );
}

/// Review Focus 2: the whole `receive_message` reverts, so the nonce stays
/// unused and the same message + attestation succeed after a refill.
#[test]
fn empty_pda_payer_fails_retryably() {
    let mut context = initialized();
    let reserve = pda_payer_pda().0;
    let mut account = context.get_account(&reserve).unwrap();
    account.lamports = 0;
    context.set_account(reserve, account).unwrap();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(22, 32)]);
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);

    assert!(context.cctp_prover().receive(&inbound, &proofs).is_err());

    context.airdrop(&reserve, common::sol_amount(1.0)).unwrap();
    context.cctp_prover().receive(&inbound, &proofs).unwrap();
    assert!(recorded(&context, 22).is_some());
}

/// Review Focus 1: a griefer's lamport transfer to the Proof PDA cannot block delivery.
#[test]
fn receive_creates_proof_at_prefunded_pda() {
    let mut context = initialized();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(23, 33)]);
    context.airdrop(&proof_pdas(&data)[0], 1_000_000).unwrap();

    context
        .cctp_prover()
        .deliver(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data)
        .unwrap();

    assert!(recorded(&context, 23).is_some());
}

#[test]
fn replayed_message_is_rejected_by_transmitter() {
    let mut context = initialized();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(24, 34)]);
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);
    context.cctp_prover().receive(&inbound, &proofs).unwrap();

    assert!(context.cctp_prover().receive(&inbound, &proofs).is_err());
}

/// Without the transmitter's authority signature nothing is recorded.
#[test]
fn direct_handler_call_is_rejected() {
    let mut context = initialized();
    let impostor = Keypair::new();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(25, 35)]);
    let mut accounts = cctp_prover::accounts::HandleReceiveMessage {
        authority_pda: impostor.pubkey(),
        config: Config::pda().0,
        pda_payer: pda_payer_pda().0,
        system_program: anchor_lang::system_program::ID,
        event_authority: eco_svm_std::event_authority_pda(&cctp_prover::ID).0,
        program: cctp_prover::ID,
    }
    .to_account_metas(None);
    accounts[0].is_signer = true;
    accounts.extend(
        proof_pdas(&data)
            .into_iter()
            .map(|proof| AccountMeta::new(proof, false)),
    );
    let ix = Instruction {
        program_id: cctp_prover::ID,
        accounts,
        data: cctp_prover::instruction::HandleReceiveFinalizedMessage {
            params: HandleReceiveMessageParams {
                remote_domain: BASE_DOMAIN,
                sender: Pubkey::new_from_array(evm_sender(BASE_SENDER)),
                finality_threshold_executed: 2000,
                message_body: data.to_bytes(),
                authority_bump: cctp::authority_pda(&cctp_prover::ID).1,
            },
        }
        .data(),
    };

    let result = context.cctp_prover().send(vec![ix], &[&impostor]);

    assert!(result.is_err_and(common::is_error(
        CctpProverError::InvalidTransmitterAuthority
    )));
    assert!(recorded(&context, 25).is_none());
}
