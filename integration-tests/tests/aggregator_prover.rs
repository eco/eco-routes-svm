use aggregator_prover::instructions::AggregatorProverError;
use aggregator_prover::state::{Config, MAX_PROVERS};
use anchor_lang::error::ErrorCode;
use anchor_lang::InstructionData;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::{
    IntentHashClaimant, IntentProven, Proof, ProofData, ProveArgs, ValidateProofArgs,
    PROVE_DISCRIMINATOR,
};
use eco_svm_std::{Bytes32, CHAIN_ID};
use polymer_prover::event::evm_address_to_bytes32;
use portal::instructions::PortalError;
use portal::state::{vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::polymer_prover_context::intent_fulfilled_result;

pub mod common;

const PROVERS: [Pubkey; 3] = [hyper_prover::ID, local_prover::ID, polymer_prover::ID];
const BRIDGE_PROVERS: [Pubkey; 2] = [hyper_prover::ID, polymer_prover::ID];
const EMITTER: [u8; 20] = [0xec; 20];
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

fn stage_polymer_result(
    context: &mut common::Context,
    result: &mock_polymer_prover::ValidationResultAccount,
) -> Keypair {
    context
        .polymer_prover()
        .init(
            vec![evm_address_to_bytes32(EMITTER)],
            polymer_prover::state::Config::pda().0,
        )
        .unwrap();
    let authority = Keypair::new();
    context
        .polymer_prover()
        .polymer_create_accounts(&authority)
        .unwrap();
    context
        .polymer_prover()
        .polymer_load_result(&authority, result)
        .unwrap();

    authority
}

fn deliver_bridge_proof(
    context: &mut common::Context,
    prover: Pubkey,
    proof_data: ProofData,
) -> common::TransactionResult {
    if prover == polymer_prover::ID {
        let proof_accounts = proof_data
            .intent_hashes_claimants
            .iter()
            .map(|intent| AccountMeta::new(Proof::pda(&intent.intent_hash, &prover).0, false))
            .collect();
        let result = intent_fulfilled_result(
            EMITTER,
            CHAIN_ID,
            proof_data.destination.try_into().unwrap(),
            proof_data.to_bytes(),
        );
        let authority = stage_polymer_result(context, &result);

        return context
            .polymer_prover()
            .validate(&authority, proof_accounts);
    }
    assert_eq!(prover, hyper_prover::ID);
    let sender = [55; 32];
    let origin: u32 = proof_data.destination.try_into().unwrap();
    context
        .hyper_prover()
        .init(vec![sender.into()], hyper_prover::state::Config::pda().0)
        .unwrap();
    context
        .airdrop(
            &hyper_prover::state::pda_payer_pda().0,
            common::sol_amount(1.0),
        )
        .unwrap();
    let payload = proof_data.to_bytes();
    let message = [
        vec![3],
        0u32.to_be_bytes().to_vec(),
        origin.to_be_bytes().to_vec(),
        sender.to_vec(),
        u32::try_from(CHAIN_ID).unwrap().to_be_bytes().to_vec(),
        hyper_prover::ID.to_bytes().to_vec(),
        payload.clone(),
    ]
    .concat();
    let accounts = context
        .hyper_prover()
        .handle_account_metas(origin, sender, payload);

    context.hyperlane().inbox_process(message, accounts)
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
fn bridge_delivery_validates_and_withdraws_without_aggregation() {
    for (prover, token_2022) in BRIDGE_PROVERS
        .into_iter()
        .flat_map(|prover| [false, true].map(|token_2022| (prover, token_2022)))
    {
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
        let proof_data = ProofData::new(
            DESTINATION,
            vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
        );
        assert!(
            deliver_bridge_proof(&mut context, prover, proof_data).is_ok_and(
                common::contains_cpi_event(IntentProven::new(hash, claimant, DESTINATION))
            )
        );
        let underlying_address = Proof::pda(&hash, &prover).0;
        let underlying_before = context.get_account(&underlying_address).unwrap();
        assert!(context
            .get_account(&Proof::pda(&hash, &aggregator_prover::ID).0)
            .is_none());
        context.warp_to_timestamp((reward.deadline + 1).try_into().unwrap());
        let vault = vault_pda(&hash).0;
        let proof_address = Proof::pda(&hash, &aggregator_prover::ID).0;
        let marker = WithdrawnMarker::pda(&hash).0;
        let refund = context.portal().refund_intent_with_accounts(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            Config::pda().0,
            marker,
            reward.creator,
            portal::instructions::RefundKind::Expired,
            Some(reward.prover),
            [],
            PROVERS
                .into_iter()
                .flat_map(|prover| {
                    [
                        AccountMeta::new_readonly(prover, false),
                        AccountMeta::new_readonly(Proof::pda(&hash, &prover).0, false),
                    ]
                })
                .collect(),
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
        let result = context.portal().withdraw_intent(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            claimant,
            Config::pda().0,
            marker,
            token_accounts,
            [
                AccountMeta::new_readonly(prover, false),
                AccountMeta::new_readonly(underlying_address, false),
            ],
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
        assert_eq!(
            context.get_account(&underlying_address).unwrap(),
            underlying_before
        );
        assert!(context.get_account(&marker).is_some());
        let cleanup = context.aggregator_prover().cleanup_accounts(&hash, prover);
        context
            .portal()
            .close_proof(DESTINATION, route_hash, reward, cleanup)
            .unwrap();
        assert!(context.get_account(&underlying_address).is_none());
    }
}

#[test]
fn selected_member_validation_returns_boolean_without_creating_proof() {
    let mut context = initialized();
    let hash = test_intent_hash();
    let claimant = Pubkey::new_unique();
    set_prover_proof(&mut context, &hash, local_prover::ID, DESTINATION, claimant);
    for (prover, expected) in [
        (hyper_prover::ID, false),
        (local_prover::ID, true),
        (polymer_prover::ID, false),
    ] {
        let result = context
            .aggregator_prover()
            .validate_proof(
                ValidateProofArgs::new(hash, DESTINATION, Some(claimant)),
                &[prover],
            )
            .unwrap();
        assert_eq!(result.return_data.program_id, aggregator_prover::ID);
        assert_eq!(result.return_data.data, vec![u8::from(expected)]);
        assert!(context
            .get_account(&Proof::pda(&hash, &aggregator_prover::ID).0)
            .is_none());
    }
}

#[test]
fn wildcard_checks_all_members_and_rejects_incomplete_or_reordered_sets() {
    for selected in PROVERS {
        let mut context = initialized();
        let hash = test_intent_hash();
        let args = ValidateProofArgs::new(hash, DESTINATION, None);
        assert_eq!(
            context
                .aggregator_prover()
                .validate_proof(args.clone(), &PROVERS)
                .unwrap()
                .return_data
                .data,
            vec![0]
        );
        set_prover_proof(
            &mut context,
            &hash,
            selected,
            DESTINATION,
            eco_svm_std::claimant::cancelled(),
        );
        assert_eq!(
            context
                .aggregator_prover()
                .validate_proof(args.clone(), &PROVERS)
                .unwrap()
                .return_data
                .data,
            vec![1]
        );
        for provers in [
            vec![selected],
            vec![PROVERS[1], PROVERS[0], PROVERS[2]],
            vec![PROVERS[0]; 3],
        ] {
            assert!(context
                .aggregator_prover()
                .validate_proof(args.clone(), &provers)
                .is_err_and(common::is_error(AggregatorProverError::InvalidProverSet)));
        }
    }
}

#[test]
fn selected_query_rejects_unconfigured_member_and_substituted_proof() {
    let mut context = initialized();
    let args = ValidateProofArgs::new(test_intent_hash(), DESTINATION, Some(Pubkey::new_unique()));
    assert!(context
        .aggregator_prover()
        .validate_proof(args.clone(), &[malicious_proof_closer::ID])
        .is_err_and(common::is_error(AggregatorProverError::InvalidProver)));
    let result = context.aggregator_prover().send_instruction(Instruction {
        program_id: aggregator_prover::ID,
        accounts: vec![
            AccountMeta::new_readonly(Config::pda().0, false),
            AccountMeta::new_readonly(local_prover::ID, false),
            AccountMeta::new_readonly(Pubkey::new_unique(), false),
        ],
        data: aggregator_prover::instruction::ValidateProof { args }.data(),
    });
    assert!(result.is_err_and(common::is_program_error(
        local_prover::ID,
        eco_svm_std::prover::ProverError::InvalidProof
    )));
}

#[test]
fn every_member_and_late_proofs_can_be_cleaned_after_withdrawal() {
    let mut context = initialized();
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = aggregator_prover::ID;
    reward.tokens.clear();
    reward.native_amount = 0;
    let route_hash = test_intent_hash();
    let hash = intent_hash(DESTINATION, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    for prover in PROVERS {
        set_prover_proof(&mut context, &hash, prover, DESTINATION, claimant);
    }
    context
        .portal()
        .withdraw_intent(
            DESTINATION,
            reward.clone(),
            vault_pda(&hash).0,
            route_hash,
            claimant,
            Config::pda().0,
            WithdrawnMarker::pda(&hash).0,
            [],
            [
                AccountMeta::new_readonly(local_prover::ID, false),
                AccountMeta::new_readonly(Proof::pda(&hash, &local_prover::ID).0, false),
            ],
        )
        .unwrap();
    for prover in PROVERS.into_iter().chain([local_prover::ID]) {
        if context.get_account(&Proof::pda(&hash, &prover).0).is_none() {
            set_prover_proof(&mut context, &hash, prover, DESTINATION, claimant);
        }
        let accounts = context.aggregator_prover().cleanup_accounts(&hash, prover);
        context
            .portal()
            .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
            .unwrap();
        assert!(context.get_account(&Proof::pda(&hash, &prover).0).is_none());
        assert!(context
            .get_account(&WithdrawnMarker::pda(&hash).0)
            .is_some());
    }
}

#[test]
fn cancellation_cleanup_waits_for_deadline_and_does_not_require_refund() {
    let mut context = initialized();
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = aggregator_prover::ID;
    reward.tokens.clear();
    let route_hash = test_intent_hash();
    let hash = intent_hash(DESTINATION, &route_hash, &reward.hash());
    for prover in PROVERS {
        set_prover_proof(
            &mut context,
            &hash,
            prover,
            DESTINATION,
            eco_svm_std::claimant::cancelled(),
        );
    }
    let accounts = context
        .aggregator_prover()
        .cleanup_accounts(&hash, local_prover::ID);
    context.warp_to_timestamp((reward.deadline - 1).try_into().unwrap());
    assert!(context
        .portal()
        .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
        .is_err_and(common::is_error(PortalError::RewardNotExpired)));
    context.warp_to_timestamp(reward.deadline.try_into().unwrap());
    for prover in PROVERS {
        let accounts = context.aggregator_prover().cleanup_accounts(&hash, prover);
        context
            .portal()
            .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
            .unwrap();
        assert!(context.get_account(&Proof::pda(&hash, &prover).0).is_none());
    }
    assert!(context
        .get_account(&WithdrawnMarker::pda(&hash).0)
        .is_none());
    assert_eq!(
        context
            .aggregator_prover()
            .validate_proof(ValidateProofArgs::new(hash, DESTINATION, None), &PROVERS)
            .unwrap()
            .return_data
            .data,
        vec![0]
    );
    set_prover_proof(
        &mut context,
        &hash,
        local_prover::ID,
        DESTINATION,
        Pubkey::new_unique(),
    );
    let accounts = context
        .aggregator_prover()
        .cleanup_accounts(&hash, local_prover::ID);
    assert!(context
        .portal()
        .close_proof(DESTINATION, route_hash, reward, accounts)
        .is_err_and(common::is_error(PortalError::IntentNotCancelled)));
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
fn eight_member_wildcard_checks_last_member_within_transaction_limits() {
    let (mut context, authority) = setup();
    let provers = (0..MAX_PROVERS)
        .map(|_| Pubkey::new_unique())
        .collect::<Vec<_>>();
    let binary = include_bytes!("../../target/deploy/mock_prover.so");
    provers
        .iter()
        .for_each(|prover| context.add_program(*prover, binary).unwrap());
    context
        .aggregator_prover()
        .init(&authority, provers.clone())
        .unwrap();
    let hash = test_intent_hash();
    let args = ValidateProofArgs::new(hash, DESTINATION, None);
    let absent = context
        .aggregator_prover()
        .validate_proof(args.clone(), &provers)
        .unwrap();
    assert_eq!(absent.return_data.data, vec![0]);
    set_prover_proof(
        &mut context,
        &hash,
        provers[MAX_PROVERS - 1],
        DESTINATION,
        Pubkey::new_unique(),
    );
    let present = context
        .aggregator_prover()
        .validate_proof(args, &provers)
        .unwrap();
    assert_eq!(present.return_data.data, vec![1]);
    assert!(absent.compute_units_consumed < 200_000);
    assert!(present.compute_units_consumed < 200_000);
    eprintln!(
        "eight-member wildcard CU: absent={}, last-present={}",
        absent.compute_units_consumed, present.compute_units_consumed
    );
}
