use anchor_lang::{InstructionData, ToAccountMetas};
use cctp_prover::cctp::{self, MessageSentAccount, MIN_FINALITY_THRESHOLD, SOLANA_DOMAIN};
use cctp_prover::constants::MAX_INTENTS_PER_PROVE;
use cctp_prover::instructions::CctpProverError;
use eco_svm_std::prover::{IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::CHAIN_ID;
use portal::state::FulfillMarker;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{evm_recipient, BASE_DOMAIN};

pub mod common;

fn setup(n: usize) -> (common::Context, Vec<common::FulfilledIntent>) {
    let mut context = common::Context::default();
    context.cctp_prover().install(Pubkey::new_unique());
    let intents = context.fulfill_rand_intents(n, cctp_prover::ID);
    (context, intents)
}

fn markers(intents: &[common::FulfilledIntent]) -> Vec<Pubkey> {
    intents
        .iter()
        .map(|intent| FulfillMarker::pda(&intent.intent_hash).0)
        .collect()
}

#[test]
fn prove_sends_fast_message_to_evm_prover() {
    let (mut context, intents) = setup(2);
    let event = Keypair::new();
    let hashes: Vec<_> = intents.iter().map(|i| i.intent_hash).collect();

    context
        .portal()
        .prove_intent_via_cctp_prover(
            hashes.clone(),
            BASE_DOMAIN as u64,
            markers(&intents),
            evm_recipient(0xcc),
            &event,
        )
        .unwrap();

    let account = context.get_account(&event.pubkey()).unwrap();
    assert_eq!(account.owner, cctp::MESSAGE_TRANSMITTER_V2_ID);
    let sent = MessageSentAccount::try_from_data(&account.data).unwrap();
    assert_eq!(sent.rent_payer, context.payer.pubkey());
    let m = &sent.message;
    assert_eq!(m[4..8], SOLANA_DOMAIN.to_be_bytes());
    assert_eq!(m[8..12], BASE_DOMAIN.to_be_bytes());
    assert_eq!(m[cctp::NONCE_INDEX..cctp::SENDER_INDEX], [0; 32]);
    assert_eq!(
        m[cctp::SENDER_INDEX..cctp::RECIPIENT_INDEX],
        cctp_prover::ID.to_bytes()
    );
    assert_eq!(
        m[cctp::RECIPIENT_INDEX..cctp::DESTINATION_CALLER_INDEX],
        evm_recipient(0xcc)[..]
    );
    assert_eq!(
        m[cctp::DESTINATION_CALLER_INDEX..cctp::MIN_FINALITY_THRESHOLD_INDEX],
        [0; 32]
    );
    assert_eq!(
        m[cctp::MIN_FINALITY_THRESHOLD_INDEX..cctp::FINALITY_THRESHOLD_EXECUTED_INDEX],
        MIN_FINALITY_THRESHOLD.to_be_bytes()
    );
    let pairs = intents
        .iter()
        .map(|intent| {
            let marker = context
                .account::<FulfillMarker>(&FulfillMarker::pda(&intent.intent_hash).0)
                .unwrap();
            IntentHashClaimant::new(intent.intent_hash, marker.claimant)
        })
        .collect();
    assert_eq!(
        m[cctp::MESSAGE_BODY_INDEX..],
        ProofData::new(CHAIN_ID, pairs).to_bytes()[..]
    );
}

#[test]
fn prove_sends_to_ethereum_domain_zero() {
    let (mut context, intents) = setup(1);
    let event = Keypair::new();

    context
        .portal()
        .prove_intent_via_cctp_prover(
            vec![intents[0].intent_hash],
            0,
            markers(&intents),
            evm_recipient(0xee),
            &event,
        )
        .unwrap();

    let sent =
        MessageSentAccount::try_from_data(&context.get_account(&event.pubkey()).unwrap().data)
            .unwrap();
    assert_eq!(sent.message[8..12], 0u32.to_be_bytes());
}

#[test]
fn prove_rejects_bad_domains() {
    for domain in [SOLANA_DOMAIN as u64, u32::MAX as u64 + 1] {
        let (mut context, intents) = setup(1);
        let result = context.portal().prove_intent_via_cctp_prover(
            vec![intents[0].intent_hash],
            domain,
            markers(&intents),
            evm_recipient(0xcc),
            &Keypair::new(),
        );
        assert!(result.is_err_and(common::is_error(CctpProverError::InvalidDomainId)));
    }
}

#[test]
fn prove_rejects_bad_recipient_data() {
    for data in [vec![1u8; 31], vec![1u8; 33], vec![0u8; 32]] {
        let (mut context, intents) = setup(1);
        let result = context.portal().prove_intent_via_cctp_prover(
            vec![intents[0].intent_hash],
            BASE_DOMAIN as u64,
            markers(&intents),
            data,
            &Keypair::new(),
        );
        assert!(result.is_err_and(common::is_error(CctpProverError::InvalidData)));
    }
}

/// Only Portal's prover-scoped dispatcher may call `prove`.
#[test]
fn prove_rejects_direct_caller() {
    let (mut context, intents) = setup(1);
    let impostor = Keypair::new();
    let event = Keypair::new();
    let payer = context.payer.pubkey();
    let args = ProveArgs::new(
        BASE_DOMAIN as u64,
        ProofData::new(
            CHAIN_ID,
            vec![IntentHashClaimant::new(
                intents[0].intent_hash,
                [9; 32].into(),
            )],
        ),
        evm_recipient(0xcc),
    );
    let mut accounts = cctp_prover::accounts::Prove {
        portal_dispatcher: impostor.pubkey(),
        sender_authority: cctp_prover::state::sender_authority_pda().0,
        payer,
        message_transmitter: cctp::message_transmitter_pda().0,
        message_sent_event_data: event.pubkey(),
        program: cctp_prover::ID,
        system_program: anchor_lang::system_program::ID,
        message_transmitter_program: cctp::MESSAGE_TRANSMITTER_V2_ID,
    }
    .to_account_metas(None);
    accounts[0].is_signer = true;
    let ix = Instruction {
        program_id: cctp_prover::ID,
        accounts,
        data: cctp_prover::instruction::Prove { args }.data(),
    };

    let result = context.cctp_prover().send(vec![ix], &[&impostor, &event]);

    assert!(result.is_err_and(common::is_error(CctpProverError::InvalidPortalDispatcher)));
}

#[test]
fn prove_rejects_batch_over_max_intents() {
    let (mut context, intents) = setup(MAX_INTENTS_PER_PROVE + 1);
    let hashes = intents.iter().map(|intent| intent.intent_hash).collect();

    let result = context.portal().prove_intent_via_cctp_prover(
        hashes,
        BASE_DOMAIN as u64,
        markers(&intents),
        evm_recipient(0xcc),
        &Keypair::new(),
    );

    assert!(result.is_err_and(common::is_error(CctpProverError::TooManyIntents)));
}
