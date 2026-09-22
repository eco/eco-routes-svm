//! Checks against Polymer's real program rather than the localnet mock.
//!
//! The `#[ignore]` wiring test loads their published fixture proof through the
//! real `create_accounts` / `load_proof`, then runs our `validate`, which must
//! get past Polymer's verification and fail on our own topic-count check (the
//! fixture event has four topics, ours has two). Needs
//! `POLYMER_PROVER_SO=<path to dumped .so>`; run with `-- --ignored`.
//!
//! The hermetic layout test decodes the `result` account bytes that real
//! program wrote for the same proof (captured once, see
//! `fixtures/polymer/README.md`) with our hand-rolled mirror, so it runs on
//! every PR with no network.

use anchor_lang::prelude::{AccountInfo, AccountMeta};
use anchor_lang::AnchorSerialize;
use polymer_prover::event::evm_address_to_bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::polymer;
use polymer_prover::state::Config;
use solana_sdk::account::Account;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

pub mod common;

const FIXTURE_EMITTER: [u8; 20] = [
    0xf2, 0x21, 0x75, 0x0e, 0x52, 0xaa, 0x08, 0x08, 0x35, 0xd2, 0x95, 0x7f, 0x2e, 0xed, 0x0d, 0x5d,
    0x7d, 0xdd, 0x8c, 0x38,
];
const FIXTURE_SIGNER: [u8; 20] = [
    0x8d, 0x39, 0x21, 0xb9, 0x6a, 0x38, 0x15, 0xf4, 0x03, 0xfb, 0x3a, 0x4c, 0x7f, 0xf5, 0x25, 0x96,
    0x9d, 0x16, 0xf9, 0xe0,
];
/// OP Sepolia, the chain the fixture event was emitted on.
const FIXTURE_CHAIN_ID: u32 = 11155420;
const FIXTURE_PEPTIDE_CHAIN_ID: u64 = 901;
const FIXTURE_CLIENT_TYPE: &str = "proof_api";

fn decode_hex(hex: &str) -> Vec<u8> {
    let hex = hex.trim().trim_start_matches("0x");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn fixture_proof() -> Vec<u8> {
    decode_hex(include_str!("fixtures/polymer/op-proof-v2.hex"))
}

/// Polymer's `InternalAccount` body: authority, client_type, signer_addr, peptide_chain_id.
#[derive(AnchorSerialize)]
struct InternalAccount {
    authority: Pubkey,
    client_type: String,
    signer_addr: [u8; 20],
    peptide_chain_id: u64,
}

fn send(ctx: &mut common::Context, signer: &Keypair, ix: Instruction) -> common::TransactionResult {
    let tx = Transaction::new(
        &[signer],
        Message::new(&[ix], Some(&signer.pubkey())),
        ctx.latest_blockhash(),
    );
    ctx.send_transaction(tx)
}

/// `polymer::ValidationResult` is a hand-rolled mirror of Polymer's
/// `ValidationResultAccount`, and every other test validates it against the
/// mock's copy of the same struct. This one feeds it the bytes Polymer's real
/// program wrote, so a field reorder or width change in the mirror fails here.
#[test]
fn polymer_written_result_account_decodes_with_our_mirror() {
    let mut data = decode_hex(include_str!(
        "fixtures/polymer/validation-result-v1.0.4.hex"
    ));
    let key = polymer::result_pda(&Pubkey::new_unique()).0;
    let owner = polymer::POLYMER_PROVER_ID;
    let mut lamports = 0u64;
    let account = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);

    let result = polymer::ValidationResult::try_from_account_info(&account).unwrap();
    assert!(result.is_valid);
    assert_eq!(result.error_message, "");
    assert_eq!(result.chain_id, FIXTURE_CHAIN_ID);
    assert_eq!(result.emitting_contract, FIXTURE_EMITTER);
    // Four topics; our own event has two, which is why the wiring test below
    // expects `InvalidTopicsLength`.
    assert_eq!(result.topics.len(), 4 * 32);
    // One ABI word of non-indexed data.
    assert_eq!(result.unindexed_data.len(), 32);
}

#[test]
#[ignore = "needs POLYMER_PROVER_SO pointing at a dumped polymer_prover.so"]
fn real_polymer_program_validates_fixture_proof_and_our_checks_run() {
    let so_path = std::env::var("POLYMER_PROVER_SO").expect("POLYMER_PROVER_SO not set");
    let program = std::fs::read(&so_path).expect("read POLYMER_PROVER_SO");

    let mut ctx = common::Context::default();
    // Replace the mock with the real binary at the same ID.
    ctx.add_program(polymer::POLYMER_PROVER_ID, &program)
        .unwrap();

    // Seed Polymer's `["internal"]` account with the fixture's parameters.
    let mut internal_data = polymer::INTERNAL_ACCOUNT_DISCRIMINATOR.to_vec();
    InternalAccount {
        authority: Pubkey::new_unique(),
        client_type: FIXTURE_CLIENT_TYPE.to_string(),
        signer_addr: FIXTURE_SIGNER,
        peptide_chain_id: FIXTURE_PEPTIDE_CHAIN_ID,
    }
    .serialize(&mut internal_data)
    .unwrap();
    internal_data.resize(8 + 32 + 4 + 32 + 20 + 8, 0); // INIT_SPACE with max_len(32) client_type
    let lamports = ctx
        .get_sysvar::<Rent>()
        .minimum_balance(internal_data.len());
    ctx.set_account(
        polymer::internal_pda().0,
        Account {
            lamports,
            data: internal_data,
            owner: polymer::POLYMER_PROVER_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    ctx.polymer_prover()
        .init(
            vec![evm_address_to_bytes32(FIXTURE_EMITTER)],
            Config::pda().0,
        )
        .unwrap();

    let authority = Keypair::new();
    ctx.airdrop(&authority.pubkey(), common::sol_amount(5.0))
        .unwrap();

    // Real `create_accounts`: [authority, cache, result, system_program].
    send(
        &mut ctx,
        &authority,
        Instruction {
            program_id: polymer::POLYMER_PROVER_ID,
            accounts: vec![
                AccountMeta::new(authority.pubkey(), true),
                AccountMeta::new(polymer::cache_pda(&authority.pubkey()).0, false),
                AccountMeta::new(polymer::result_pda(&authority.pubkey()).0, false),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            ],
            data: polymer::CREATE_ACCOUNTS_DISCRIMINATOR.to_vec(),
        },
    )
    .unwrap();

    // Real `load_proof(chunk)` in 800-byte chunks: [authority, cache].
    for chunk in fixture_proof().chunks(800) {
        let mut data = polymer::LOAD_PROOF_DISCRIMINATOR.to_vec();
        chunk.to_vec().serialize(&mut data).unwrap();
        send(
            &mut ctx,
            &authority,
            Instruction {
                program_id: polymer::POLYMER_PROVER_ID,
                accounts: vec![
                    AccountMeta::new(authority.pubkey(), true),
                    AccountMeta::new(polymer::cache_pda(&authority.pubkey()).0, false),
                ],
                data,
            },
        )
        .unwrap();
    }

    // Our `validate`: Polymer accepts the proof (is_valid), the emitter is
    // whitelisted, then our topic-count check rejects the four-topic event.
    let result = ctx.polymer_prover().validate(&authority, vec![]);

    // Polymer's frame must have returned Ok: its success log is what separates
    // "our check rejected the four-topic event" from "Polymer itself happened
    // to return the same custom error code".
    assert!(
        result
            .clone()
            .is_err_and(common::program_succeeded(polymer::POLYMER_PROVER_ID)),
        "polymer program did not succeed: {result:?}"
    );
    assert!(
        result.clone().is_err_and(common::is_program_error(
            polymer_prover::ID,
            PolymerProverError::InvalidTopicsLength
        )),
        "{result:?}"
    );
}
