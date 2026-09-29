use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::instructions::PortalError;
use portal::state::WithdrawnMarker;
use portal::types::{intent_hash, Reward};
use solana_sdk::account::Account;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

pub mod common;

#[test]
fn forwarded_authority_cannot_close_another_intents_proof() {
    for prover in [local_prover::ID, hyper_prover::ID, polymer_prover::ID] {
        for substitute_hash in [false, true] {
            let mut context = common::Context::default();
            let victim_hash: Bytes32 = [3; 32].into();
            let victim_proof = Proof::pda(&victim_hash, &prover).0;
            context.set_proof(
                victim_proof,
                Proof::new(CHAIN_ID, Pubkey::new_unique()),
                prover,
            );
            let victim_before = context.get_account(&victim_proof).unwrap();
            let reward = Reward {
                deadline: context.now() + 3600,
                creator: context.creator.pubkey(),
                prover: malicious_proof_closer::ID,
                native_amount: 0,
                tokens: vec![],
            };
            let route_hash: Bytes32 = [4; 32].into();
            let hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
            context.set_withdrawn_marker(WithdrawnMarker::pda(&hash).0);
            let own_proof = Proof::pda(&hash, &reward.prover).0;
            if substitute_hash {
                context
                    .set_account(
                        own_proof,
                        Account {
                            lamports: 1_000_000,
                            data: victim_hash.to_vec(),
                            owner: malicious_proof_closer::ID,
                            ..Account::default()
                        },
                    )
                    .unwrap();
            }
            let recipient = if prover == hyper_prover::ID {
                hyper_prover::state::pda_payer_pda().0
            } else {
                context.payer.pubkey()
            };
            let result = context.portal().close_proof(
                CHAIN_ID,
                route_hash,
                reward,
                vec![
                    AccountMeta::new_readonly(own_proof, false),
                    AccountMeta::new_readonly(prover, false),
                    AccountMeta::new(victim_proof, false),
                    AccountMeta::new(recipient, prover != hyper_prover::ID),
                ],
            );
            let code = match (prover, substitute_hash) {
                (program, true) if program == local_prover::ID => {
                    local_prover::instructions::LocalProverError::InvalidPortalProofCloser as u32
                }
                (program, false) if program == local_prover::ID => {
                    local_prover::instructions::LocalProverError::InvalidProof as u32
                }
                (program, true) if program == hyper_prover::ID => {
                    hyper_prover::instructions::HyperProverError::InvalidPortalProofCloser as u32
                }
                (program, false) if program == hyper_prover::ID => {
                    hyper_prover::instructions::HyperProverError::InvalidProof as u32
                }
                (_, true) => {
                    polymer_prover::instructions::PolymerProverError::InvalidPortalProofCloser
                        as u32
                }
                (_, false) => polymer_prover::instructions::PolymerProverError::InvalidProof as u32,
            };
            assert!(
                result.clone().is_err_and(common::reached_program(prover)),
                "{result:?}"
            );
            assert!(result.is_err_and(common::is_program_error(prover, code + 6000)));
            assert_eq!(context.get_account(&victim_proof).unwrap(), victim_before);
        }
    }
}

#[test]
fn cleanup_rejects_forged_markers_and_wrong_closer() {
    let mut context = common::Context::default();
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = local_prover::ID;
    let route_hash: Bytes32 = [4; 32].into();
    let hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let marker = WithdrawnMarker::pda(&hash).0;
    let proof = Proof::pda(&hash, &reward.prover).0;
    context.set_proof(
        proof,
        Proof::new(CHAIN_ID, Pubkey::new_unique()),
        reward.prover,
    );
    context.set_withdrawn_marker(marker);
    let valid_marker = context.get_account(&marker).unwrap();
    let accounts = vec![
        AccountMeta::new(proof, false),
        AccountMeta::new(context.payer.pubkey(), true),
    ];
    for wrong_owner in [false, true] {
        let mut forged = valid_marker.clone();
        if wrong_owner {
            forged.owner = local_prover::ID;
        } else {
            forged.data.fill(0);
        }
        context.set_account(marker, forged).unwrap();
        assert!(context
            .portal()
            .close_proof(CHAIN_ID, route_hash, reward.clone(), accounts.clone())
            .is_err_and(common::is_error(PortalError::InvalidWithdrawnMarker)));
        assert!(context.get_account(&proof).is_some());
    }
    context.set_account(marker, valid_marker).unwrap();
    let mut instruction = context
        .portal()
        .close_proof_instruction(CHAIN_ID, route_hash, reward, accounts);
    instruction.accounts[2].pubkey = Pubkey::new_unique();
    assert!(context
        .aggregator_prover()
        .send_instruction(instruction)
        .is_err_and(common::is_error(PortalError::InvalidProofCloser)));
}

#[test]
fn bundled_cleanup_failure_rolls_back_withdrawal() {
    let mut context = common::Context::default();
    let (_, _, mut reward) = context.rand_intent();
    reward.prover = local_prover::ID;
    reward.tokens.clear();
    let route_hash: Bytes32 = [15; 32].into();
    let hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let marker = WithdrawnMarker::pda(&hash).0;
    let vault = portal::state::vault_pda(&hash).0;
    context.airdrop(&vault, reward.native_amount).unwrap();
    let claimant = Pubkey::new_unique();
    let proof = Proof::pda(&hash, &reward.prover).0;
    context.set_proof(proof, Proof::new(CHAIN_ID, claimant), reward.prover);
    let payer = context.payer.pubkey();
    let withdraw = Instruction {
        program_id: portal::ID,
        accounts: portal::accounts::Withdraw {
            payer,
            claimant,
            vault,
            prover: reward.prover,
            withdrawn_marker: marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain([AccountMeta::new_readonly(proof, false)])
        .collect(),
        data: portal::instruction::Withdraw {
            args: portal::instructions::WithdrawArgs {
                destination: CHAIN_ID,
                route_hash,
                reward: reward.clone(),
            },
        }
        .data(),
    };
    let mut cleanup = context.portal().close_proof_instruction(
        CHAIN_ID,
        route_hash,
        reward.clone(),
        vec![
            AccountMeta::new(proof, false),
            AccountMeta::new(payer, true),
        ],
    );
    cleanup.accounts[2].pubkey = Pubkey::new_unique();
    let transaction = Transaction::new(
        &[&context.payer],
        Message::new(&[withdraw, cleanup], Some(&payer)),
        context.latest_blockhash(),
    );
    assert!(context
        .send_transaction(transaction)
        .is_err_and(common::is_error(PortalError::InvalidProofCloser)));
    assert!(context.get_account(&marker).is_none());
    assert!(context.get_account(&proof).is_some());
    assert_eq!(context.balance(&claimant), 0);
    assert_eq!(context.balance(&vault), reward.native_amount);
}
