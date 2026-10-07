//! Deliverability ceilings. litesvm does not enforce the 1232-byte packet
//! limit, so these compile the real v0 transactions and measure them.
//!
//! Outbound: `[ComputeBudget, portal::prove, send_message]` with one ALT
//! holding every non-signer, non-invoked account that exists before the
//! transaction (FulfillMarkers, store, dispatcher, LayerZero accounts, four
//! DVN worker quadruples). The `PendingSend` PDA is new per batch -> static.
//!
//! Inbound: the executor's delivery transaction modelled as
//! `[ComputeBudget limit, ComputeBudget price, executor pre_execute,
//! lz_receive, executor post_execute]`. Our ALT holds the fixed `lz_receive`
//! accounts and every peer's Nonce (`required_alt_addresses`, the contents
//! `set_alt` enforces); the PayloadHash and each new `Proof` PDA
//! are static. `pre_execute`/`post_execute` are modelled as
//! `[payer (s,w), execution context PDA (w)]` with 16 / 8 data bytes - the
//! devnet E2E confirms the real executor fits the same count.

use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use eco_svm_std::{Bytes32, CHAIN_ID};
use layerzero_prover::constants::{lz_receive_gas, MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_MESSAGE};
use layerzero_prover::instructions::required_alt_addresses;
use layerzero_prover::state::{PendingSend, Store};
use portal::state::FulfillMarker;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_packet::PACKET_DATA_SIZE;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::{v0, AddressLookupTableAccount, VersionedMessage};
use solana_sdk::pubkey::Pubkey;

use crate::common::layerzero_prover_context::{
    build_lz_receive_instruction, build_send_message_instruction, peers, receive_params,
    send_accounts, BASE_CHAIN_ID, BASE_EID,
};

pub mod common;

const DVN_COUNT: usize = 4;
const EXECUTOR_PROGRAM: Pubkey = Pubkey::new_from_array([0xe0; 32]);

/// Solana's per-transaction account lock limit.
const MAX_TX_ACCOUNT_LOCKS: usize = 64;

fn compile(payer: &Pubkey, instructions: &[Instruction], table: Vec<Pubkey>) -> v0::Message {
    let alt = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: table,
    };
    v0::Message::try_compile(payer, instructions, &[alt], Hash::default()).unwrap()
}

fn transaction_len(message: v0::Message) -> usize {
    // compact-u16 signature count, one signature, then the message.
    1 + 64 + VersionedMessage::V0(message).serialize().len()
}

/// Every account the transaction loads: the static `account_keys` plus each
/// lookup's `writable_indexes` and `readonly_indexes` (solana-message v0).
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

fn outbound_message(n: usize) -> v0::Message {
    let payer = Pubkey::new_unique();
    let receiver = peers()[0].address;
    let hashes: Vec<Bytes32> = (0..n).map(|i| [i as u8 + 1; 32].into()).collect();
    let markers: Vec<Pubkey> = hashes
        .iter()
        .map(|hash| FulfillMarker::pda(hash).0)
        .collect();
    let payload = ProofData::new(
        CHAIN_ID,
        hashes
            .iter()
            .map(|hash| IntentHashClaimant::new(*hash, [7; 32].into()))
            .collect(),
    )
    .to_bytes();
    let pending = PendingSend::pda(BASE_EID, &receiver, &payload).0;
    let dispatcher = portal::state::dispatcher_pda(&layerzero_prover::ID).0;

    let prove = Instruction {
        program_id: portal::ID,
        accounts: portal::accounts::Prove {
            prover: layerzero_prover::ID,
            dispatcher,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(
            markers
                .iter()
                .map(|marker| AccountMeta::new_readonly(*marker, false)),
        )
        .chain([
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(Store::pda().0, false),
            AccountMeta::new(pending, false),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        ])
        .collect(),
        data: portal::instruction::Prove {
            args: portal::instructions::ProveArgs {
                prover: layerzero_prover::ID,
                source_chain_domain_id: BASE_EID.into(),
                intent_hashes: hashes,
                data: receiver.to_vec(),
            },
        }
        .data(),
    };
    let mut send = build_send_message_instruction(pending, payer, payer, BASE_EID, &receiver, 1);
    let workers: Vec<AccountMeta> = (0..4 + 4 * DVN_COUNT)
        .map(|i| AccountMeta::new_readonly(Pubkey::new_from_array([0x40 + i as u8; 32]), false))
        .collect();
    send.accounts.extend(workers.clone());

    let table = markers
        .into_iter()
        .chain([
            layerzero_prover::ID,
            dispatcher,
            Store::pda().0,
            anchor_lang::system_program::ID,
        ])
        .chain(
            send_accounts(Store::pda().0, payer, BASE_EID, &receiver)
                .into_iter()
                .map(|meta| meta.pubkey)
                .filter(|key| *key != payer),
        )
        .chain(workers.into_iter().map(|meta| meta.pubkey))
        .collect();

    compile(
        &payer,
        &[
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            prove,
            send,
        ],
        table,
    )
}

fn outbound_len(n: usize) -> usize {
    transaction_len(outbound_message(n))
}

fn inbound_len(n: usize) -> usize {
    let payer = Pubkey::new_unique();
    let proof_data = ProofData::new(
        BASE_CHAIN_ID,
        (0..n)
            .map(|i| IntentHashClaimant::new([i as u8 + 1; 32].into(), [7; 32].into()))
            .collect(),
    );
    let params = receive_params(&peers()[0], u64::MAX, proof_data.clone());
    let accounts = layerzero_prover::instructions::lz_receive_accounts(&params, &proof_data);
    let lz_receive = build_lz_receive_instruction(&params, accounts);
    let execution_context = Pubkey::new_unique();
    let wrapper = |data_len: usize| Instruction {
        program_id: EXECUTOR_PROGRAM,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(execution_context, false),
        ],
        data: vec![0; data_len],
    };
    let table = required_alt_addresses(&Store::new(peers()).unwrap());

    transaction_len(compile(
        &payer,
        &[
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudgetInstruction::set_compute_unit_price(1),
            wrapper(16),
            lz_receive,
            wrapper(8),
        ],
        table,
    ))
}

#[test]
fn outbound_ceiling_matches_max_intents_per_prove() {
    let measured = ceiling(outbound_len, 64);
    assert_eq!(
        measured, MAX_INTENTS_PER_PROVE,
        "outbound ceiling measured at {measured}"
    );
    // Fitting the packet is not enough: the transaction must also stay within
    // the account lock limit, counting the accounts loaded through the ALT.
    let accounts = account_count(&outbound_message(MAX_INTENTS_PER_PROVE));
    println!("outbound at {MAX_INTENTS_PER_PROVE} intents: {accounts} accounts");
    assert!(accounts <= MAX_TX_ACCOUNT_LOCKS);
}

#[test]
fn inbound_ceiling_matches_max_pairs_per_message() {
    let measured = ceiling(inbound_len, 64);
    assert_eq!(
        measured, MAX_PAIRS_PER_MESSAGE,
        "inbound ceiling measured at {measured}"
    );
}

/// The EVM sender's options give `lz_receive` `lz_receive_gas(n)` compute
/// units; a full batch must run well under that.
#[test]
fn max_inbound_batch_executes_under_floor_cu() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let proof_data = ProofData::new(
        BASE_CHAIN_ID,
        (0..MAX_PAIRS_PER_MESSAGE)
            .map(|i| IntentHashClaimant::new([i as u8 + 1; 32].into(), [7; 32].into()))
            .collect(),
    );

    let result = context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, proof_data))
        .unwrap();

    let floor = lz_receive_gas(MAX_PAIRS_PER_MESSAGE) as u64;
    println!(
        "lz_receive of {MAX_PAIRS_PER_MESSAGE} pairs: {} CU (floor {floor})",
        result.compute_units_consumed
    );
    assert!(result.compute_units_consumed < floor);
}
