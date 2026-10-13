use anchor_lang::prelude::borsh;
use anchor_lang::{AnchorSerialize, InstructionData};
use eco_svm_std::prover::{
    GetProofArgs, Proof, ProverError, CLOSE_PROOF_DISCRIMINATOR, GET_PROOF_DISCRIMINATOR,
};
use eco_svm_std::{claimant, Bytes32, CHAIN_ID};
use portal::instructions::PortalError;
use portal::state::{vault_pda, WithdrawnMarker};
use portal::types::intent_hash;
use serde_json::{json, Value};
use solana_sdk::account::Account;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

fn query(
    context: &mut common::Context,
    prover: Pubkey,
    proof: Pubkey,
    args: GetProofArgs,
) -> common::TransactionResult {
    let mut data = GET_PROOF_DISCRIMINATOR.to_vec();
    args.serialize(&mut data).unwrap();
    context.send_instruction(Instruction {
        program_id: prover,
        accounts: vec![AccountMeta::new_readonly(proof, false)],
        data,
    })
}

#[test]
fn concrete_provers_return_only_valid_canonical_proofs() {
    for prover in [local_prover::ID, hyper_prover::ID, polymer_prover::ID] {
        let mut context = common::Context::default();
        let hash: Bytes32 = [12; 32].into();
        let proof = Proof::pda(&hash, &prover).0;
        let claimant = Pubkey::new_unique();
        let args = GetProofArgs::new(hash, CHAIN_ID, vec![]);
        assert_eq!(
            query(&mut context, prover, proof, args.clone())
                .unwrap()
                .return_data
                .data,
            vec![0]
        );
        context.airdrop(&proof, 1_000_000).unwrap();
        assert_eq!(
            query(&mut context, prover, proof, args.clone())
                .unwrap()
                .return_data
                .data,
            vec![0]
        );
        for (destination, actual_claimant, expected) in [
            (CHAIN_ID, claimant, true),
            (CHAIN_ID + 1, claimant, false),
            (CHAIN_ID, Pubkey::new_unique(), true),
            (CHAIN_ID, Pubkey::default(), false),
        ] {
            context.set_proof(proof, Proof::new(destination, actual_claimant), prover);
            let before = context.get_account(&proof).unwrap();
            let result = query(&mut context, prover, proof, args.clone()).unwrap();
            assert_eq!(result.return_data.program_id, prover);
            assert_eq!(
                result.return_data.data,
                borsh::to_vec(&expected.then(|| Proof::new(destination, actual_claimant))).unwrap()
            );
            assert_eq!(context.get_account(&proof).unwrap(), before);
        }
        context.set_proof(proof, Proof::new(CHAIN_ID, claimant::cancelled()), prover);
        assert_eq!(
            query(&mut context, prover, proof, args.clone())
                .unwrap()
                .return_data
                .data,
            borsh::to_vec(&Some(Proof::new(CHAIN_ID, claimant::cancelled()))).unwrap(),
        );
        context.set_proof(proof, Proof::new(CHAIN_ID, claimant), prover);
        let valid = context.get_account(&proof).unwrap();
        for mutation in 0..2 {
            let mut invalid = valid.clone();
            match mutation {
                0 => invalid.owner = Pubkey::new_unique(),
                _ => invalid.data[0] ^= 1,
            }
            context.set_account(proof, invalid).unwrap();
            assert_eq!(
                query(&mut context, prover, proof, args.clone())
                    .unwrap()
                    .return_data
                    .data,
                vec![0]
            );
        }
        // an owned proof that does not decode is an error, never an absent proof
        for truncate in [true, false] {
            let mut invalid = valid.clone();
            if truncate {
                invalid.data.pop();
            } else {
                invalid.data.push(0);
            }
            context.set_account(proof, invalid).unwrap();
            assert!(query(&mut context, prover, proof, args.clone())
                .is_err_and(common::is_error(ProverError::InvalidProof)));
        }
        assert!(query(&mut context, prover, Pubkey::new_unique(), args)
            .is_err_and(common::is_error(ProverError::InvalidProof)));
    }
}

#[test]
fn invalid_proof_responses_never_authorize_timeout_refund() {
    for mode in 0..10 {
        let mut context = common::Context::default();
        let (_, _, mut reward) = context.rand_intent();
        let destination = CHAIN_ID;
        reward.prover = malicious_proof_closer::ID;
        reward.tokens.clear();
        let route_hash: Bytes32 = [13; 32].into();
        let hash = intent_hash(destination, &route_hash, &reward.hash());
        let proof = Proof::pda(&hash, &reward.prover).0;
        let vault = vault_pda(&hash).0;
        context.airdrop(&vault, 1_000_000_000).unwrap();
        context
            .set_account(
                proof,
                Account {
                    lamports: 1_000_000,
                    data: vec![mode],
                    owner: reward.prover,
                    ..Account::default()
                },
            )
            .unwrap();
        context.warp_to_timestamp(reward.deadline.try_into().unwrap());
        let payer = context.payer.pubkey();
        let result = context.portal().refund_intent_with_accounts(
            destination,
            reward.clone(),
            vault,
            route_hash,
            proof,
            WithdrawnMarker::pda(&hash).0,
            reward.creator,
            Some(reward.prover),
            [],
            vec![
                AccountMeta::new_readonly(local_prover::ID, false),
                AccountMeta::new_readonly(Proof::pda(&hash, &local_prover::ID).0, false),
                AccountMeta::new(payer, true),
            ],
        );
        if mode == 5 {
            assert!(result.is_ok());
            assert_eq!(context.balance(&vault), 0);
            continue;
        }
        if mode == 4 {
            assert!(result.is_err());
        } else {
            assert!(result.is_err_and(common::is_error(ProverError::InvalidReturnData)));
        }
        assert_eq!(context.balance(&vault), 1_000_000_000);
        assert!(context
            .get_account(&WithdrawnMarker::pda(&hash).0)
            .is_none());
    }
}

#[test]
fn cancellation_cleanup_rejects_wrong_destination() {
    for claimant in [Pubkey::new_unique(), claimant::cancelled()] {
        let mut context = common::Context::default();
        let (destination, _, mut reward) = context.rand_intent();
        reward.prover = local_prover::ID;
        let route_hash: Bytes32 = [14; 32].into();
        let hash = intent_hash(destination, &route_hash, &reward.hash());
        let proof = Proof::pda(&hash, &reward.prover).0;
        context.warp_to_timestamp(reward.deadline.try_into().unwrap());
        context.set_proof(proof, Proof::new(destination + 1, claimant), reward.prover);
        let accounts = vec![
            AccountMeta::new(proof, false),
            AccountMeta::new(context.payer.pubkey(), true),
        ];
        assert!(context
            .portal()
            .close_proof(destination, route_hash, reward, accounts)
            .is_err_and(common::is_error(PortalError::IntentNotCancelled)));
        assert!(context.get_account(&proof).is_some());
    }
}

#[test]
fn shared_instruction_discriminators_and_generated_idls_match() {
    let args = GetProofArgs::new([0; 32].into(), 0, vec![]);
    for data in [
        local_prover::instruction::GetProof { args: args.clone() }.data(),
        hyper_prover::instruction::GetProof { args: args.clone() }.data(),
        polymer_prover::instruction::GetProof { args: args.clone() }.data(),
        aggregator_prover::instruction::GetProof { args }.data(),
    ] {
        assert_eq!(data[..8], GET_PROOF_DISCRIMINATOR);
    }
    for name in [
        "local_prover",
        "hyper_prover",
        "polymer_prover",
        "aggregator_prover",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../target/idl/{name}.json"));
        let idl: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let query_args = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|definition| definition["name"] == "GetProofArgs")
            .unwrap();
        assert_eq!(
            query_args["type"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| field["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["intent_hash", "destination", "data"]
        );
        let instructions = idl["instructions"].as_array().unwrap();
        let validate = instructions
            .iter()
            .find(|instruction| instruction["name"] == "get_proof")
            .unwrap();
        assert_eq!(
            validate["returns"],
            json!({"option": {"defined": {"name": "Proof"}}})
        );
        assert_eq!(validate["discriminator"], json!(GET_PROOF_DISCRIMINATOR));
        let close = instructions
            .iter()
            .find(|instruction| instruction["name"] == "close_proof")
            .unwrap();
        assert_eq!(close["discriminator"], json!(CLOSE_PROOF_DISCRIMINATOR));
        assert_eq!(close["args"][0]["name"], "args");
        if name == "aggregator_prover" {
            assert!(instructions
                .iter()
                .all(|instruction| instruction["name"] != "aggregate"));
            assert!(idl
                .get("events")
                .is_none_or(|events| events.as_array().unwrap().is_empty()));
            assert!(idl["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|account| account["name"] != "ProofAccount"));
        }
    }
}
