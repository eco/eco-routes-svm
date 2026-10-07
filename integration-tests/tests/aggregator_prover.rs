use aggregator_prover::instructions::AggregatorProverError;
use aggregator_prover::state::{Config, MAX_PROVERS};
use anchor_lang::error::ErrorCode;
use anchor_lang::prelude::borsh;
use anchor_lang::{AnchorDeserialize, InstructionData};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::{
    GetProofArgs, IntentHashClaimant, IntentProven, Proof, ProofData, ProveArgs,
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
        let query = common::aggregator_query(&hash, &PROVERS);
        let refund = context.portal().refund_intent_with_accounts(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            Config::pda().0,
            marker,
            reward.creator,
            Some(reward.prover),
            [],
            query,
        );
        assert!(refund.is_err_and(common::is_error(
            PortalError::IntentFulfilledAndNotWithdrawn
        )));
        let others = PROVERS
            .into_iter()
            .filter(|member| *member != prover)
            .collect::<Vec<_>>();
        let query = common::aggregator_query(&hash, &others);
        let refund = context.portal().refund_intent_with_accounts(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            Config::pda().0,
            marker,
            reward.creator,
            Some(reward.prover),
            [],
            query,
        );
        assert!(refund.is_err_and(common::is_program_error(
            aggregator_prover::ID,
            AggregatorProverError::IncompleteProverSet
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
        let query = common::aggregator_query(&hash, &[prover]);
        let result = context.portal().withdraw_intent(
            DESTINATION,
            reward.clone(),
            vault,
            route_hash,
            claimant,
            Config::pda().0,
            marker,
            token_accounts,
            query,
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
        let cleanup = context
            .aggregator_prover()
            .cleanup_accounts(&hash, &[prover]);
        context
            .portal()
            .close_proof(DESTINATION, route_hash, reward, cleanup)
            .unwrap();
        assert!(context.get_account(&underlying_address).is_none());
    }
}

#[test]
fn returns_first_proof_in_query_order_without_creating_an_aggregate_proof() {
    let mut context = initialized();
    let hash = test_intent_hash();
    let args = GetProofArgs::new(hash, DESTINATION, vec![]);
    for (index, prover) in PROVERS.into_iter().enumerate() {
        let claimant = Pubkey::new_from_array([index as u8 + 1; 32]);
        set_prover_proof(&mut context, &hash, prover, DESTINATION, claimant);
    }
    for (index, selected) in PROVERS.into_iter().enumerate() {
        let provers = std::iter::once(selected)
            .chain(PROVERS.into_iter().filter(|prover| *prover != selected))
            .collect::<Vec<_>>();
        let result = context
            .aggregator_prover()
            .get_proof(args.clone(), &provers)
            .unwrap();
        assert_eq!(result.return_data.program_id, aggregator_prover::ID);
        assert_eq!(
            Option::<Proof>::try_from_slice(&result.return_data.data).unwrap(),
            Some(Proof::new(
                DESTINATION,
                Pubkey::new_from_array([index as u8 + 1; 32])
            ))
        );
        assert!(context
            .get_account(&Proof::pda(&hash, &aggregator_prover::ID).0)
            .is_none());
    }
}

#[test]
fn any_member_subset_returns_a_proof_but_absence_requires_all_members() {
    for selected in PROVERS {
        let mut context = initialized();
        let hash = test_intent_hash();
        let args = GetProofArgs::new(hash, DESTINATION, vec![]);
        let others = PROVERS
            .into_iter()
            .filter(|prover| *prover != selected)
            .collect::<Vec<_>>();
        assert_eq!(
            context
                .aggregator_prover()
                .get_proof(args.clone(), &PROVERS)
                .unwrap()
                .return_data
                .data,
            vec![0]
        );
        for provers in [vec![], vec![selected], others.clone()] {
            assert!(context
                .aggregator_prover()
                .get_proof(args.clone(), &provers)
                .is_err_and(common::is_error(AggregatorProverError::IncompleteProverSet)));
        }
        set_prover_proof(
            &mut context,
            &hash,
            selected,
            DESTINATION,
            eco_svm_std::claimant::cancelled(),
        );
        let expected = borsh::to_vec(&Some(Proof::new(
            DESTINATION,
            eco_svm_std::claimant::cancelled(),
        )))
        .unwrap();
        for provers in [PROVERS.to_vec(), vec![selected], vec![others[0], selected]] {
            assert_eq!(
                context
                    .aggregator_prover()
                    .get_proof(args.clone(), &provers)
                    .unwrap()
                    .return_data
                    .data,
                expected
            );
        }
        assert!(context
            .aggregator_prover()
            .get_proof(args.clone(), &others)
            .is_err_and(common::is_error(AggregatorProverError::IncompleteProverSet)));
        for provers in [vec![selected, selected], vec![PROVERS[0]; 3]] {
            assert!(context
                .aggregator_prover()
                .get_proof(args.clone(), &provers)
                .is_err_and(common::is_error(AggregatorProverError::InvalidProverSet)));
        }
    }
}

#[test]
fn query_rejects_unconfigured_member_and_substituted_proof() {
    let mut context = initialized();
    let args = GetProofArgs::new(test_intent_hash(), DESTINATION, vec![]);
    assert!(context
        .aggregator_prover()
        .get_proof(
            args.clone(),
            &[malicious_proof_closer::ID, PROVERS[1], PROVERS[2]]
        )
        .is_err_and(common::is_error(AggregatorProverError::InvalidProver)));
    let query = common::aggregator_query(
        &args.intent_hash,
        &[local_prover::ID, hyper_prover::ID, polymer_prover::ID],
    );
    let mut accounts = std::iter::once(AccountMeta::new_readonly(Config::pda().0, false))
        .chain(query.accounts)
        .collect::<Vec<_>>();
    accounts[2].pubkey = Pubkey::new_unique();
    let result = context.aggregator_prover().send_instruction(Instruction {
        program_id: aggregator_prover::ID,
        accounts,
        data: aggregator_prover::instruction::GetProof {
            args: GetProofArgs {
                data: query.data,
                ..args
            },
        }
        .data(),
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
    for prover in [hyper_prover::ID, polymer_prover::ID] {
        set_prover_proof(&mut context, &hash, prover, DESTINATION, claimant);
    }
    let query = common::aggregator_query(&hash, &PROVERS);
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
            query,
        )
        .unwrap();
    let accounts = context.aggregator_prover().cleanup_accounts(&hash, &[]);
    assert!(context
        .portal()
        .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
        .is_err_and(common::is_program_error(
            aggregator_prover::ID,
            AggregatorProverError::InvalidProverSet
        )));
    let accounts = context
        .aggregator_prover()
        .cleanup_accounts(&hash, &[hyper_prover::ID, local_prover::ID]);
    // a member without a proof fails the whole cleanup
    assert!(context
        .portal()
        .close_proof(DESTINATION, route_hash, reward.clone(), accounts.clone())
        .is_err_and(common::is_program_error(
            local_prover::ID,
            ErrorCode::AccountNotInitialized
        )));
    assert!(context
        .get_account(&Proof::pda(&hash, &hyper_prover::ID).0)
        .is_some());
    set_prover_proof(&mut context, &hash, local_prover::ID, DESTINATION, claimant);
    context
        .portal()
        .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
        .unwrap();
    for prover in [polymer_prover::ID, local_prover::ID] {
        if context.get_account(&Proof::pda(&hash, &prover).0).is_none() {
            set_prover_proof(&mut context, &hash, prover, DESTINATION, claimant);
        }
        let accounts = context
            .aggregator_prover()
            .cleanup_accounts(&hash, &[prover]);
        context
            .portal()
            .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
            .unwrap();
    }
    for prover in PROVERS {
        assert!(context.get_account(&Proof::pda(&hash, &prover).0).is_none());
    }
    assert!(context
        .get_account(&WithdrawnMarker::pda(&hash).0)
        .is_some());
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
        .cleanup_accounts(&hash, &[local_prover::ID]);
    context.warp_to_timestamp((reward.deadline - 1).try_into().unwrap());
    assert!(context
        .portal()
        .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
        .is_err_and(common::is_error(PortalError::RewardNotExpired)));
    context.warp_to_timestamp(reward.deadline.try_into().unwrap());
    for members in [
        vec![hyper_prover::ID],
        vec![local_prover::ID, polymer_prover::ID],
    ] {
        let accounts = context
            .aggregator_prover()
            .cleanup_accounts(&hash, &members);
        context
            .portal()
            .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
            .unwrap();
        for prover in members {
            assert!(context.get_account(&Proof::pda(&hash, &prover).0).is_none());
        }
    }
    assert!(context
        .get_account(&WithdrawnMarker::pda(&hash).0)
        .is_none());
    assert_eq!(
        context
            .aggregator_prover()
            .get_proof(GetProofArgs::new(hash, DESTINATION, vec![]), &PROVERS)
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
        .cleanup_accounts(&hash, &[local_prover::ID]);
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
fn eight_member_query_checks_last_member_within_transaction_limits() {
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
    let args = GetProofArgs::new(hash, DESTINATION, vec![]);
    let absent = context
        .aggregator_prover()
        .get_proof(args.clone(), &provers)
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
        .get_proof(args, &provers)
        .unwrap();
    assert!(Option::<Proof>::try_from_slice(&present.return_data.data)
        .unwrap()
        .is_some());
    assert!(absent.compute_units_consumed < 200_000);
    assert!(present.compute_units_consumed < 200_000);
    eprintln!(
        "eight-member query CU: absent={}, last-present={}",
        absent.compute_units_consumed, present.compute_units_consumed
    );
}

#[test]
fn member_queries_forward_variable_account_lists_and_data() {
    let (mut context, authority) = setup();
    let provers = [Pubkey::new_unique(), Pubkey::new_unique()];
    let binary = include_bytes!("../../target/deploy/mock_prover.so");
    provers
        .iter()
        .for_each(|prover| context.add_program(*prover, binary).unwrap());
    context
        .aggregator_prover()
        .init(&authority, provers.to_vec())
        .unwrap();
    let hash = test_intent_hash();
    let claimant = Pubkey::new_unique();
    let accounts = vec![
        AccountMeta::new_readonly(Config::pda().0, false),
        AccountMeta::new_readonly(provers[0], false),
        AccountMeta::new_readonly(Proof::pda(&hash, &provers[0]).0, false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(provers[1], false),
        AccountMeta::new_readonly(Proof::pda(&hash, &provers[1]).0, false),
    ];
    let queries = vec![
        aggregator_prover::instructions::MemberQuery {
            account_count: 3,
            data: vec![3],
        },
        aggregator_prover::instructions::MemberQuery {
            account_count: 1,
            data: vec![],
        },
    ];
    let instruction = Instruction {
        program_id: aggregator_prover::ID,
        accounts: accounts.clone(),
        data: aggregator_prover::instruction::GetProof {
            args: GetProofArgs::new(hash, DESTINATION, borsh::to_vec(&queries).unwrap()),
        }
        .data(),
    };
    let absent = context.send_instruction(instruction.clone()).unwrap();
    assert_eq!(absent.return_data.data, vec![0]);
    set_prover_proof(&mut context, &hash, provers[1], DESTINATION, claimant);
    let present = context.send_instruction(instruction.clone()).unwrap();
    assert_eq!(
        Option::<Proof>::try_from_slice(&present.return_data.data).unwrap(),
        Some(Proof::new(DESTINATION, claimant))
    );

    // Wrong child data must fail even though the framing itself is complete.
    let mut wrong_data = queries.clone();
    wrong_data[0].data = vec![2];
    let result = context.send_instruction(Instruction {
        data: aggregator_prover::instruction::GetProof {
            args: GetProofArgs::new(hash, DESTINATION, borsh::to_vec(&wrong_data).unwrap()),
        }
        .data(),
        ..instruction.clone()
    });
    assert!(result.is_err());

    // Validate the entire framing before returning an early member's proof.
    set_prover_proof(&mut context, &hash, provers[0], DESTINATION, claimant);
    for count in [0, 2, 4, u8::MAX] {
        let mut malformed = queries.clone();
        malformed[0].account_count = count;
        assert!(context
            .send_instruction(Instruction {
                data: aggregator_prover::instruction::GetProof {
                    args: GetProofArgs::new(hash, DESTINATION, borsh::to_vec(&malformed).unwrap()),
                }
                .data(),
                ..instruction.clone()
            })
            .is_err());
    }
    for extra in [false, true] {
        let mut malformed = accounts.clone();
        if extra {
            malformed.push(AccountMeta::new_readonly(Pubkey::new_unique(), false));
        } else {
            malformed.pop();
        }
        assert!(context
            .send_instruction(Instruction {
                accounts: malformed,
                ..instruction.clone()
            })
            .is_err_and(common::is_error(AggregatorProverError::InvalidProverSet)));
    }
}

#[test]
fn cancellation_cleanup_follows_the_first_returned_proof_and_closes_every_member() {
    let mut context = initialized();
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = aggregator_prover::ID;
    reward.tokens.clear();
    let route_hash = test_intent_hash();
    let hash = intent_hash(DESTINATION, &route_hash, &reward.hash());
    set_prover_proof(
        &mut context,
        &hash,
        local_prover::ID,
        DESTINATION,
        Pubkey::new_unique(),
    );
    set_prover_proof(
        &mut context,
        &hash,
        hyper_prover::ID,
        DESTINATION,
        eco_svm_std::claimant::cancelled(),
    );
    context.warp_to_timestamp(reward.deadline.try_into().unwrap());
    for members in [
        vec![local_prover::ID],
        vec![local_prover::ID, hyper_prover::ID],
    ] {
        let accounts = context
            .aggregator_prover()
            .cleanup_accounts(&hash, &members);
        assert!(context
            .portal()
            .close_proof(DESTINATION, route_hash, reward.clone(), accounts)
            .is_err_and(common::is_error(PortalError::IntentNotCancelled)));
        for prover in members {
            assert!(context.get_account(&Proof::pda(&hash, &prover).0).is_some());
        }
    }
    let accounts = context
        .aggregator_prover()
        .cleanup_accounts(&hash, &[hyper_prover::ID, local_prover::ID]);
    context
        .portal()
        .close_proof(DESTINATION, route_hash, reward, accounts)
        .unwrap();
    for prover in [hyper_prover::ID, local_prover::ID] {
        assert!(context.get_account(&Proof::pda(&hash, &prover).0).is_none());
    }
}

#[test]
fn refund_uses_returned_cancellation_before_or_after_deadline() {
    for (selected, expired) in PROVERS
        .into_iter()
        .flat_map(|prover| [false, true].map(|expired| (prover, expired)))
    {
        let mut context = initialized();
        let (_, _, mut reward) = context.rand_intent();
        reward.prover = aggregator_prover::ID;
        reward.tokens.clear();
        let route_hash = test_intent_hash();
        let hash = intent_hash(DESTINATION, &route_hash, &reward.hash());
        let vault = vault_pda(&hash).0;
        context.airdrop(&vault, reward.native_amount).unwrap();
        set_prover_proof(
            &mut context,
            &hash,
            selected,
            DESTINATION,
            eco_svm_std::claimant::cancelled(),
        );
        if expired {
            context.warp_to_timestamp(reward.deadline.try_into().unwrap());
        }
        let query = common::aggregator_query(&hash, &PROVERS);
        let balance = context.balance(&reward.creator);
        context
            .portal()
            .refund_intent_with_accounts(
                DESTINATION,
                reward.clone(),
                vault,
                route_hash,
                Config::pda().0,
                WithdrawnMarker::pda(&hash).0,
                reward.creator,
                Some(reward.prover),
                [],
                query,
            )
            .unwrap();
        assert_eq!(
            context.balance(&reward.creator),
            balance + reward.native_amount
        );
        assert!(context
            .get_account(&Proof::pda(&hash, &selected).0)
            .is_some());
        assert!(context
            .get_account(&WithdrawnMarker::pda(&hash).0)
            .is_none());
    }
}

#[test]
fn refund_skips_mismatched_proofs_without_hiding_another_members_proof() {
    for claimant in [Pubkey::new_unique(), eco_svm_std::claimant::cancelled()] {
        let mut context = initialized();
        let (_, _, mut reward) = context.rand_intent();
        reward.prover = aggregator_prover::ID;
        reward.tokens.clear();
        let route_hash = test_intent_hash();
        let hash = intent_hash(DESTINATION, &route_hash, &reward.hash());
        let vault = vault_pda(&hash).0;
        context.airdrop(&vault, reward.native_amount).unwrap();
        set_prover_proof(
            &mut context,
            &hash,
            local_prover::ID,
            DESTINATION + 1,
            claimant,
        );
        set_prover_proof(
            &mut context,
            &hash,
            hyper_prover::ID,
            DESTINATION,
            Pubkey::new_unique(),
        );
        context.warp_to_timestamp(reward.deadline.try_into().unwrap());
        for blocked in [true, false] {
            if !blocked {
                context
                    .set_account(Proof::pda(&hash, &hyper_prover::ID).0, Default::default())
                    .unwrap();
            }
            let query = common::aggregator_query(
                &hash,
                &[local_prover::ID, hyper_prover::ID, polymer_prover::ID],
            );
            let result = context.portal().refund_intent_with_accounts(
                DESTINATION,
                reward.clone(),
                vault,
                route_hash,
                Config::pda().0,
                WithdrawnMarker::pda(&hash).0,
                reward.creator,
                Some(reward.prover),
                [],
                query,
            );
            if blocked {
                assert!(result.is_err_and(common::is_error(
                    PortalError::IntentFulfilledAndNotWithdrawn
                )));
                assert_eq!(context.balance(&vault), reward.native_amount);
            } else {
                assert!(result.is_ok());
                assert_eq!(context.balance(&vault), 0);
            }
        }
    }
}
