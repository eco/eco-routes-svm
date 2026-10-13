//! Deliverability ceilings. litesvm does not enforce the 1232-byte packet
//! limit, so these compile the real transactions and measure them.
//!
//! Outbound: `[ComputeBudget, portal::prove]`, signed by the payer and the
//! fresh event keypair, with one lookup table holding every non-signer,
//! non-invoked account that exists before the transaction (FulfillMarkers,
//! cctp-prover's dispatcher/sender authority/program, the transmitter state
//! and program, the system program).
//!
//! Inbound: `[ComputeBudget, receive_message]` with one lookup table holding
//! `cctp::receive_lookup_table_addresses()`. The payer, the per-message
//! `used_nonce` PDA, the invoked transmitter, and each new `Proof` PDA are
//! static. The attestation is mainnet's 2-of-2 (130 bytes).
//!
//! Atomic: the solver's `[ComputeBudget, portal::fulfill, portal::prove]`.

use anchor_lang::prelude::borsh;
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::spl_token;
use cctp_prover::cctp;
use cctp_prover::constants::{MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_MESSAGE};
use cctp_prover::state::MAX_PEERS;
use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::state::{executor_pda, FulfillMarker};
use portal::types::{self, Call, Calldata, CalldataWithAccounts, Route};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_packet::PACKET_DATA_SIZE;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::{v0, v1, AddressLookupTableAccount, Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::cctp_prover_context::{
    attest, build_init_instruction, evm_peer, evm_sender, install_transmitter, peers, proof_pdas,
    prove_accounts, TestAttester, BASE_CHAIN_ID, BASE_DOMAIN, COMPUTE_UNIT_LIMIT,
};

pub mod common;

/// Solana's per-transaction account lock limit.
const MAX_TX_ACCOUNT_LOCKS: usize = 64;
/// Mainnet attestations carry two 65-byte signatures.
const MAINNET_ATTESTATION_LEN: usize = 2 * 65;

fn compile(payer: &Pubkey, instructions: &[Instruction], table: Vec<Pubkey>) -> v0::Message {
    let tables = [AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: table,
    }];

    v0::Message::try_compile(payer, instructions, &tables, Hash::default()).unwrap()
}

/// Compact-u16 signature count, `signatures` signatures, then the message.
fn v0_len(message: v0::Message, signatures: usize) -> usize {
    1 + 64 * signatures + VersionedMessage::V0(message).serialize().len()
}

/// Every account the transaction loads: the static `account_keys` plus each
/// lookup's `writable_indexes` and `readonly_indexes`.
fn account_count(message: &v0::Message) -> usize {
    message.account_keys.len()
        + message
            .address_table_lookups
            .iter()
            .map(|lookup| lookup.writable_indexes.len() + lookup.readonly_indexes.len())
            .sum::<usize>()
}

fn ceiling(len: impl Fn(usize) -> usize, max: usize) -> usize {
    (1..=max)
        .take_while(|&n| len(n) <= PACKET_DATA_SIZE)
        .last()
        .unwrap_or(0)
}

fn prove_instruction(payer: &Pubkey, event: &Pubkey, hashes: &[Bytes32]) -> Instruction {
    let markers = hashes
        .iter()
        .map(|hash| AccountMeta::new_readonly(FulfillMarker::pda(hash).0, false));

    Instruction {
        program_id: portal::ID,
        accounts: portal::accounts::Prove {
            prover: cctp_prover::ID,
            dispatcher: portal::state::dispatcher_pda(&cctp_prover::ID).0,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(markers)
        .chain(prove_accounts(payer, event))
        .collect(),
        data: portal::instruction::Prove {
            args: portal::instructions::ProveArgs {
                prover: cctp_prover::ID,
                source_chain_domain_id: BASE_DOMAIN as u64,
                intent_hashes: hashes.to_vec(),
                data: common::cctp_prover_context::evm_recipient(0xcc),
            },
        }
        .data(),
    }
}

/// Every non-signer account of `instructions`; the compiler keeps invoked
/// programs and anything else that must stay static out of the lookups.
fn table_of(instructions: &[Instruction]) -> Vec<Pubkey> {
    instructions
        .iter()
        .flat_map(|ix| ix.accounts.iter())
        .filter(|meta| !meta.is_signer)
        .map(|meta| meta.pubkey)
        .collect()
}

fn outbound_message(n: usize) -> v0::Message {
    let payer = Pubkey::new_unique();
    let event = Pubkey::new_unique();
    let hashes: Vec<Bytes32> = (0..n).map(|i| [i as u8 + 1; 32].into()).collect();
    let instructions = [
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        prove_instruction(&payer, &event, &hashes),
    ];

    compile(&payer, &instructions, table_of(&instructions))
}

fn inbound_instructions(payer: &Pubkey, n: usize) -> Vec<Instruction> {
    let data = ProofData::new(
        BASE_CHAIN_ID,
        (0..n)
            .map(|i| IntentHashClaimant::new([i as u8 + 1; 32].into(), [7; 32].into()))
            .collect(),
    );
    let message = cctp::format_message(
        BASE_DOMAIN,
        cctp::SOLANA_DOMAIN,
        [9; 32],
        &Pubkey::new_from_array(evm_sender(0xba)),
        &cctp_prover::ID,
        1000,
        1000,
        &data.clone().to_bytes(),
    );
    let ix = cctp::receive_message_instruction(
        payer,
        message,
        vec![0; MAINNET_ATTESTATION_LEN],
        &proof_pdas(&data),
    )
    .unwrap();

    vec![
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        ix,
    ]
}

fn inbound_v0(n: usize) -> v0::Message {
    let payer = Pubkey::new_unique();

    compile(
        &payer,
        &inbound_instructions(&payer, n),
        cctp::receive_lookup_table_addresses(),
    )
}

fn inbound_legacy_len(n: usize) -> usize {
    let payer = Pubkey::new_unique();

    1 + 64
        + Message::new(&inbound_instructions(&payer, n), Some(&payer))
            .serialize()
            .len()
}

#[test]
fn outbound_ceiling_matches_max_intents_per_prove() {
    let measured = ceiling(|n| v0_len(outbound_message(n), 2), 64);
    let at_max = outbound_message(MAX_INTENTS_PER_PROVE);
    let len = v0_len(at_max.clone(), 2);
    let locks = account_count(&at_max);
    let over_len = v0_len(outbound_message(MAX_INTENTS_PER_PROVE + 1), 2);
    println!(
        "outbound ceiling: {measured} intents, {len} bytes ({} headroom), {locks} locks; {} intents: {over_len} bytes",
        PACKET_DATA_SIZE - len,
        MAX_INTENTS_PER_PROVE + 1,
    );

    assert_eq!(
        measured, MAX_INTENTS_PER_PROVE,
        "outbound ceiling measured at {measured}"
    );
    assert!(locks <= MAX_TX_ACCOUNT_LOCKS);
}

#[test]
fn inbound_ceiling_matches_max_pairs_per_message() {
    let measured = ceiling(|n| v0_len(inbound_v0(n), 1), 64);
    let legacy = ceiling(inbound_legacy_len, 64);
    let at_max = inbound_v0(MAX_PAIRS_PER_MESSAGE);
    let len = v0_len(at_max.clone(), 1);
    let locks = account_count(&at_max);
    println!(
        "inbound ceiling: v0 + table {measured} pairs ({len} bytes, {} headroom, {locks} locks), legacy {legacy} pairs",
        PACKET_DATA_SIZE - len,
    );

    assert_eq!(
        measured, MAX_PAIRS_PER_MESSAGE,
        "inbound ceiling measured at {measured}"
    );
    assert!(legacy < MAX_PAIRS_PER_MESSAGE);
    assert!(locks <= MAX_TX_ACCOUNT_LOCKS);
}

/// A full-size inbound batch executes through the real transmitter with a
/// 2-of-2 attestation under the relayer's compute limit.
#[test]
fn max_inbound_batch_executes_under_compute_limit() {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.cctp_prover().init(&authority, peers()).unwrap();
    let attesters = [TestAttester::new([0x41; 32]), TestAttester::new([0x42; 32])];
    install_transmitter(&mut context, &attesters, 2);
    let data = ProofData::new(
        BASE_CHAIN_ID,
        (0..MAX_PAIRS_PER_MESSAGE)
            .map(|i| IntentHashClaimant::new([i as u8 + 1; 32].into(), [7; 32].into()))
            .collect(),
    );
    let message = cctp::format_message(
        BASE_DOMAIN,
        cctp::SOLANA_DOMAIN,
        [8; 32],
        &Pubkey::new_from_array(evm_sender(0xba)),
        &cctp_prover::ID,
        1000,
        1000,
        &data.clone().to_bytes(),
    );
    let attestation = attest(&attesters, &message);
    assert_eq!(attestation.len(), MAINNET_ATTESTATION_LEN);
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, attestation, &proof_pdas(&data))
        .unwrap();

    let result = context.cctp_prover().send(vec![ix], &[]).unwrap();

    println!(
        "{MAX_PAIRS_PER_MESSAGE} pairs: {} CU",
        result.compute_units_consumed
    );
    assert!(result.compute_units_consumed < COMPUTE_UNIT_LIMIT as u64);
    assert!(proof_pdas(&data)
        .iter()
        .all(|pda| context.get_account(pda).is_some()));
}

/// A full peer set overflows a v0 `init` but fits a v1 one.
#[test]
fn max_peers_init_needs_v1() {
    let deployer = Pubkey::new_unique();
    let ix = build_init_instruction(
        &deployer,
        &deployer,
        (0..MAX_PEERS)
            .map(|i| evm_peer(100 + i as u32, 900_000 + i as u64, i as u8 + 1))
            .collect(),
    );
    let v0 = v0_len(compile(&deployer, std::slice::from_ref(&ix), vec![]), 1);
    let v1_len =
        VersionedMessage::V1(v1::Message::try_compile(&deployer, &[ix], Hash::default()).unwrap())
            .serialize()
            .len()
            + 64;

    println!("init with {MAX_PEERS} peers: v0 {v0} bytes, v1 {v1_len} bytes");
    assert!(v0 > PACKET_DATA_SIZE);
    assert!(v1_len <= v1::MAX_TRANSACTION_SIZE);
}

/// The solver's atomic transaction: `[ComputeBudget, portal::fulfill (1 token
/// transfer + 1 call), portal::prove(cctp_prover, that intent)]`, v0, one
/// lookup table, signed by the payer, the solver and the event keypair.
///
/// The lookup table holds every non-signer account that exists before the
/// transaction. The `FulfillMarker` and the executor's ATA are created by
/// `fulfill` itself, so they stay static (conservative: an already-existing
/// executor ATA could move into the table).
///
/// Measured: 946 bytes and 20 account locks, so the solver can fulfill and
/// prove one intent in a single v0 transaction. Using the solver as the payer
/// (payer == solver) drops one signature and one static key, shrinking it
/// further.
#[test]
fn atomic_fulfill_and_prove_one_intent_fits_v0() {
    let mut ctx = common::Context::default();
    ctx.cctp_prover().install(Pubkey::new_unique());
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    route.tokens.truncate(1);
    let token = route.tokens[0].clone();
    let token_program = ctx.token_program;
    let executor = executor_pda().0;
    let solver = ctx.solver.pubkey();
    let payer = ctx.payer.pubkey();
    let recipient = Pubkey::new_unique();
    let ata = |owner: &Pubkey| {
        get_associated_token_address_with_program_id(owner, &token.token, &token_program)
    };
    let (executor_ata, recipient_ata, solver_ata) = (ata(&executor), ata(&recipient), ata(&solver));
    let calldata = Calldata {
        data: spl_token::instruction::transfer_checked(
            &token_program,
            &executor_ata,
            &token.token,
            &recipient_ata,
            &executor,
            &[],
            token.amount,
            6,
        )
        .unwrap()
        .data,
        account_count: 4,
    };
    let call_accounts = vec![
        AccountMeta::new(executor_ata, false),
        AccountMeta::new_readonly(token.token, false),
        AccountMeta::new(recipient_ata, false),
        AccountMeta::new_readonly(executor, false),
    ];
    let source_call = Call {
        target: token_program.to_bytes().into(),
        data: borsh::to_vec(
            &CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap(),
        )
        .unwrap(),
    };
    let mut source_route: Route = route.clone();
    source_route.calls = vec![source_call];
    route.calls = vec![Call {
        target: token_program.to_bytes().into(),
        data: borsh::to_vec(&calldata).unwrap(),
    }];
    let reward_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let marker = FulfillMarker::pda(&intent_hash).0;
    ctx.airdrop_token_ata(&token.token, &solver, token.amount);
    ctx.airdrop_token_ata(&token.token, &recipient, 0);

    let fulfill = Instruction {
        program_id: portal::ID,
        accounts: portal::accounts::Fulfill {
            payer,
            solver,
            executor,
            fulfill_marker: marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain([
            AccountMeta::new(solver_ata, false),
            AccountMeta::new(executor_ata, false),
            AccountMeta::new_readonly(token.token, false),
        ])
        .chain(call_accounts)
        .collect(),
        data: portal::instruction::Fulfill {
            args: portal::instructions::FulfillArgs {
                intent_hash,
                route: route.clone(),
                reward_hash,
                claimant: Pubkey::new_unique().to_bytes().into(),
            },
        }
        .data(),
    };
    let event = Keypair::new();
    let prove = prove_instruction(&payer, &event.pubkey(), &[intent_hash]);
    let instructions = [
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        fulfill,
        prove,
    ];
    let created_in_tx = [marker, executor_ata];
    let table: Vec<_> = table_of(&instructions)
        .into_iter()
        .filter(|key| !created_in_tx.contains(key))
        .collect();

    let message = compile(&payer, &instructions, table);
    let len = v0_len(message.clone(), 3);
    let locks = account_count(&message);
    println!(
        "atomic fulfill+prove: {len} bytes ({} headroom of {PACKET_DATA_SIZE}), {locks} locks (limit {MAX_TX_ACCOUNT_LOCKS})",
        PACKET_DATA_SIZE as i64 - len as i64,
    );

    // The same instructions execute, so the measured shape is a real one.
    let signers = [&ctx.payer, &ctx.solver, &event];
    let transaction = Transaction::new(
        &signers,
        Message::new(&instructions, Some(&payer)),
        ctx.latest_blockhash(),
    );
    let result = ctx.send_transaction(transaction).unwrap();
    println!("atomic fulfill+prove: {} CU", result.compute_units_consumed);

    assert!(len <= PACKET_DATA_SIZE, "atomic tx is {len} bytes");
    assert!(locks <= MAX_TX_ACCOUNT_LOCKS);
}

/// A full-size outbound batch executes through the real transmitter.
#[test]
fn max_outbound_batch_executes_under_compute_limit() {
    let mut context = common::Context::default();
    context.cctp_prover().install(Pubkey::new_unique());
    let intents = context.fulfill_rand_intents(MAX_INTENTS_PER_PROVE, cctp_prover::ID);

    let result = context
        .portal()
        .prove_intent_via_cctp_prover(
            intents.iter().map(|intent| intent.intent_hash).collect(),
            BASE_DOMAIN as u64,
            intents
                .iter()
                .map(|intent| FulfillMarker::pda(&intent.intent_hash).0)
                .collect(),
            common::cctp_prover_context::evm_recipient(0xcc),
            &Keypair::new(),
        )
        .unwrap();

    println!(
        "{MAX_INTENTS_PER_PROVE} intents: {} CU",
        result.compute_units_consumed
    );
    assert!(result.compute_units_consumed < COMPUTE_UNIT_LIMIT as u64);
}
