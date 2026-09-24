use std::iter;

use aggregator_prover::instructions::AggregatorProverError;
use aggregator_prover::state::{Config, ProofAccount, MAX_PROVERS};
use anchor_lang::error::ErrorCode;
use anchor_lang::{AnchorDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::{
    IntentHashClaimant, IntentProven, Proof, ProofData, ProveArgs, PROVE_DISCRIMINATOR,
};
use eco_svm_std::{Bytes32, CHAIN_ID};
use hyper_prover::hyperlane::MailboxInstruction;
use portal::instructions::PortalError;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

const PROVERS: [Pubkey; 2] = [hyper_prover::ID, local_prover::ID];
const DESTINATION: u64 = 10;

fn setup() -> (common::Context, Keypair) {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.aggregator_prover().install(authority.pubkey());

    (context, authority)
}

fn initialized() -> common::Context {
    let (mut context, authority) = setup();
    context
        .aggregator_prover()
        .init(&authority, PROVERS.to_vec())
        .unwrap();

    context
}

fn test_intent_hash() -> Bytes32 {
    [42; 32].into()
}

fn set_prover_proof(
    context: &mut common::Context,
    hash: &Bytes32,
    prover: Pubkey,
    destination: u64,
    claimant: Pubkey,
) {
    context.set_proof(
        Proof::pda(hash, &prover).0,
        Proof::new(destination, claimant),
        prover,
    );
}

fn aggregate_address(hash: &Bytes32) -> Pubkey {
    Proof::pda(hash, &aggregator_prover::ID).0
}

#[test]
fn init_preserves_order_and_cannot_be_reinitialized() {
    let (mut context, authority) = setup();
    context.airdrop(&Config::pda().0, 1_000_000).unwrap();
    context
        .aggregator_prover()
        .init(&authority, PROVERS.to_vec())
        .unwrap();
    let config: Config = context.account(&Config::pda().0).unwrap();
    assert_eq!(config.provers, PROVERS);
    let result = context
        .aggregator_prover()
        .init(&authority, PROVERS.into_iter().rev().collect());
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}

#[test]
fn init_rejects_wrong_authority() {
    let (mut context, _) = setup();
    let result = context
        .aggregator_prover()
        .init(&Keypair::new(), PROVERS.to_vec());
    assert!(result.is_err_and(common::is_error(AggregatorProverError::InvalidAuthority)));
}

#[test]
fn init_rejects_invalid_prover_sets() {
    for provers in [vec![], vec![PROVERS[0]; MAX_PROVERS + 1]] {
        let (mut context, authority) = setup();
        let result = context.aggregator_prover().init(&authority, provers);
        assert!(result.is_err_and(common::is_error(AggregatorProverError::InvalidProverSet)));
    }
}

#[test]
fn init_rejects_duplicate_provers() {
    let (mut context, authority) = setup();
    let result = context
        .aggregator_prover()
        .init(&authority, vec![PROVERS[0]; 2]);
    assert!(result.is_err_and(common::is_error(AggregatorProverError::DuplicateProver)));
}

#[test]
fn init_rejects_non_executable_zero_and_self_provers() {
    for prover in [
        Pubkey::new_unique(),
        Pubkey::default(),
        aggregator_prover::ID,
    ] {
        let (mut context, authority) = setup();
        let result = context.aggregator_prover().init(&authority, vec![prover]);
        assert!(result.is_err_and(common::is_error(AggregatorProverError::InvalidProver)));
    }
}

#[test]
fn aggregate_copies_caller_selected_proof_and_emits_standard_event() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let hash = test_intent_hash();
    let proof_address = aggregate_address(&hash);
    context.airdrop(&proof_address, 1_000_000).unwrap();
    set_prover_proof(&mut context, &hash, PROVERS[0], DESTINATION, claimant);
    set_prover_proof(
        &mut context,
        &hash,
        PROVERS[1],
        DESTINATION,
        Pubkey::new_unique(),
    );
    let result = context.aggregator_prover().aggregate(hash, PROVERS[0]);
    assert!(
        result.is_ok_and(common::contains_cpi_event(IntentProven::new(
            hash,
            claimant,
            DESTINATION
        )))
    );
    let proof: ProofAccount = context.account(&proof_address).unwrap();
    assert_eq!(proof.0.claimant, claimant);
    assert_eq!(proof.0.destination, DESTINATION);
}

#[test]
fn aggregate_can_select_second_prover_despite_conflicting_first_proof() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let hash = test_intent_hash();
    set_prover_proof(
        &mut context,
        &hash,
        PROVERS[0],
        DESTINATION + 1,
        Pubkey::new_unique(),
    );
    set_prover_proof(&mut context, &hash, PROVERS[1], DESTINATION, claimant);
    assert!(context
        .aggregator_prover()
        .aggregate(hash, PROVERS[1])
        .is_ok());
    let proof: ProofAccount = context.account(&aggregate_address(&hash)).unwrap();
    assert_eq!(proof.0.claimant, claimant);
    assert_eq!(proof.0.destination, DESTINATION);
}

#[test]
fn aggregate_copies_selected_prover_destination_and_claimant() {
    let mut context = initialized();
    let hash = test_intent_hash();
    let claimant = Pubkey::new_unique();
    set_prover_proof(&mut context, &hash, PROVERS[0], DESTINATION + 1, claimant);
    set_prover_proof(
        &mut context,
        &hash,
        PROVERS[1],
        DESTINATION,
        Pubkey::new_unique(),
    );
    let result = context.aggregator_prover().aggregate(hash, PROVERS[0]);
    assert!(
        result.is_ok_and(common::contains_cpi_event(IntentProven::new(
            hash,
            claimant,
            DESTINATION + 1
        )))
    );
    let proof: ProofAccount = context.account(&aggregate_address(&hash)).unwrap();
    assert_eq!(proof.0.destination, DESTINATION + 1);
    assert_eq!(proof.0.claimant, claimant);
}

#[test]
fn aggregate_rejects_unproven_intent() {
    let mut context = initialized();
    let hash = test_intent_hash();
    assert!(context
        .aggregator_prover()
        .aggregate(hash, PROVERS[0])
        .is_err_and(common::is_error(AggregatorProverError::InvalidProof)));
    assert!(context.get_account(&aggregate_address(&hash)).is_none());
}

#[test]
fn aggregate_rejects_zero_claimants_malformed_data_and_wrong_owners() {
    for variant in 0..5 {
        let mut context = initialized();
        let claimant = Pubkey::new_unique();
        let hash = test_intent_hash();
        set_prover_proof(
            &mut context,
            &hash,
            PROVERS[0],
            DESTINATION,
            if variant == 0 {
                Pubkey::default()
            } else {
                claimant
            },
        );
        let address = Proof::pda(&hash, &PROVERS[0]).0;
        let mut account = context.get_account(&address).unwrap();
        match variant {
            0 => (),
            1 => account.data.truncate(9),
            2 => account.owner = Pubkey::new_unique(),
            3 => account.data[0] ^= 1,
            4 => account.data.push(0),
            _ => unreachable!(),
        }
        context.set_account(address, account).unwrap();
        set_prover_proof(&mut context, &hash, PROVERS[1], DESTINATION, claimant);
        assert!(context
            .aggregator_prover()
            .aggregate(hash, PROVERS[0])
            .is_err_and(common::is_error(AggregatorProverError::InvalidProof)));
    }
}

#[test]
fn aggregate_rejects_unconfigured_prover() {
    let mut context = initialized();
    let hash = test_intent_hash();
    set_prover_proof(
        &mut context,
        &hash,
        dummy_ism::ID,
        DESTINATION,
        Pubkey::new_unique(),
    );
    assert!(context
        .aggregator_prover()
        .aggregate(hash, dummy_ism::ID)
        .is_err_and(common::is_error(AggregatorProverError::InvalidProver)));
    assert!(context.get_account(&aggregate_address(&hash)).is_none());
}

#[test]
fn aggregate_rejects_proof_from_another_prover_or_intent() {
    for wrong_prover in [false, true] {
        let mut context = initialized();
        let hash = test_intent_hash();
        let other_hash: Bytes32 = [43; 32].into();
        let prover = if wrong_prover { PROVERS[1] } else { PROVERS[0] };
        let proof_hash = if wrong_prover { hash } else { other_hash };
        set_prover_proof(
            &mut context,
            &proof_hash,
            prover,
            DESTINATION,
            Pubkey::new_unique(),
        );
        let mut instruction = context
            .aggregator_prover()
            .build_aggregate_instruction(hash, PROVERS[0]);
        instruction.accounts[3].pubkey = Proof::pda(&proof_hash, &prover).0;
        assert!(context
            .aggregator_prover()
            .send_instruction(instruction)
            .is_err_and(common::is_error(AggregatorProverError::InvalidProof)));
        assert!(context.get_account(&aggregate_address(&hash)).is_none());
    }
}

#[test]
fn aggregate_rejects_existing_proof_for_identical_and_conflicting_claimants() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    set_prover_proof(
        &mut context,
        &test_intent_hash(),
        PROVERS[0],
        DESTINATION,
        claimant,
    );
    context
        .aggregator_prover()
        .aggregate(test_intent_hash(), PROVERS[0])
        .unwrap();
    context.expire_blockhash();
    assert!(context
        .aggregator_prover()
        .aggregate(test_intent_hash(), PROVERS[0])
        .is_err_and(common::is_error(ErrorCode::ConstraintZero)));
    let other_claimant = Pubkey::new_unique();
    set_prover_proof(
        &mut context,
        &test_intent_hash(),
        PROVERS[0],
        DESTINATION,
        other_claimant,
    );
    context.expire_blockhash();
    assert!(context
        .aggregator_prover()
        .aggregate(test_intent_hash(), PROVERS[0])
        .is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}

#[test]
fn close_proof_rejects_unscoped_signer() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let hash = test_intent_hash();
    let proof_address = aggregate_address(&hash);
    set_prover_proof(&mut context, &hash, PROVERS[0], DESTINATION, claimant);
    context
        .aggregator_prover()
        .aggregate(hash, PROVERS[0])
        .unwrap();
    let instruction = Instruction {
        program_id: aggregator_prover::ID,
        accounts: aggregator_prover::accounts::CloseProof {
            portal_proof_closer: context.payer.pubkey(),
            proof: proof_address,
            payer: context.payer.pubkey(),
        }
        .to_account_metas(None),
        data: aggregator_prover::instruction::CloseProof {}.data(),
    };
    assert!(context
        .aggregator_prover()
        .send_instruction(instruction)
        .is_err_and(common::is_error(
            AggregatorProverError::InvalidPortalProofCloser
        )));
    assert!(context.get_account(&proof_address).is_some());
}

fn funded(context: &mut common::Context) -> (Reward, Bytes32, Bytes32) {
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = aggregator_prover::ID;
    let route_hash: Bytes32 = [42; 32].into();
    let hash = intent_hash(DESTINATION, &route_hash, &reward.hash());
    let vault = vault_pda(&hash).0;
    let funder = context.funder.pubkey();
    context.airdrop(&funder, reward.native_amount).unwrap();
    let token_program = context.token_program;
    reward
        .tokens
        .iter()
        .for_each(|token| context.airdrop_token_ata(&token.token, &funder, token.amount));
    context
        .portal()
        .fund_intent(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            false,
            reward.tokens.iter().flat_map(|token| {
                [
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &funder,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &vault,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new_readonly(token.token, false),
                ]
            }),
        )
        .unwrap();

    (reward, route_hash, hash)
}

#[test]
fn existing_portal_withdraws_native_and_tokens_and_closes_only_aggregate_proof() {
    for token_2022 in [false, true] {
        let (mut context, authority) = setup();
        if token_2022 {
            context.token_program = anchor_spl::token_2022::ID;
        }
        context
            .aggregator_prover()
            .init(&authority, PROVERS.to_vec())
            .unwrap();
        let (reward, route_hash, hash) = funded(&mut context);
        let claimant = Pubkey::new_unique();
        set_prover_proof(&mut context, &hash, PROVERS[1], DESTINATION, claimant);
        context
            .aggregator_prover()
            .aggregate(hash, PROVERS[1])
            .unwrap();
        context.warp_to_timestamp((reward.deadline + 1).try_into().unwrap());
        let vault = vault_pda(&hash).0;
        let proof_address = Proof::pda(&hash, &aggregator_prover::ID).0;
        let marker = WithdrawnMarker::pda(&hash).0;
        let refund = context.portal().refund_intent(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            proof_address,
            marker,
            reward.creator,
            [],
        );
        assert!(refund.is_err_and(common::is_error(
            PortalError::IntentFulfilledAndNotWithdrawn
        )));
        let token_program = context.token_program;
        reward
            .tokens
            .iter()
            .for_each(|token| context.airdrop_token_ata(&token.token, &claimant, 0));
        let token_accounts = reward
            .tokens
            .iter()
            .flat_map(|token| {
                [
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &vault,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &claimant,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new_readonly(token.token, false),
                ]
            })
            .collect::<Vec<_>>();
        let payer = context.payer.pubkey();
        let result = context.portal().withdraw_intent(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            claimant,
            proof_address,
            marker,
            proof_closer_pda(&aggregator_prover::ID).0,
            token_accounts,
            iter::once(AccountMeta::new(payer, true)),
        );
        assert!(result.is_ok_and(common::contains_event(
            portal::events::IntentWithdrawn::new(hash, claimant)
        )));
        assert_eq!(context.balance(&claimant), reward.native_amount);
        reward.tokens.iter().for_each(|token| {
            assert_eq!(
                context.token_balance_ata(&token.token, &claimant),
                token.amount
            )
        });
        assert!(context.get_account(&proof_address).is_none());
        assert!(context
            .get_account(&Proof::pda(&hash, &PROVERS[1]).0)
            .is_some());
        assert!(context.get_account(&marker).is_some());
    }
}

#[test]
fn max_provers_can_select_last_prover() {
    let (mut context, authority) = setup();
    let provers: Vec<_> = (0..MAX_PROVERS).map(|_| Pubkey::new_unique()).collect();
    provers.iter().for_each(|prover| {
        context
            .add_program(*prover, include_bytes!("../../target/deploy/dummy_ism.so"))
            .unwrap();
    });
    context
        .aggregator_prover()
        .init(&authority, provers.clone())
        .unwrap();
    let claimant = Pubkey::new_unique();
    let hash = test_intent_hash();
    set_prover_proof(
        &mut context,
        &hash,
        provers[MAX_PROVERS - 1],
        DESTINATION,
        claimant,
    );
    let result = context
        .aggregator_prover()
        .aggregate(hash, provers[MAX_PROVERS - 1])
        .unwrap();
    assert!(result.compute_units_consumed < 100_000);
    println!(
        "eight-prover aggregation: {} CU",
        result.compute_units_consumed
    );
}

#[test]
fn aggregate_rejects_substituted_aggregate_pda() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let hash = test_intent_hash();
    set_prover_proof(&mut context, &hash, PROVERS[0], DESTINATION, claimant);
    let mut instruction = context
        .aggregator_prover()
        .build_aggregate_instruction(hash, PROVERS[0]);
    instruction.accounts[4].pubkey = Pubkey::new_unique();
    assert!(context
        .aggregator_prover()
        .send_instruction(instruction)
        .is_err_and(common::is_error(AggregatorProverError::InvalidProof)));
}

#[test]
fn different_prover_cannot_overwrite_recorded_claimant() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let original = test_intent_hash();
    let address = aggregate_address(&original);
    set_prover_proof(&mut context, &original, PROVERS[1], DESTINATION, claimant);
    context
        .aggregator_prover()
        .aggregate(original, PROVERS[1])
        .unwrap();
    let other_claimant = Pubkey::new_unique();
    let later = test_intent_hash();
    set_prover_proof(
        &mut context,
        &later,
        PROVERS[0],
        DESTINATION,
        other_claimant,
    );
    context.expire_blockhash();
    assert!(context
        .aggregator_prover()
        .aggregate(later, PROVERS[0])
        .is_err_and(common::is_error(ErrorCode::ConstraintZero)));
    assert_eq!(
        context
            .account::<ProofAccount>(&address)
            .unwrap()
            .0
            .claimant,
        claimant
    );
}

#[test]
fn existing_portal_refunds_unaggregated_intent_after_deadline() {
    let mut context = initialized();
    let (reward, route_hash, hash) = funded(&mut context);
    let vault = vault_pda(&hash).0;
    // This pins the integration boundary: prover delivery alone is not settlement readiness.
    context.set_proof(
        Proof::pda(&hash, &PROVERS[0]).0,
        Proof::new(DESTINATION, Pubkey::new_unique()),
        PROVERS[0],
    );
    context.warp_to_timestamp((reward.deadline + 1).try_into().unwrap());
    let token_program = context.token_program;
    reward
        .tokens
        .iter()
        .for_each(|token| context.airdrop_token_ata(&token.token, &reward.creator, 0));
    let token_accounts = reward
        .tokens
        .iter()
        .flat_map(|token| {
            [
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &reward.creator,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect::<Vec<_>>();
    assert!(context
        .portal()
        .refund_intent(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            Proof::pda(&hash, &aggregator_prover::ID).0,
            WithdrawnMarker::pda(&hash).0,
            reward.creator,
            token_accounts
        )
        .is_ok());
    assert_eq!(context.balance(&reward.creator), reward.native_amount);
}

#[test]
fn unsupported_prove_rejects_direct_calls_without_recording_proofs() {
    let mut context = initialized();
    let hash: Bytes32 = [91; 32].into();
    let instruction = Instruction {
        program_id: aggregator_prover::ID,
        accounts: vec![],
        data: PROVE_DISCRIMINATOR
            .into_iter()
            .chain(
                anchor_lang::prelude::borsh::to_vec(&ProveArgs::new(
                    10,
                    ProofData::new(
                        CHAIN_ID,
                        vec![IntentHashClaimant::new(hash, [92; 32].into())],
                    ),
                    vec![],
                ))
                .unwrap(),
            )
            .collect(),
    };
    assert!(context
        .aggregator_prover()
        .send_instruction(instruction)
        .is_err_and(common::is_error(ErrorCode::InstructionFallbackNotFound)));
    assert!(context
        .get_account(&Proof::pda(&hash, &aggregator_prover::ID).0)
        .is_none());
}

#[test]
fn unsupported_prove_rejects_portal_dispatch_without_sending_or_recording_proofs() {
    let mut context = initialized();
    let intent = context
        .fulfill_rand_intents(1, aggregator_prover::ID)
        .remove(0);
    let outbox = common::hyperlane_context::outbox_pda();
    let before = context.get_account(&outbox).unwrap();
    let result = context.portal().prove_intent_via_program(
        aggregator_prover::ID,
        vec![intent.intent_hash],
        10,
        vec![portal::state::FulfillMarker::pda(&intent.intent_hash).0],
        portal::state::dispatcher_pda(&aggregator_prover::ID).0,
        hyper_prover::ID.to_bytes().to_vec(),
        vec![],
    );
    assert!(result.is_err_and(common::is_error(ErrorCode::InstructionFallbackNotFound)));
    assert_eq!(context.get_account(&outbox).unwrap(), before);
    assert!(context
        .get_account(&Proof::pda(&intent.intent_hash, &aggregator_prover::ID).0)
        .is_none());
}

#[test]
fn hyper_prover_relays_then_source_aggregates_delivered_proof() {
    let mut destination = common::Context::default();
    let intent = destination
        .fulfill_rand_intents(1, aggregator_prover::ID)
        .remove(0);
    let marker = portal::state::FulfillMarker::pda(&intent.intent_hash).0;
    let claimant = destination
        .account::<portal::state::FulfillMarker>(&marker)
        .unwrap()
        .claimant;
    let transaction = destination
        .portal()
        .prove_intent_via_hyper_prover(
            vec![intent.intent_hash],
            CHAIN_ID,
            vec![marker],
            portal::state::dispatcher_pda(&hyper_prover::ID).0,
            hyper_prover::state::dispatcher_pda().0,
            hyper_prover::hyperlane::MAILBOX_ID,
            hyper_prover::ID.to_bytes().to_vec(),
        )
        .unwrap();
    let dispatch = transaction
        .inner_instructions
        .into_iter()
        .flatten()
        .find_map(|instruction| {
            match MailboxInstruction::try_from_slice(&instruction.instruction.data).ok()? {
                MailboxInstruction::OutboxDispatch(dispatch) => Some(dispatch),
                _ => None,
            }
        })
        .expect("HyperProver must send an OutboxDispatch");
    assert_eq!(dispatch.recipient, hyper_prover::ID.to_bytes());
    assert_eq!(dispatch.sender, hyper_prover::state::dispatcher_pda().0);
    let proof_data = ProofData::from_bytes(&dispatch.message_body).unwrap();
    assert_eq!(proof_data.destination, CHAIN_ID);
    assert_eq!(
        proof_data.intent_hashes_claimants,
        vec![IntentHashClaimant::new(intent.intent_hash, claimant)]
    );

    let mut source = initialized();
    source
        .hyper_prover()
        .init(
            vec![dispatch.sender.to_bytes().into()],
            hyper_prover::state::Config::pda().0,
        )
        .unwrap();
    source
        .airdrop(
            &hyper_prover::state::pda_payer_pda().0,
            common::sol_amount(1.0),
        )
        .unwrap();
    let origin: u32 = CHAIN_ID.try_into().unwrap();
    let message: Vec<_> = [
        vec![3],
        0u32.to_be_bytes().to_vec(),
        origin.to_be_bytes().to_vec(),
        dispatch.sender.to_bytes().to_vec(),
        dispatch.destination_domain.to_be_bytes().to_vec(),
        dispatch.recipient.to_vec(),
        dispatch.message_body.clone(),
    ]
    .concat();
    let handle_accounts = source.hyper_prover().handle_account_metas(
        origin,
        dispatch.sender.to_bytes(),
        dispatch.message_body,
    );
    source
        .hyperlane()
        .inbox_process(message, handle_accounts)
        .unwrap();
    let aggregate_address = Proof::pda(&intent.intent_hash, &aggregator_prover::ID).0;
    assert!(source.get_account(&aggregate_address).is_none());
    let result = source.aggregator_prover().aggregate(
        proof_data.intent_hashes_claimants[0].intent_hash,
        PROVERS[0],
    );
    assert!(
        result.is_ok_and(common::contains_cpi_event(IntentProven::new(
            intent.intent_hash,
            Pubkey::new_from_array(claimant.into()),
            CHAIN_ID
        )))
    );
    let proof: ProofAccount = source.account(&aggregate_address).unwrap();
    assert!(claimant == proof.0.claimant);
    assert_eq!(proof.0.destination, CHAIN_ID);
}
