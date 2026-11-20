//! Real venues for the split fulfillment path: mainnet program binaries, the
//! venue state they read, and the client-shaped transaction the venue tests in
//! `prove_and_withdraw.rs` measure. Provenance, licenses and refresh commands:
//! `tests/fixtures/split-venues/README.md`.

use std::path::{Path, PathBuf};

use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent;
use anchor_spl::token::spl_token;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use eco_svm_std::event_authority_pda;
use flash_fulfiller::state::prove_authority_pda;
use portal::state::executor_pda;
use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, extend_lookup_table,
};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signer::Signer;
use solana_sdk::slot_hashes::SlotHashes;
use solana_sdk::transaction::{Transaction, VersionedTransaction};

use super::Context;

/// `block_time` of the fixture snapshot's last slot. The venues read the clock
/// (stabble's amplification ramp, Meteora's locked-profit decay, Helium's
/// breaker window and freeze time), so the tests run at the snapshot's time.
pub const VENUE_SNAPSHOT_UNIX_TIMESTAMP: i64 = 1_795_115_563;
/// The compute-unit limit the measured transaction requests.
pub const SPLIT_VENUE_COMPUTE_LIMIT: u32 = 600_000;
/// A mainnet client also sets a priority fee; its instruction is part of the
/// measured packet. The price itself does not change the size.
pub const SPLIT_VENUE_COMPUTE_UNIT_PRICE: u64 = 200_000;
/// Solana's per-transaction account lock limit.
pub const MAX_TX_ACCOUNT_LOCKS: usize = 64;
/// Names the directory holding the venue binaries that are not vendored.
pub const SPLIT_VENUE_PROGRAMS: &str = "SPLIT_VENUE_PROGRAMS";

/// Helium's `treasury-management` (Apache-2.0, helium/helium-program-library),
/// dumped from mainnet-beta at slot 471448476 (last deployed in slot 339861344).
pub const HELIUM_TREASURY_MANAGEMENT_BIN: &[u8] =
    include_bytes!("../../../bins/helium_treasury_management.so");
pub const HELIUM_TREASURY_MANAGEMENT_SHA256: &str =
    "b7569880426f864b9a23bb4efead42019c562fa38d68a67d2be12cbecddb4649";
/// Helium's `circuit-breaker` (Apache-2.0), dumped with the program above (last
/// deployed in slot 339861292).
pub const HELIUM_CIRCUIT_BREAKER_BIN: &[u8] =
    include_bytes!("../../../bins/helium_circuit_breaker.so");
pub const HELIUM_CIRCUIT_BREAKER_SHA256: &str =
    "e07a3891c3ee5394810941db5ce56e480eb545a882e6a8b450f074c0c16e50c3";

pub const HELIUM_TREASURY_MANAGEMENT_ID: Pubkey =
    solana_sdk::pubkey!("treaf4wWBBty3fHdyBpo35Mz84M8k3heKXmjmi9vFt5");
pub const HELIUM_CIRCUIT_BREAKER_ID: Pubkey =
    solana_sdk::pubkey!("circAbx64bbsscPbQzZAUvuXpHqrCe6fLMzc2uKXz9g");
pub const STABBLE_STABLE_SWAP_ID: Pubkey =
    solana_sdk::pubkey!("swapNyd8XiQwJ6ianp9snpu4brUqFxadzvHebnAXjJZ");
pub const STABBLE_VAULT_ID: Pubkey =
    solana_sdk::pubkey!("vo1tWgqZMjG61Z2T9qUaMYKqZ75CYzMuaZ2LZP1n7HV");
pub const METEORA_DAMM_V1_ID: Pubkey =
    solana_sdk::pubkey!("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");
pub const METEORA_VAULT_ID: Pubkey =
    solana_sdk::pubkey!("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi");

const USDC: Pubkey = solana_sdk::pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
const USDT: Pubkey = solana_sdk::pubkey!("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB");
const MOBILE: Pubkey = solana_sdk::pubkey!("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6");
const HNT: Pubkey = solana_sdk::pubkey!("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux");
/// The stabble vault's stored withdraw authority: a swap-program PDA that only
/// signs, so it has no account to load.
const STABBLE_WITHDRAW_AUTHORITY: Pubkey =
    solana_sdk::pubkey!("8BSWYgAczR36C7ukr32v7uTepoRhYJYxAVnpBtYniZTm");

/// `sha256("global:redeem_v0")[..8]`.
const HELIUM_REDEEM_V0: [u8; 8] = [0xeb, 0x7f, 0xab, 0x8b, 0x77, 0x4d, 0xeb, 0x76];
/// `sha256("global:swap")[..8]`, the `swap` of both stabble and Meteora DAMM v1.
const ANCHOR_SWAP: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];
/// `TreasuryManagementV0.curve`'s `k: u128`, after the 1-byte curve tag.
const HELIUM_CURVE_K: std::ops::Range<usize> = 137..153;

/// One swap through a real venue, executed by portal's executor and paid to a
/// recipient's ATA.
pub struct VenueRoute {
    /// The fixture directory, also the label in test output.
    pub name: &'static str,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    /// What the route funds the executor with and the venue spends.
    pub amount_in: u64,
    /// The venue's slippage floor, encoded in `instruction`.
    pub min_out: u64,
    /// The output the venue's published formula gives at the fixture state,
    /// when the test can compute it exactly.
    pub exact_out: Option<u64>,
    /// The program the venue CPIs between itself and SPL Token.
    pub sub_program: Pubkey,
    /// The venue instruction: the executor swaps and the recipient's ATA
    /// receives.
    pub instruction: Instruction,
    /// Venue programs and state, which any solver's table can hold.
    pub lookup_addresses: Vec<Pubkey>,
}

/// The measured transaction with its wire size and its account locks (static
/// keys plus every account loaded through the table).
pub struct MeasuredSplitTransaction {
    pub transaction: VersionedTransaction,
    pub wire_bytes: usize,
    pub account_locks: usize,
}

/// A classic SPL Token ATA; every venue mint here is classic.
pub fn spl_ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, &spl_token::ID)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    solana_sdk::hash::hash(bytes)
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The directory `SPLIT_VENUE_PROGRAMS` names.
pub fn venue_programs_dir() -> PathBuf {
    std::env::var(SPLIT_VENUE_PROGRAMS)
        .unwrap_or_else(|_| panic!("set {SPLIT_VENUE_PROGRAMS}"))
        .into()
}

/// What a solver can hold in one long-lived table before any given intent
/// exists: the programs, PDAs and mints the split pair touches, the venue's
/// programs and state, the engine stand-in, and the solver's own reward ATA.
/// Every intent-specific account, the recipient and the signers stay static.
pub fn split_venue_lookup_addresses(
    venue: &VenueRoute,
    engine: &Pubkey,
    solver: &Pubkey,
) -> Vec<Pubkey> {
    let executor = executor_pda().0;
    let mut addresses = vec![
        local_prover::ID,
        prove_authority_pda(&local_prover::ID).0,
        event_authority_pda(&local_prover::ID).0,
        solana_instructions_sysvar::ID,
        spl_token::ID,
        anchor_spl::token_2022::ID,
        anchor_lang::system_program::ID,
        executor,
        spl_ata(&executor, &venue.input_mint),
        venue.input_mint,
        venue.output_mint,
        *engine,
        spl_ata(solver, &venue.input_mint),
    ];
    addresses.extend(venue.lookup_addresses.iter().copied());
    addresses.sort();
    addresses.dedup();
    addresses
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/split-venues")
        .join(format!("{name}.json"))
}

impl Context {
    /// Loads a `solana account --output json` fixture at its recorded address.
    pub fn load_venue_fixture(&mut self, name: &str) -> Pubkey {
        let path = fixture_path(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let account = &json["account"];
        assert_eq!(account["data"][1], "base64", "{name}");
        assert_eq!(account["executable"], false, "{name}");
        let pubkey: Pubkey = json["pubkey"].as_str().unwrap().parse().unwrap();
        self.set_account(
            pubkey,
            solana_sdk::account::Account {
                lamports: account["lamports"].as_u64().unwrap(),
                data: STANDARD
                    .decode(account["data"][0].as_str().unwrap())
                    .unwrap(),
                owner: account["owner"].as_str().unwrap().parse().unwrap(),
                executable: false,
                rent_epoch: account["rentEpoch"].as_u64().unwrap(),
            },
        )
        .unwrap();
        pubkey
    }

    /// The real mints the venue routes trade.
    pub fn load_venue_mints(&mut self) {
        for mint in ["usdc", "usdt", "mobile", "hnt"] {
            self.load_venue_fixture(&format!("mints/{mint}"));
        }
    }

    /// Writes `owner`'s ATA of a dumped `mint` holding `amount`.
    /// `airdrop_token_ata` cannot: it mints with the test's mint authority.
    pub fn set_token_balance(&mut self, owner: &Pubkey, mint: &Pubkey, amount: u64) -> Pubkey {
        let mut data = [0u8; spl_token::state::Account::LEN];
        spl_token::state::Account::pack(
            spl_token::state::Account {
                mint: *mint,
                owner: *owner,
                amount,
                state: spl_token::state::AccountState::Initialized,
                ..Default::default()
            },
            &mut data,
        )
        .unwrap();
        let address = spl_ata(owner, mint);
        let lamports = self.get_sysvar::<Rent>().minimum_balance(data.len());
        self.set_account(
            address,
            solana_sdk::account::Account {
                lamports,
                data: data.to_vec(),
                owner: spl_token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        address
    }

    fn load_dumped_program(&mut self, dir: &Path, name: &str, id: Pubkey) {
        let path = dir.join(format!("{name}.so"));
        let binary =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        println!("{name}.so sha256 {}", sha256_hex(&binary));
        self.add_program(id, &binary).unwrap();
    }

    /// MOBILE → HNT through Helium's `redeem_v0`: treasury-management burns
    /// MOBILE and CPIs circuit-breaker, which CPIs SPL Token.
    pub fn helium_treasury_route(&mut self, recipient: &Pubkey) -> VenueRoute {
        self.add_program(
            HELIUM_TREASURY_MANAGEMENT_ID,
            HELIUM_TREASURY_MANAGEMENT_BIN,
        )
        .unwrap();
        self.add_program(HELIUM_CIRCUIT_BREAKER_ID, HELIUM_CIRCUIT_BREAKER_BIN)
            .unwrap();
        let management = self.load_venue_fixture("helium-treasury/treasury-management");
        let treasury = self.load_venue_fixture("helium-treasury/treasury");
        let breaker = self.load_venue_fixture("helium-treasury/circuit-breaker");
        let amount_in = 1_000_000_000; // 1,000 MOBILE

        // With k = 0, `redeem_v0` pays treasury · burned / supply, rounded down.
        let curve = &self.get_account(&management).unwrap().data[HELIUM_CURVE_K];
        assert!(
            curve.iter().all(|byte| *byte == 0),
            "exact output assumes k = 0"
        );
        let treasury_amount =
            spl_token::state::Account::unpack(&self.get_account(&treasury).unwrap().data)
                .unwrap()
                .amount;
        let supply = spl_token::state::Mint::unpack(&self.get_account(&MOBILE).unwrap().data)
            .unwrap()
            .supply;
        let exact_out =
            u64::try_from(u128::from(treasury_amount) * u128::from(amount_in) / u128::from(supply))
                .unwrap();

        let executor = executor_pda().0;
        VenueRoute {
            name: "helium-treasury",
            input_mint: MOBILE,
            output_mint: HNT,
            amount_in,
            min_out: exact_out,
            exact_out: Some(exact_out),
            sub_program: HELIUM_CIRCUIT_BREAKER_ID,
            instruction: Instruction {
                program_id: HELIUM_TREASURY_MANAGEMENT_ID,
                accounts: vec![
                    AccountMeta::new_readonly(management, false),
                    AccountMeta::new_readonly(HNT, false),
                    AccountMeta::new(MOBILE, false),
                    AccountMeta::new(treasury, false),
                    AccountMeta::new(breaker, false),
                    AccountMeta::new(spl_ata(&executor, &MOBILE), false),
                    AccountMeta::new(spl_ata(recipient, &HNT), false),
                    AccountMeta::new_readonly(executor, true),
                    AccountMeta::new_readonly(HELIUM_CIRCUIT_BREAKER_ID, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                ],
                data: [
                    &HELIUM_REDEEM_V0[..],
                    &amount_in.to_le_bytes(),
                    &exact_out.to_le_bytes(),
                ]
                .concat(),
            },
            lookup_addresses: vec![
                HELIUM_TREASURY_MANAGEMENT_ID,
                HELIUM_CIRCUIT_BREAKER_ID,
                management,
                treasury,
                breaker,
            ],
        }
    }

    /// USDC → USDT through stabble's stable-swap `swap`: the pool pulls USDC
    /// itself and CPIs the vault program, which CPIs SPL Token to pay out.
    pub fn stabble_stable_swap_route(&mut self, programs: &Path, recipient: &Pubkey) -> VenueRoute {
        self.load_dumped_program(programs, "stabble_stable_swap", STABBLE_STABLE_SWAP_ID);
        self.load_dumped_program(programs, "stabble_vault", STABBLE_VAULT_ID);
        let pool = self.load_venue_fixture("stabble-stable-swap/pool");
        let vault = self.load_venue_fixture("stabble-stable-swap/vault");
        let vault_authority = self.load_venue_fixture("stabble-stable-swap/vault-authority");
        let vault_usdc = self.load_venue_fixture("stabble-stable-swap/vault-token-usdc");
        let vault_usdt = self.load_venue_fixture("stabble-stable-swap/vault-token-usdt");
        let beneficiary_usdt =
            self.load_venue_fixture("stabble-stable-swap/beneficiary-token-usdt");
        let amount_in: u64 = 1_000_000; // 1 USDC
        let min_out: u64 = 990_000;

        let executor = executor_pda().0;
        VenueRoute {
            name: "stabble-stable-swap",
            input_mint: USDC,
            output_mint: USDT,
            amount_in,
            min_out,
            exact_out: None,
            sub_program: STABBLE_VAULT_ID,
            instruction: Instruction {
                program_id: STABBLE_STABLE_SWAP_ID,
                accounts: vec![
                    AccountMeta::new_readonly(executor, true),
                    AccountMeta::new(spl_ata(&executor, &USDC), false),
                    AccountMeta::new(spl_ata(recipient, &USDT), false),
                    AccountMeta::new(vault_usdc, false),
                    AccountMeta::new(vault_usdt, false),
                    AccountMeta::new(beneficiary_usdt, false),
                    AccountMeta::new(pool, false),
                    AccountMeta::new_readonly(STABBLE_WITHDRAW_AUTHORITY, false),
                    AccountMeta::new_readonly(vault, false),
                    AccountMeta::new_readonly(vault_authority, false),
                    AccountMeta::new_readonly(STABBLE_VAULT_ID, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                ],
                // `swap(amount_in: Option<u64>, minimum_amount_out: u64)`.
                data: [
                    &ANCHOR_SWAP[..],
                    &[1],
                    &amount_in.to_le_bytes(),
                    &min_out.to_le_bytes(),
                ]
                .concat(),
            },
            lookup_addresses: vec![
                STABBLE_STABLE_SWAP_ID,
                STABBLE_VAULT_ID,
                pool,
                vault,
                vault_authority,
                vault_usdc,
                vault_usdt,
                beneficiary_usdt,
                STABBLE_WITHDRAW_AUTHORITY,
            ],
        }
    }

    /// USDC → USDT through Meteora DAMM v1's stable pool: the pool CPIs the
    /// dynamic-vault program twice (deposit, then withdraw), each into SPL Token.
    pub fn meteora_damm_v1_stable_route(
        &mut self,
        programs: &Path,
        recipient: &Pubkey,
    ) -> VenueRoute {
        self.load_dumped_program(programs, "meteora_damm_v1", METEORA_DAMM_V1_ID);
        self.load_dumped_program(programs, "meteora_vault", METEORA_VAULT_ID);
        let pool = self.load_venue_fixture("meteora-damm-v1-stable/pool");
        let a_vault = self.load_venue_fixture("meteora-damm-v1-stable/a-vault");
        let b_vault = self.load_venue_fixture("meteora-damm-v1-stable/b-vault");
        let a_token_vault = self.load_venue_fixture("meteora-damm-v1-stable/a-token-vault");
        let b_token_vault = self.load_venue_fixture("meteora-damm-v1-stable/b-token-vault");
        let a_lp_mint = self.load_venue_fixture("meteora-damm-v1-stable/a-lp-mint");
        let b_lp_mint = self.load_venue_fixture("meteora-damm-v1-stable/b-lp-mint");
        let a_vault_lp = self.load_venue_fixture("meteora-damm-v1-stable/a-vault-lp");
        let b_vault_lp = self.load_venue_fixture("meteora-damm-v1-stable/b-vault-lp");
        let protocol_fee = self.load_venue_fixture("meteora-damm-v1-stable/protocol-token-a-fee");
        let amount_in: u64 = 1_000_000; // 1 USDC
        let min_out: u64 = 990_000;

        let executor = executor_pda().0;
        VenueRoute {
            name: "meteora-damm-v1-stable",
            input_mint: USDC,
            output_mint: USDT,
            amount_in,
            min_out,
            exact_out: None,
            sub_program: METEORA_VAULT_ID,
            instruction: Instruction {
                program_id: METEORA_DAMM_V1_ID,
                accounts: vec![
                    AccountMeta::new(pool, false),
                    AccountMeta::new(spl_ata(&executor, &USDC), false),
                    AccountMeta::new(spl_ata(recipient, &USDT), false),
                    AccountMeta::new(a_vault, false),
                    AccountMeta::new(b_vault, false),
                    AccountMeta::new(a_token_vault, false),
                    AccountMeta::new(b_token_vault, false),
                    AccountMeta::new(a_lp_mint, false),
                    AccountMeta::new(b_lp_mint, false),
                    AccountMeta::new(a_vault_lp, false),
                    AccountMeta::new(b_vault_lp, false),
                    AccountMeta::new(protocol_fee, false),
                    AccountMeta::new_readonly(executor, true),
                    AccountMeta::new_readonly(METEORA_VAULT_ID, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                ],
                // `swap(in_amount: u64, minimum_out_amount: u64)`.
                data: [
                    &ANCHOR_SWAP[..],
                    &amount_in.to_le_bytes(),
                    &min_out.to_le_bytes(),
                ]
                .concat(),
            },
            lookup_addresses: vec![
                METEORA_DAMM_V1_ID,
                METEORA_VAULT_ID,
                pool,
                a_vault,
                b_vault,
                a_token_vault,
                b_token_vault,
                a_lp_mint,
                b_lp_mint,
                a_vault_lp,
                b_vault_lp,
                protocol_fee,
            ],
        }
    }

    /// Creates `addresses` as a lookup table through the lookup-table program in
    /// its own transaction, before the measured one; `send_transaction`
    /// advances the slot, which activates the new entries.
    pub fn create_venue_lookup_table(
        &mut self,
        addresses: Vec<Pubkey>,
    ) -> AddressLookupTableAccount {
        let payer = self.payer.pubkey();
        let slot = self.get_sysvar::<SlotHashes>().first().unwrap().0;
        let (create, key) = create_lookup_table(payer, payer, slot);
        let extend = extend_lookup_table(key, payer, Some(payer), addresses.clone());
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[create, extend], Some(&payer)),
            self.latest_blockhash(),
        );
        self.send_transaction(transaction).unwrap();
        AddressLookupTableAccount { key, addresses }
    }

    /// The client's split transaction: compute limit and price, the heap frame
    /// every flash-fulfiller instruction needs, idempotent creation of the
    /// solver's reward ATA and the recipient's output ATA, then `settlement`
    /// (the pair, and anything a client would append). A v0 message against
    /// `table`, signed by the payer and the solver.
    pub fn split_venue_transaction(
        &self,
        venue: &VenueRoute,
        recipient: &Pubkey,
        settlement: &[Instruction],
        table: AddressLookupTableAccount,
    ) -> MeasuredSplitTransaction {
        let payer = self.payer.pubkey();
        let instructions: Vec<_> = [
            ComputeBudgetInstruction::set_compute_unit_limit(SPLIT_VENUE_COMPUTE_LIMIT),
            ComputeBudgetInstruction::set_compute_unit_price(SPLIT_VENUE_COMPUTE_UNIT_PRICE),
            ComputeBudgetInstruction::request_heap_frame(256 * 1024),
            create_associated_token_account_idempotent(
                &payer,
                &self.solver.pubkey(),
                &venue.input_mint,
                &spl_token::ID,
            ),
            create_associated_token_account_idempotent(
                &payer,
                recipient,
                &venue.output_mint,
                &spl_token::ID,
            ),
        ]
        .into_iter()
        .chain(settlement.iter().cloned())
        .collect();
        let message =
            v0::Message::try_compile(&payer, &instructions, &[table], self.latest_blockhash())
                .unwrap();
        let account_locks = message.account_keys.len()
            + message
                .address_table_lookups
                .iter()
                .map(|lookup| lookup.writable_indexes.len() + lookup.readonly_indexes.len())
                .sum::<usize>();
        let transaction = VersionedTransaction::try_new(
            VersionedMessage::V0(message),
            &[&self.payer, &self.solver],
        )
        .unwrap();
        let wire_bytes = bincode::serialize(&transaction).unwrap().len();
        MeasuredSplitTransaction {
            transaction,
            wire_bytes,
            account_locks,
        }
    }
}
