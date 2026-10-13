use anchor_lang::{InstructionData, Space, ToAccountMetas};
use cctp_prover::cctp::{self, HandleReceiveMessageParams};
use cctp_prover::instructions::CctpProverError;
use cctp_prover::state::{Config, ProofAccount};
use eco_svm_std::prover::{IntentHashClaimant, IntentProven, Proof, ProofData};
use eco_svm_std::Bytes32;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{
    evm_sender, peers, proof_pdas, receive_instruction, Inbound, BASE_CHAIN_ID, BASE_DOMAIN,
    ETHEREUM_CHAIN_ID, ETHEREUM_DOMAIN, OP_DOMAIN, TAIL_PAYER_INDEX,
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

fn proof_rent() -> u64 {
    common::Context::default()
        .get_sysvar::<Rent>()
        .minimum_balance(8 + ProofAccount::INIT_SPACE)
}

fn nonce_used(context: &common::Context, inbound: &Inbound) -> bool {
    let nonce = cctp::nonce(&inbound.message).unwrap();
    context
        .get_account(&cctp::used_nonce_pda(&nonce).0)
        .is_some()
}

/// The relayer, not the context's fee payer, pays exactly Circle's
/// `used_nonce` rent plus one `Proof` rent per pair.
#[test]
fn finalized_message_from_base_records_proofs() {
    let mut context = initialized();
    let relayer = Keypair::new();
    context
        .airdrop(&relayer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let relayer_before = context.balance(&relayer.pubkey());
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(1, 11), pair(2, 12)]);
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);

    let result = context
        .cctp_prover()
        .receive_by(&relayer, &inbound, &proofs);

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
    let nonce = cctp::nonce(&inbound.message).unwrap();
    let used_nonce_rent = context
        .get_account(&cctp::used_nonce_pda(&nonce).0)
        .unwrap()
        .lamports;
    assert_eq!(
        relayer_before - context.balance(&relayer.pubkey()),
        used_nonce_rent + 2 * proof_rent()
    );
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

/// Review Focus 2: a relayer too poor for the `Proof` rent reverts the whole
/// `receive_message`, so the nonce stays unused and the same message +
/// attestation succeed once the relayer is funded.
#[test]
fn underfunded_payer_fails_retryably() {
    let mut context = initialized();
    let relayer = Keypair::new();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(22, 32)]);
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);
    // Enough for Circle's `used_nonce` (8-byte discriminator + `is_used`),
    // one lamport short of the `Proof` that follows.
    let used_nonce_rent = context.get_sysvar::<Rent>().minimum_balance(8 + 1);
    context
        .airdrop(&relayer.pubkey(), used_nonce_rent + proof_rent() - 1)
        .unwrap();

    let result = context
        .cctp_prover()
        .receive_by(&relayer, &inbound, &proofs);

    assert!(
        result
            .clone()
            .is_err_and(common::reached_program(cctp_prover::ID)),
        "{result:?}"
    );
    assert!(result
        .unwrap_err()
        .meta
        .logs
        .iter()
        .any(|log| log.contains("insufficient lamports")));
    assert!(recorded(&context, 22).is_none());
    assert!(!nonce_used(&context, &inbound));

    context.airdrop(&relayer.pubkey(), 1).unwrap();
    context
        .cctp_prover()
        .receive_by(&relayer, &inbound, &proofs)
        .unwrap();
    assert!(recorded(&context, 22).is_some());
    assert!(nonce_used(&context, &inbound));
}

/// Circle forwards the tail's signer flags, so a relayer cannot name someone
/// else's account as the rent payer: unsigned, it is rejected and untouched.
#[test]
fn unsigned_payer_is_rejected() {
    let mut context = initialized();
    let victim = Pubkey::new_unique();
    context.airdrop(&victim, common::sol_amount(1.0)).unwrap();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(28, 38)]);
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);
    let payer = context.payer.pubkey();
    let mut ix = receive_instruction(&payer, &inbound, &proofs);
    ix.accounts[TAIL_PAYER_INDEX] = AccountMeta::new(victim, false);

    let result = context.cctp_prover().send(vec![ix], &[]);

    assert!(result.is_err_and(common::is_program_error(
        cctp_prover::ID,
        anchor_lang::error::ErrorCode::AccountNotSigner
    )));
    assert_eq!(context.balance(&victim), common::sol_amount(1.0));
    assert!(recorded(&context, 28).is_none());
    assert!(!nonce_used(&context, &inbound));
}

/// The transmitter signs its authority PDA in the CPI and duplicate accounts
/// share signer flags, so naming that PDA as the tail payer would reach us
/// signed; it is rejected, and lamports sent to the PDA stay put.
#[test]
fn authority_pda_as_payer_is_rejected() {
    let mut context = initialized();
    let authority = cctp::authority_pda(&cctp_prover::ID).0;
    context
        .airdrop(&authority, common::sol_amount(1.0))
        .unwrap();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(29, 39)]);
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data);
    let payer = context.payer.pubkey();
    let mut ix = receive_instruction(&payer, &inbound, &proofs);
    ix.accounts[TAIL_PAYER_INDEX] = AccountMeta::new(authority, false);

    let result = context.cctp_prover().send(vec![ix], &[]);

    assert!(result.is_err_and(common::is_program_error(
        cctp_prover::ID,
        CctpProverError::InvalidRentRecipient
    )));
    assert_eq!(context.balance(&authority), common::sol_amount(1.0));
    assert!(recorded(&context, 29).is_none());
    assert!(!nonce_used(&context, &inbound));
}

/// Review Focus 1: a griefer's lamport transfer to the Proof PDA cannot block
/// delivery, below the rent minimum (top-up branch) or at/above it (no top-up).
#[test]
fn receive_creates_proof_at_prefunded_pda() {
    let rent_minimum = common::Context::default()
        .get_sysvar::<Rent>()
        .minimum_balance(8 + ProofAccount::INIT_SPACE);

    for (hash, claimant, prefund) in [
        (23, 33, 1_000_000),
        (26, 36, rent_minimum),
        (27, 37, rent_minimum + 1_000_000),
    ] {
        let mut context = initialized();
        let data = ProofData::new(BASE_CHAIN_ID, vec![pair(hash, claimant)]);
        context.airdrop(&proof_pdas(&data)[0], prefund).unwrap();

        context
            .cctp_prover()
            .deliver(BASE_DOMAIN, evm_sender(BASE_SENDER), 2000, data)
            .unwrap();

        assert_eq!(
            recorded(&context, hash),
            Some(Proof::new(
                BASE_CHAIN_ID,
                Pubkey::new_from_array([claimant; 32])
            ))
        );
    }
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

    let replay = context
        .cctp_prover()
        .receive(&inbound, &proofs)
        .unwrap_err();

    assert!(
        replay
            .meta
            .logs
            .iter()
            .any(|log| log.contains("already in use")),
        "{:?}",
        replay.meta.logs
    );
}

/// A direct `handle_receive_finalized_message` call: `authority` is the
/// account in the authority slot, `signer` signs it when it is a keypair.
fn direct_call(
    context: &mut common::Context,
    authority: Pubkey,
    signer: Option<&Keypair>,
    authority_bump: u8,
    data: &ProofData,
) -> common::TransactionResult {
    let mut accounts = cctp_prover::accounts::HandleReceiveMessage {
        authority_pda: authority,
        config: Config::pda().0,
        payer: context.payer.pubkey(),
        system_program: anchor_lang::system_program::ID,
        event_authority: eco_svm_std::event_authority_pda(&cctp_prover::ID).0,
        program: cctp_prover::ID,
    }
    .to_account_metas(None);
    accounts[0].is_signer = signer.is_some();
    accounts.extend(
        proof_pdas(data)
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
                message_body: data.clone().to_bytes(),
                authority_bump,
            },
        }
        .data(),
    };

    context
        .cctp_prover()
        .send(vec![ix], &signer.into_iter().collect::<Vec<_>>())
}

/// Without the transmitter's authority signature nothing is recorded.
#[test]
fn direct_handler_call_is_rejected() {
    let mut context = initialized();
    let impostor = Keypair::new();
    let data = ProofData::new(BASE_CHAIN_ID, vec![pair(25, 35)]);
    let (real, bump) = cctp::authority_pda(&cctp_prover::ID);

    let signed_impostor = direct_call(
        &mut context,
        impostor.pubkey(),
        Some(&impostor),
        bump,
        &data,
    );
    assert!(signed_impostor.is_err_and(common::is_error(
        CctpProverError::InvalidTransmitterAuthority
    )));

    // The real PDA cannot sign outside the transmitter's CPI.
    let unsigned_real = direct_call(&mut context, real, None, bump, &data);
    assert!(unsigned_real.is_err_and(common::is_error(
        anchor_lang::error::ErrorCode::AccountNotSigner
    )));

    // A signer under a non-canonical bump no more matches the derived address.
    let non_canonical = (0..=u8::MAX)
        .find(|candidate| {
            *candidate != bump
                && cctp::authority_pda_with_bump(&cctp_prover::ID, *candidate).is_some()
        })
        .unwrap();
    let wrong_bump = direct_call(
        &mut context,
        impostor.pubkey(),
        Some(&impostor),
        non_canonical,
        &data,
    );
    assert!(wrong_bump.is_err_and(common::is_error(
        CctpProverError::InvalidTransmitterAuthority
    )));
    assert!(recorded(&context, 25).is_none());
}
