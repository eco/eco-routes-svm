use std::iter;

use aggregator_prover::instructions::{AggregateArgs, AggregatorProverError, IntentPreimage};
use aggregator_prover::state::{Config, ProofAccount, MAX_MEMBERS};
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

const MEMBERS: [Pubkey; 2] = [hyper_prover::ID, local_prover::ID];
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
        .init(&authority, MEMBERS.to_vec())
        .unwrap();

    context
}

fn args(claimant: Pubkey) -> AggregateArgs {
    let preimage = IntentPreimage {
        route_hash: [42; 32].into(),
        reward_hash: [43; 32].into(),
    };
    let hash = intent_hash(DESTINATION, &preimage.route_hash, &preimage.reward_hash);

    AggregateArgs {
        proof_data: ProofData::new(
            DESTINATION,
            vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
        ),
        preimages: vec![preimage],
    }
}

fn set_member_proof(
    context: &mut common::Context,
    args: &AggregateArgs,
    member: Pubkey,
    destination: u64,
    claimant: Pubkey,
) {
    let hash = args.proof_data.intent_hashes_claimants[0].intent_hash;
    context.set_proof(
        Proof::pda(&hash, &member).0,
        Proof::new(destination, claimant),
        member,
    );
}

fn aggregate_address(args: &AggregateArgs) -> Pubkey {
    Proof::pda(
        &args.proof_data.intent_hashes_claimants[0].intent_hash,
        &aggregator_prover::ID,
    )
    .0
}

#[test]
fn init_preserves_order_and_cannot_be_reinitialized() {
    let (mut context, authority) = setup();
    context.airdrop(&Config::pda().0, 1_000_000).unwrap();
    context
        .aggregator_prover()
        .init(&authority, MEMBERS.to_vec())
        .unwrap();
    let config: Config = context.account(&Config::pda().0).unwrap();
    assert_eq!(config.members, MEMBERS);
    let result = context
        .aggregator_prover()
        .init(&authority, MEMBERS.into_iter().rev().collect());
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}

#[test]
fn init_rejects_wrong_authority() {
    let (mut context, _) = setup();
    let result = context
        .aggregator_prover()
        .init(&Keypair::new(), MEMBERS.to_vec());
    assert!(result.is_err_and(common::is_error(AggregatorProverError::InvalidAuthority)));
}

#[test]
fn init_rejects_invalid_member_sets() {
    for members in [vec![], vec![MEMBERS[0]; MAX_MEMBERS + 1]] {
        let (mut context, authority) = setup();
        let result = context.aggregator_prover().init(&authority, members);
        assert!(result.is_err_and(common::is_error(AggregatorProverError::InvalidMemberSet)));
    }
}

#[test]
fn init_rejects_duplicate_members() {
    let (mut context, authority) = setup();
    let result = context
        .aggregator_prover()
        .init(&authority, vec![MEMBERS[0]; 2]);
    assert!(result.is_err_and(common::is_error(AggregatorProverError::DuplicateMember)));
}

#[test]
fn init_rejects_non_executable_zero_and_self_members() {
    for member in [
        Pubkey::new_unique(),
        Pubkey::default(),
        aggregator_prover::ID,
    ] {
        let (mut context, authority) = setup();
        let result = context.aggregator_prover().init(&authority, vec![member]);
        assert!(result.is_err_and(common::is_error(AggregatorProverError::InvalidMember)));
    }
}

#[test]
fn aggregate_selects_first_member_and_emits_standard_event() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let args = args(claimant);
    let hash = args.proof_data.intent_hashes_claimants[0].intent_hash;
    let proof_address = aggregate_address(&args);
    context.airdrop(&proof_address, 1_000_000).unwrap();
    set_member_proof(&mut context, &args, MEMBERS[0], DESTINATION, claimant);
    set_member_proof(
        &mut context,
        &args,
        MEMBERS[1],
        DESTINATION,
        Pubkey::new_unique(),
    );
    let result = context.aggregator_prover().aggregate(args, &MEMBERS);
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
fn aggregate_falls_back_to_second_member() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let args = args(claimant);
    set_member_proof(&mut context, &args, MEMBERS[1], DESTINATION, claimant);
    assert!(context
        .aggregator_prover()
        .aggregate(args, &MEMBERS)
        .is_ok());
}

#[test]
fn aggregate_wrong_destination_does_not_shadow_valid_member() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let args = args(claimant);
    set_member_proof(
        &mut context,
        &args,
        MEMBERS[0],
        DESTINATION + 1,
        Pubkey::new_unique(),
    );
    set_member_proof(&mut context, &args, MEMBERS[1], DESTINATION, claimant);
    assert!(context
        .aggregator_prover()
        .aggregate(args, &MEMBERS)
        .is_ok());
}

#[test]
fn aggregate_skips_zero_claimants_malformed_data_and_wrong_owners() {
    for variant in 0..4 {
        let mut context = initialized();
        let claimant = Pubkey::new_unique();
        let args = args(claimant);
        set_member_proof(
            &mut context,
            &args,
            MEMBERS[0],
            DESTINATION,
            Pubkey::default(),
        );
        let address = Proof::pda(
            &args.proof_data.intent_hashes_claimants[0].intent_hash,
            &MEMBERS[0],
        )
        .0;
        let mut account = context.get_account(&address).unwrap();
        match variant {
            0 => (),
            1 => account.data.truncate(9),
            2 => account.owner = Pubkey::new_unique(),
            3 => account.data[0] ^= 1,
            _ => unreachable!(),
        }
        context.set_account(address, account).unwrap();
        set_member_proof(&mut context, &args, MEMBERS[1], DESTINATION, claimant);
        assert!(context
            .aggregator_prover()
            .aggregate(args, &MEMBERS)
            .is_ok());
    }
}

#[test]
fn aggregate_cannot_omit_reorder_or_substitute_members() {
    for members in [
        vec![MEMBERS[1]],
        vec![MEMBERS[1], MEMBERS[0]],
        vec![MEMBERS[0], Pubkey::new_unique()],
    ] {
        let mut context = initialized();
        let claimant = Pubkey::new_unique();
        let args = args(claimant);
        set_member_proof(&mut context, &args, MEMBERS[1], DESTINATION, claimant);
        assert!(context
            .aggregator_prover()
            .aggregate(args, &members)
            .is_err_and(common::is_error(AggregatorProverError::InvalidProof)));
    }
}

#[test]
fn aggregate_rejects_unproven_and_wrong_destination_only() {
    for wrong_destination in [false, true] {
        let mut context = initialized();
        let claimant = Pubkey::new_unique();
        let args = args(claimant);
        if wrong_destination {
            set_member_proof(&mut context, &args, MEMBERS[0], DESTINATION + 1, claimant);
        }
        assert!(context
            .aggregator_prover()
            .aggregate(args, &MEMBERS)
            .is_err_and(common::is_error(AggregatorProverError::NoMatchingProof)));
    }
}

#[test]
fn aggregate_rejects_forged_destination_preimage_and_claimant() {
    for variant in 0..3 {
        let mut context = initialized();
        let claimant = Pubkey::new_unique();
        let mut args = args(claimant);
        set_member_proof(&mut context, &args, MEMBERS[0], DESTINATION, claimant);
        let expected_error = match variant {
            0 => {
                args.proof_data.destination += 1;
                AggregatorProverError::InvalidIntentHash
            }
            1 => {
                args.preimages[0].route_hash = [99; 32].into();
                AggregatorProverError::InvalidIntentHash
            }
            2 => {
                args.proof_data.intent_hashes_claimants[0].claimant = [99; 32].into();
                AggregatorProverError::ClaimantMismatch
            }
            _ => unreachable!(),
        };
        assert!(context
            .aggregator_prover()
            .aggregate(args, &MEMBERS)
            .is_err_and(common::is_error(expected_error)));
    }
}

#[test]
fn aggregate_rejects_mismatched_preimage_count() {
    for count in [0, 2] {
        let mut context = initialized();
        let mut args = args(Pubkey::new_unique());
        args.preimages = vec![args.preimages[0].clone(); count];
        assert!(context
            .aggregator_prover()
            .aggregate(args, &MEMBERS)
            .is_err_and(common::is_error(AggregatorProverError::InvalidData)));
    }
}

#[test]
fn aggregate_repeated_proof_is_idempotent_but_cannot_change_claimant() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    set_member_proof(
        &mut context,
        &args(claimant),
        MEMBERS[0],
        DESTINATION,
        claimant,
    );
    context
        .aggregator_prover()
        .aggregate(args(claimant), &MEMBERS)
        .unwrap();
    context.expire_blockhash();
    assert!(context
        .aggregator_prover()
        .aggregate(args(claimant), &MEMBERS)
        .is_ok());
    let other_claimant = Pubkey::new_unique();
    set_member_proof(
        &mut context,
        &args(other_claimant),
        MEMBERS[0],
        DESTINATION,
        other_claimant,
    );
    assert!(context
        .aggregator_prover()
        .aggregate(args(other_claimant), &MEMBERS)
        .is_err_and(common::is_error(AggregatorProverError::IntentAlreadyProven)));
}

#[test]
fn aggregate_batches_are_atomic() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let mut args = args(claimant);
    let proof_address = aggregate_address(&args);
    set_member_proof(&mut context, &args, MEMBERS[0], DESTINATION, claimant);
    let mut preimages = args.preimages.clone();
    let second = IntentPreimage {
        route_hash: [44; 32].into(),
        reward_hash: [45; 32].into(),
    };
    args.proof_data
        .intent_hashes_claimants
        .push(IntentHashClaimant::new(
            intent_hash(DESTINATION, &second.route_hash, &second.reward_hash),
            claimant.to_bytes().into(),
        ));
    preimages.push(second);
    args.preimages = preimages;
    assert!(context
        .aggregator_prover()
        .aggregate(args, &MEMBERS)
        .is_err_and(common::is_error(AggregatorProverError::NoMatchingProof)));
    assert!(context.get_account(&proof_address).is_none());
}

#[test]
fn close_proof_rejects_unscoped_signer() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let args = args(claimant);
    let proof_address = aggregate_address(&args);
    set_member_proof(&mut context, &args, MEMBERS[0], DESTINATION, claimant);
    context
        .aggregator_prover()
        .aggregate(args, &MEMBERS)
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
            .init(&authority, MEMBERS.to_vec())
            .unwrap();
        let (reward, route_hash, hash) = funded(&mut context);
        let claimant = Pubkey::new_unique();
        let args = AggregateArgs {
            proof_data: ProofData::new(
                DESTINATION,
                vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
            ),
            preimages: vec![IntentPreimage {
                route_hash,
                reward_hash: reward.hash(),
            }],
        };
        set_member_proof(&mut context, &args, MEMBERS[1], DESTINATION, claimant);
        context
            .aggregator_prover()
            .aggregate(args, &MEMBERS)
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
            .get_account(&Proof::pda(&hash, &MEMBERS[1]).0)
            .is_some());
        assert!(context.get_account(&marker).is_some());
    }
}

#[test]
fn max_members_can_resolve_last_proof() {
    let (mut context, authority) = setup();
    let members: Vec<_> = (0..MAX_MEMBERS).map(|_| Pubkey::new_unique()).collect();
    members.iter().for_each(|member| {
        context
            .add_program(*member, include_bytes!("../../target/deploy/dummy_ism.so"))
            .unwrap();
    });
    context
        .aggregator_prover()
        .init(&authority, members.clone())
        .unwrap();
    let claimant = Pubkey::new_unique();
    let args = args(claimant);
    set_member_proof(
        &mut context,
        &args,
        members[MAX_MEMBERS - 1],
        DESTINATION,
        claimant,
    );
    let result = context
        .aggregator_prover()
        .aggregate(args, &members)
        .unwrap();
    assert!(result.compute_units_consumed < 100_000);
    println!(
        "eight-member aggregation: {} CU",
        result.compute_units_consumed
    );
}

#[test]
fn aggregate_rejects_substituted_aggregate_pda() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let args = args(claimant);
    set_member_proof(&mut context, &args, MEMBERS[0], DESTINATION, claimant);
    let mut instruction = context
        .aggregator_prover()
        .build_aggregate_instruction(args, &MEMBERS);
    instruction.accounts[5].pubkey = Pubkey::new_unique();
    assert!(context
        .aggregator_prover()
        .send_instruction(instruction)
        .is_err_and(common::is_error(AggregatorProverError::InvalidProof)));
}

#[test]
fn later_higher_priority_proof_cannot_overwrite_recorded_claimant() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let original = args(claimant);
    let address = aggregate_address(&original);
    set_member_proof(&mut context, &original, MEMBERS[1], DESTINATION, claimant);
    context
        .aggregator_prover()
        .aggregate(original, &MEMBERS)
        .unwrap();
    let other_claimant = Pubkey::new_unique();
    let later = args(other_claimant);
    set_member_proof(
        &mut context,
        &later,
        MEMBERS[0],
        DESTINATION,
        other_claimant,
    );
    assert!(context
        .aggregator_prover()
        .aggregate(later, &MEMBERS)
        .is_err_and(common::is_error(AggregatorProverError::IntentAlreadyProven)));
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
fn aggregate_batch_resolves_members_independently() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let other_claimant = Pubkey::new_unique();
    let mut args = args(claimant);
    let first_address = aggregate_address(&args);
    set_member_proof(&mut context, &args, MEMBERS[1], DESTINATION, claimant);
    let mut preimages = args.preimages.clone();
    let second = IntentPreimage {
        route_hash: [44; 32].into(),
        reward_hash: [45; 32].into(),
    };
    let second_hash = intent_hash(DESTINATION, &second.route_hash, &second.reward_hash);
    context.set_proof(
        Proof::pda(&second_hash, &MEMBERS[0]).0,
        Proof::new(DESTINATION, other_claimant),
        MEMBERS[0],
    );
    args.proof_data
        .intent_hashes_claimants
        .push(IntentHashClaimant::new(
            second_hash,
            other_claimant.to_bytes().into(),
        ));
    preimages.push(second);
    args.preimages = preimages;
    context
        .aggregator_prover()
        .aggregate(args, &MEMBERS)
        .unwrap();
    assert_eq!(
        context
            .account::<ProofAccount>(&first_address)
            .unwrap()
            .0
            .claimant,
        claimant
    );
    assert_eq!(
        context
            .account::<ProofAccount>(&Proof::pda(&second_hash, &aggregator_prover::ID).0)
            .unwrap()
            .0
            .claimant,
        other_claimant
    );
}

#[test]
fn existing_portal_refunds_unaggregated_intent_after_deadline() {
    let mut context = initialized();
    let (reward, route_hash, hash) = funded(&mut context);
    let vault = vault_pda(&hash).0;
    // This pins the integration boundary: member delivery alone is not settlement readiness.
    context.set_proof(
        Proof::pda(&hash, &MEMBERS[0]).0,
        Proof::new(DESTINATION, Pubkey::new_unique()),
        MEMBERS[0],
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
        AggregateArgs {
            proof_data,
            preimages: vec![IntentPreimage {
                route_hash: intent.route.hash(),
                reward_hash: intent.reward_hash,
            }],
        },
        &MEMBERS,
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
