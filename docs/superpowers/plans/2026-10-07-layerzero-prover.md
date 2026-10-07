# LayerZero Prover for Solana Implementation Plan

> **Historical:** written against the pre-#102 prover interface (portal `withdraw` CPIs `close_proof`, `proof_closer_pda(prover)`, aggregator `aggregate`). After stacking on eco-routes-svm #102 the program implements `get_proof` / `close_proof(CloseProofArgs)` with intent-hash-keyed closers; see `docs/prover-interface.md` and the design spec for the current behaviour.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `programs/layerzero-prover`, a LayerZero V2-backed Solana prover that proves Solana-fulfilled intents to EVM (record-then-send) and turns EVM-sent LayerZero messages into standard `Proof` PDAs (`lz_receive`), plus a localnet mock endpoint and the full test suite.

**Architecture:** `portal::prove` CPIs `prove`, which commits the batch to a content-addressed `PendingSend` PDA; a permissionless top-level `send_message` CPIs `endpoint::send` (sending inside `portal::prove` would exceed Solana's 5-frame invoke stack). Inbound, LayerZero's executor calls `lz_receive_types_info` → `lz_receive_types_v2` → `lz_receive`; `lz_receive` CPIs `endpoint::clear` first, then checks peer and chain, then creates `Proof` PDAs funded by the program's `pda_payer` reserve (which is also the OApp's LayerZero delegate). Setup instructions are gated on the program's upgrade authority, so finalizing the program revokes all admin.

**Tech Stack:** Rust 1.97.1 host / platform-tools v1.52 (rustc 1.89.0-dev on-chain), Anchor 1.1.2, `eco-svm-std`, `tiny-keccak`, litesvm 0.16 integration tests, goldie 0.7 snapshots.

**Spec:** `docs/superpowers/specs/2026-10-06-layerzero-prover-design.md` (read it; this plan argues from it). LayerZero interface reference used for every mirrored ID, seed, discriminator and account list: LayerZero-Labs/LayerZero-v2 @ `9c741e7f9790639537b1710a203bcdfd73b0b9ac`, `packages/layerzero-v2/solana/programs/{endpoint,uln}` and `libs/oapp`.

**Scope:** this plan delivers the Solana program, the mock, tests, build/release wiring and docs. Out of scope (separate plans after this merges): EVM `LayerZeroProver` fleet deployment, new aggregator deployments, eco-solver integration, devnet E2E, mainnet rollout (spec §5, §7).

## Global Constraints

- Anchor 1.1.2; every program crate pins `[package.metadata.solana] tools-version = "v1.52"`. Program code must compile under platform-tools rustc 1.89.0-dev — no `is_multiple_of` or other post-1.89 std methods.
- No LayerZero crate dependency. Hand-mirror IDs, seeds, discriminators, param structs in `programs/layerzero-prover/src/layerzero.rs`.
- LayerZero pinned IDs: Endpoint `76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6`, ULN302 `7a4WjyR8VZ7yZz5XJAKm39BUGn5iT9CKcv2pmG9tdXVH` (same on mainnet and devnet). Solana EIDs (EVM-side config only, not used on-chain): `30168` mainnet, `40168` devnet.
- Every u32/u64 PDA seed in LayerZero programs is **big-endian**. ULN seeds put the eid **before** the OApp; endpoint seeds put the OApp first.
- The send/receive library is addressed by the ULN's **`MessageLib` PDA** (`find_program_address([b"MessageLib"], ULN)`), never the ULN program ID.
- Executor options: type-3, executor worker 1, `lzReceive` option 1, gas only: `0x0003 | 01 | 0x0011 | 01 | gas u128 BE`. Gas floor `MIN_GAS_LIMIT (200_000) + n * GAS_PER_INTENT (50_000)` — identical to EVM `LayerZeroProver`.
- Outbound payload = `ProofData::to_bytes()` (8-byte `CHAIN_ID` BE header ‖ 64-byte pairs). Claimants pass through unchanged, including `eco_svm_std::CANCELLED`.
- Prover-scoped boundaries: `prove` accepts only `portal::state::dispatcher_pda(&crate::ID)`; `close_proof` only `portal::state::proof_closer_pda(&crate::ID)`.
- Account creation uses `eco_svm_std::account::AccountExt::init` / `create_account` (griefing-resistant), not Anchor `init`.
- `close_proof` must never fail for a valid closer (portal `refund` CPIs it on proven cancellations).
- Naming deviation from spec (Rust `Send` trait collision): spec `send` → instruction `send_message` (accounts `SendMessage`); spec `quote_send` → `quote_message` (accounts `QuoteMessage`).
- `mock-layerzero-endpoint` is localnet-only: never add it to `build-devnet` / `build-mainnet` / `release.yml` enumerations.
- Lint/format gates: `cargo clippy --all-targets -- -D warnings` (no `--all-features`), `cargo +nightly fmt`, `cargo sort --workspace --check`.
- Builds before tests: tests `include_bytes!` from `target/deploy/`, so run `anchor build` before `cargo test` whenever program code changed.
- Commit messages: conventional commits, no co-author lines; end each with `Claude-Session: https://claude.ai/code/session_01FMticFmtqpK9CEPCeHw6F9`.

## Review Focus

1. **Duplicate intent hash inside one inbound batch** — identical duplicate must succeed (second is a no-op, event repeats); a conflicting duplicate must fail `IntentAlreadyProven`. Test: Task 7 `duplicate_pair_in_one_message_is_idempotent_and_conflict_fails`.
2. **Underfunded `pda_payer` at delivery** — `lz_receive` must fail without consuming the message (PayloadHash survives) and succeed on retry after a top-up. Test: Task 7 `underfunded_pda_payer_fails_then_retry_succeeds`.
3. **Endpoint path that exists for a non-peer sender** (misconfigured or forged nonce) — `lz_receive` must still reject on our own peer check. Test: Task 7 `non_peer_sender_rejected_even_if_endpoint_path_exists`.
4. **`send_message` replayed after success** — second call must fail (commit closed), not double-charge. Test: Task 5 `second_send_fails_once_commit_closed`.
5. **`domain_id` above `u32::MAX` from portal** — must fail `InvalidDomainId`, not truncate onto another EID. Test: Task 4 `domain_id_above_u32_rejected`.

---

## File Structure

```
programs/layerzero-prover/
  Cargo.toml
  src/lib.rs                       # declare_id!, #[program] dispatch
  src/constants.rs                 # MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_MESSAGE, gas floor
  src/layerzero.rs                 # mirrored LZ IDs, seeds, PDAs, discriminators, params, options, invoke helper
  src/state.rs                     # Store, Peer, LzReceiveTypesAccount, PendingSend, ProofAccount, pda_payer
  src/instructions/mod.rs          # module wiring + LayerZeroProverError
  src/instructions/{close_proof,init,init_path,set_path_config,set_alt,prove,send_message,quote_message,lz_receive_types,lz_receive}.rs
programs/mock-layerzero-endpoint/
  Cargo.toml
  src/lib.rs                       # localnet stand-in at the endpoint ID
integration-tests/tests/common/layerzero_prover_context.rs   # builders/helpers
integration-tests/tests/{close_proof,init,prove,send,lz_receive_types,lz_receive}_layerzero_prover.rs
integration-tests/tests/layerzero_prover_batch_limits.rs
integration-tests/tests/layerzero_prover_real.rs
Modified: Cargo.toml (workspace deps), Anchor.toml, integration-tests/Cargo.toml,
  integration-tests/tests/common/mod.rs, .github/workflows/release.yml,
  scripts/bump-cargo-versions.sh, CLAUDE.md, README.md
```

---

### Task 1: Crate scaffold, LayerZero mirror, state, `close_proof`

**Files:**
- Create: `programs/layerzero-prover/Cargo.toml`, `src/lib.rs`, `src/constants.rs`, `src/layerzero.rs`, `src/state.rs`, `src/instructions/mod.rs`, `src/instructions/close_proof.rs`
- Modify: `Cargo.toml` (workspace deps), `Anchor.toml` (`[programs.localnet]`)

**Interfaces:**
- Produces (used by every later task):
  - `layerzero_prover::layerzero::{ENDPOINT_ID, ULN_ID}`; PDA fns `endpoint_settings_pda()`, `oapp_registry_pda(&Pubkey)`, `nonce_pda(&Pubkey, u32, &[u8;32])`, `pending_nonce_pda(&Pubkey, u32, &[u8;32])`, `payload_hash_pda(&Pubkey, u32, &[u8;32], u64)`, `send_library_config_pda(&Pubkey, u32)`, `default_send_library_config_pda(u32)`, `receive_library_config_pda(&Pubkey, u32)`, `message_lib_info_pda(&Pubkey)`, `endpoint_event_authority()`, `uln_settings_pda()`, `uln_send_config_pda(u32, &Pubkey)`, `uln_receive_config_pda(u32, &Pubkey)`, `uln_default_send_config_pda(u32)`, `uln_default_receive_config_pda(u32)`, `uln_event_authority()` — all return `(Pubkey, u8)`.
  - Discriminator consts `REGISTER_OAPP_DISCRIMINATOR`, `INIT_NONCE_DISCRIMINATOR`, `INIT_SEND_LIBRARY_DISCRIMINATOR`, `INIT_RECEIVE_LIBRARY_DISCRIMINATOR`, `SET_SEND_LIBRARY_DISCRIMINATOR`, `SET_RECEIVE_LIBRARY_DISCRIMINATOR`, `INIT_CONFIG_DISCRIMINATOR`, `SET_CONFIG_DISCRIMINATOR`, `CLEAR_DISCRIMINATOR`, `SEND_DISCRIMINATOR`, `QUOTE_DISCRIMINATOR`, `LZ_RECEIVE_DISCRIMINATOR: [u8; 8]`.
  - Param structs `RegisterOAppParams, InitNonceParams, InitSendLibraryParams, InitReceiveLibraryParams, SetSendLibraryParams, SetReceiveLibraryParams, InitConfigParams, SetConfigParams, ClearParams, SendParams, QuoteParams, MessagingFee, LzReceiveParams, UlnConfig, ExecutorConfig`; V2 executor types `LzReceiveTypesV2Accounts, LzReceiveTypesInfoResult, AddressLocator, AccountMetaRef, LzInstruction, LzReceiveTypesV2Result`; consts `CONFIG_TYPE_EXECUTOR=1, CONFIG_TYPE_SEND_ULN=2, CONFIG_TYPE_RECEIVE_ULN=3, NIL_DVN_COUNT=255, LZ_RECEIVE_TYPES_VERSION=2, EXECUTION_CONTEXT_VERSION_1=1`.
  - `layerzero::lz_receive_options(gas: u128) -> Vec<u8>`; `layerzero::invoke(program_id: Pubkey, discriminator: [u8; 8], params: &impl AnchorSerialize, accounts: &[AccountInfo<'info>], signers: &[Pubkey], signer_seeds: &[&[&[u8]]]) -> Result<()>` (`accounts[0]` is the callee program).
  - `constants::{MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_MESSAGE, MIN_GAS_LIMIT, GAS_PER_INTENT, lz_receive_gas(usize) -> u128}`.
  - `state::{Peer, Store, LzReceiveTypesAccount, PendingSend, ProofAccount, pda_payer_pda(), STORE_SEED, PDA_PAYER_SEED, PENDING_SEND_SEED, MAX_PEERS}`; `Store::new(Vec<Peer>) -> Result<Store>`, `Store::pda()`, `Store::peer(u32) -> Option<&Peer>`, `LzReceiveTypesAccount::pda()`, `PendingSend::key(u32, &Bytes32, &[u8]) -> [u8; 32]`, `PendingSend::pda(u32, &Bytes32, &[u8])`, `PendingSend::intent_count(&self) -> usize`.
  - `instructions::LayerZeroProverError` (full enum, below); instruction `close_proof`.

- [ ] **Step 1: Grind the program keypair**

```bash
cd target/deploy 2>/dev/null || mkdir -p target/deploy && cd target/deploy
solana-keygen grind --starts-with EcoZ:1 --ignore-case=false
mv EcoZ*.json layerzero_prover-keypair.json
solana address -k layerzero_prover-keypair.json
cd ../..
```
Record the printed address as `<PROGRAM_ID>`; it replaces `<PROGRAM_ID>` in every step below. (`target/` is git-ignored; the rollout plan regrinds or reuses this key per deploy conventions.)

- [ ] **Step 2: Create `programs/layerzero-prover/Cargo.toml`**

```toml
[package]
description = "LayerZero V2-backed prover for eco-routes-svm"
edition = "2021"
name = "layerzero-prover"
version = "0.1.0"

# Pins the platform-tools for a plain `cargo build-sbf` (solana-verify); keep in step with
# the v1.52 anchor-cli 1.1.2 hard-codes and CI asserts (SBF_TOOLS_VERSION).
[package.metadata.solana]
tools-version = "v1.52"

[lib]
crate-type = ["cdylib", "lib"]
name = "layerzero_prover"

[features]
cpi = ["no-entrypoint"]
default = []
idl-build = ["anchor-lang/idl-build"]
mainnet = ["eco-svm-std/mainnet", "portal/mainnet"]
no-entrypoint = []
no-idl = []
no-log-ix-name = []

[dependencies]
anchor-lang = { workspace = true, features = ["event-cpi"] }
eco-svm-std = { workspace = true }
portal = { workspace = true, features = ["no-entrypoint"] }
tiny-keccak = { workspace = true }

# The k256 → getrandom path (via anchor's solana-secp256k1-recover) needs a
# getrandom backend for the SBF target; enable the custom backend there only.
[target.'cfg(target_os = "solana")'.dependencies]
getrandom = { workspace = true }

[dev-dependencies]
goldie = { workspace = true }
```

In root `Cargo.toml` `[workspace.dependencies]` add (keep alphabetical; `cargo sort` checks):

```toml
layerzero-prover = { path = "programs/layerzero-prover" }
```

In `Anchor.toml` `[programs.localnet]` add `layerzero-prover = "<PROGRAM_ID>"`.

- [ ] **Step 3: Write `src/constants.rs`**

```rust
/// Largest batch `prove` accepts. Chosen so `[ComputeBudget, portal::prove,
/// send_message]` fits one 1232-byte v0 transaction with the FulfillMarkers
/// and LayerZero accounts in address lookup tables; pinned by
/// `layerzero_prover_batch_limits::outbound_ceiling_matches_max_intents_per_prove`.
pub const MAX_INTENTS_PER_PROVE: usize = 16;

/// Largest inbound batch the executor's delivery transaction can carry: every
/// pair needs its brand-new `Proof` PDA as a static key (a lookup table cannot
/// hold an address nobody pre-registered) plus 64 message bytes. The EVM
/// `LayerZeroProver` cannot enforce it, so the solver must batch EVM
/// `Inbox.prove` calls toward Solana at or below this. Pinned by
/// `layerzero_prover_batch_limits::inbound_ceiling_matches_max_pairs_per_message`.
pub const MAX_PAIRS_PER_MESSAGE: usize = 8;

/// Same floor the EVM `LayerZeroProver` computes, so a permissionless
/// `send_message` caller cannot under-gas the EVM `lzReceive`.
pub const MIN_GAS_LIMIT: u128 = 200_000;
pub const GAS_PER_INTENT: u128 = 50_000;

pub fn lz_receive_gas(intents: usize) -> u128 {
    MIN_GAS_LIMIT + intents as u128 * GAS_PER_INTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz_receive_gas_matches_evm_floor() {
        assert_eq!(lz_receive_gas(0), 200_000);
        assert_eq!(lz_receive_gas(2), 300_000);
        assert_eq!(lz_receive_gas(MAX_INTENTS_PER_PROVE), 1_000_000);
    }
}
```

- [ ] **Step 4: Write `src/layerzero.rs`**

```rust
//! Hand-rolled mirror of the pieces of LayerZero V2's Solana programs this
//! prover consumes: the Endpoint (OApp registration, path setup, `clear`,
//! `send`, `quote`), ULN302's config PDAs, and the executor's V2
//! `lz_receive_types` ABI. Kept as constants rather than a crate dependency so
//! LayerZero's Anchor 0.29 pin stays out of our build graph, the same way
//! `hyperlane.rs` and `polymer.rs` mirror their bridges.
//!
//! Pinned to LayerZero-Labs/LayerZero-v2@9c741e7f9790639537b1710a203bcdfd73b0b9ac
//! (`packages/layerzero-v2/solana/programs/{endpoint,uln}`, `libs/oapp`).
//! Every u32/u64 seed is big-endian. Discriminators are
//! `sha256("global:<name>")[..8]`, pinned against Anchor-derived values in
//! `integration-tests/tests/close_proof_layerzero_prover.rs`.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::event_authority_pda;

use crate::instructions::LayerZeroProverError;

pub const ENDPOINT_ID: Pubkey = pubkey!("76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6");
pub const ULN_ID: Pubkey = pubkey!("7a4WjyR8VZ7yZz5XJAKm39BUGn5iT9CKcv2pmG9tdXVH");

/// Solana's LayerZero EIDs, for EVM-side configuration and tests; the program
/// never needs its own EID on-chain.
pub const MAINNET_SOLANA_EID: u32 = 30168;
pub const DEVNET_SOLANA_EID: u32 = 40168;

pub const ENDPOINT_SEED: &[u8] = b"Endpoint";
pub const OAPP_SEED: &[u8] = b"OApp";
pub const NONCE_SEED: &[u8] = b"Nonce";
pub const PENDING_NONCE_SEED: &[u8] = b"PendingNonce";
pub const PAYLOAD_HASH_SEED: &[u8] = b"PayloadHash";
pub const SEND_LIBRARY_CONFIG_SEED: &[u8] = b"SendLibraryConfig";
pub const RECEIVE_LIBRARY_CONFIG_SEED: &[u8] = b"ReceiveLibraryConfig";
pub const MESSAGE_LIB_SEED: &[u8] = b"MessageLib";
pub const SEND_CONFIG_SEED: &[u8] = b"SendConfig";
pub const RECEIVE_CONFIG_SEED: &[u8] = b"ReceiveConfig";
/// The executor derives `[LZ_RECEIVE_TYPES_SEED, store]` under the OApp
/// program itself; it must not change.
pub const LZ_RECEIVE_TYPES_SEED: &[u8] = b"LzReceiveTypes";

pub const REGISTER_OAPP_DISCRIMINATOR: [u8; 8] = [129, 89, 71, 68, 11, 82, 210, 125];
pub const INIT_NONCE_DISCRIMINATOR: [u8; 8] = [204, 171, 16, 214, 182, 191, 27, 196];
pub const INIT_SEND_LIBRARY_DISCRIMINATOR: [u8; 8] = [156, 24, 235, 120, 73, 193, 144, 19];
pub const INIT_RECEIVE_LIBRARY_DISCRIMINATOR: [u8; 8] = [197, 114, 81, 100, 45, 233, 36, 230];
pub const SET_SEND_LIBRARY_DISCRIMINATOR: [u8; 8] = [251, 118, 78, 158, 134, 149, 129, 5];
pub const SET_RECEIVE_LIBRARY_DISCRIMINATOR: [u8; 8] = [223, 172, 180, 105, 165, 161, 147, 228];
pub const INIT_CONFIG_DISCRIMINATOR: [u8; 8] = [23, 235, 115, 232, 168, 96, 1, 231];
pub const SET_CONFIG_DISCRIMINATOR: [u8; 8] = [108, 158, 154, 175, 212, 98, 52, 66];
pub const CLEAR_DISCRIMINATOR: [u8; 8] = [250, 39, 28, 213, 123, 163, 133, 5];
pub const SEND_DISCRIMINATOR: [u8; 8] = [102, 251, 20, 187, 65, 75, 12, 69];
pub const QUOTE_DISCRIMINATOR: [u8; 8] = [149, 42, 109, 247, 134, 146, 213, 123];
/// Hard-coded in LayerZero's executor; our `lz_receive` must hash to it.
pub const LZ_RECEIVE_DISCRIMINATOR: [u8; 8] = [8, 179, 120, 109, 33, 118, 189, 80];

pub const CONFIG_TYPE_EXECUTOR: u32 = 1;
pub const CONFIG_TYPE_SEND_ULN: u32 = 2;
pub const CONFIG_TYPE_RECEIVE_ULN: u32 = 3;
/// `UlnConfig` count meaning "none" (as opposed to 0 = "LayerZero default").
pub const NIL_DVN_COUNT: u8 = u8::MAX;

pub const LZ_RECEIVE_TYPES_VERSION: u8 = 2;
pub const EXECUTION_CONTEXT_VERSION_1: u8 = 1;

pub fn endpoint_settings_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[ENDPOINT_SEED], &ENDPOINT_ID)
}

pub fn oapp_registry_pda(oapp: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[OAPP_SEED, oapp.as_ref()], &ENDPOINT_ID)
}

pub fn nonce_pda(oapp: &Pubkey, eid: u32, remote: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[NONCE_SEED, oapp.as_ref(), &eid.to_be_bytes(), remote],
        &ENDPOINT_ID,
    )
}

pub fn pending_nonce_pda(oapp: &Pubkey, eid: u32, remote: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[PENDING_NONCE_SEED, oapp.as_ref(), &eid.to_be_bytes(), remote],
        &ENDPOINT_ID,
    )
}

pub fn payload_hash_pda(receiver: &Pubkey, src_eid: u32, sender: &[u8; 32], nonce: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            PAYLOAD_HASH_SEED,
            receiver.as_ref(),
            &src_eid.to_be_bytes(),
            sender,
            &nonce.to_be_bytes(),
        ],
        &ENDPOINT_ID,
    )
}

pub fn send_library_config_pda(oapp: &Pubkey, eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SEND_LIBRARY_CONFIG_SEED, oapp.as_ref(), &eid.to_be_bytes()],
        &ENDPOINT_ID,
    )
}

pub fn default_send_library_config_pda(eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SEND_LIBRARY_CONFIG_SEED, &eid.to_be_bytes()], &ENDPOINT_ID)
}

pub fn receive_library_config_pda(oapp: &Pubkey, eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RECEIVE_LIBRARY_CONFIG_SEED, oapp.as_ref(), &eid.to_be_bytes()],
        &ENDPOINT_ID,
    )
}

/// The endpoint's record for a message library, keyed by the library's
/// `MessageLib` PDA (for ULN302: [`uln_settings_pda`]), not its program ID.
pub fn message_lib_info_pda(message_lib: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MESSAGE_LIB_SEED, message_lib.as_ref()], &ENDPOINT_ID)
}

pub fn endpoint_event_authority() -> (Pubkey, u8) {
    event_authority_pda(&ENDPOINT_ID)
}

/// ULN302's settings account, which is also the address the endpoint uses to
/// name the library (`new_lib`, `message_lib`).
pub fn uln_settings_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MESSAGE_LIB_SEED], &ULN_ID)
}

pub fn uln_send_config_pda(eid: u32, oapp: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SEND_CONFIG_SEED, &eid.to_be_bytes(), oapp.as_ref()], &ULN_ID)
}

pub fn uln_receive_config_pda(eid: u32, oapp: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RECEIVE_CONFIG_SEED, &eid.to_be_bytes(), oapp.as_ref()], &ULN_ID)
}

pub fn uln_default_send_config_pda(eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SEND_CONFIG_SEED, &eid.to_be_bytes()], &ULN_ID)
}

pub fn uln_default_receive_config_pda(eid: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RECEIVE_CONFIG_SEED, &eid.to_be_bytes()], &ULN_ID)
}

pub fn uln_event_authority() -> (Pubkey, u8) {
    event_authority_pda(&ULN_ID)
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct RegisterOAppParams {
    pub delegate: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitNonceParams {
    pub local_oapp: Pubkey,
    pub remote_eid: u32,
    pub remote_oapp: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SetSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SetReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
    pub grace_period: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct InitConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SetConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
    pub config_type: u32,
    /// Plain Borsh of the inner `UlnConfig` / `ExecutorConfig`, no enum tag.
    pub config: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct ClearParams {
    pub receiver: Pubkey,
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
    pub message: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SendParams {
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct QuoteParams {
    pub sender: Pubkey,
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub pay_in_lz_token: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq)]
pub struct MessagingFee {
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveParams {
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
    pub message: Vec<u8>,
    pub extra_data: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct UlnConfig {
    pub confirmations: u64,
    pub required_dvn_count: u8,
    pub optional_dvn_count: u8,
    pub optional_dvn_threshold: u8,
    pub required_dvns: Vec<Pubkey>,
    pub optional_dvns: Vec<Pubkey>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct ExecutorConfig {
    pub max_message_size: u32,
    pub executor: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveTypesV2Accounts {
    pub accounts: Vec<Pubkey>,
}

/// Borsh-identical to LayerZero's `(u8, LzReceiveTypesV2Accounts)` tuple
/// return; a named struct so the IDL builder can describe it.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveTypesInfoResult {
    pub version: u8,
    pub accounts: LzReceiveTypesV2Accounts,
}

/// Variant order is the Borsh tag and part of LayerZero's ABI.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub enum AddressLocator {
    Address(Pubkey),
    AltIndex(u8, u8),
    Payer,
    Signer(u8),
    Context,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct AccountMetaRef {
    pub pubkey: AddressLocator,
    pub is_writable: bool,
}

/// LayerZero's `Instruction` enum, renamed to avoid the Solana type.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub enum LzInstruction {
    LzReceive {
        accounts: Vec<AccountMetaRef>,
    },
    Standard {
        program_id: Pubkey,
        accounts: Vec<AccountMetaRef>,
        data: Vec<u8>,
    },
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct LzReceiveTypesV2Result {
    pub context_version: u8,
    pub alts: Vec<Pubkey>,
    pub instructions: Vec<LzInstruction>,
}

/// Type-3 executor options carrying one gas-only `lzReceive` option:
/// `u16 type=3 ‖ u8 worker=1 ‖ u16 size=17 ‖ u8 option=1 ‖ u128 gas` (all BE).
pub fn lz_receive_options(gas: u128) -> Vec<u8> {
    [
        3u16.to_be_bytes().as_slice(),
        &[1u8],
        &17u16.to_be_bytes(),
        &[1u8],
        &gas.to_be_bytes(),
    ]
    .concat()
}

/// CPIs `program_id` with `discriminator ‖ borsh(params)`. `accounts[0]` must
/// be the callee program (it is handed to the runtime but not listed); the rest
/// become the instruction's account metas in order, keeping each account's
/// writable flag. `signers` are PDAs this program signs for with
/// `signer_seeds`; every other account keeps the signer flag it arrived with.
pub fn invoke<'info>(
    program_id: Pubkey,
    discriminator: [u8; 8],
    params: &impl AnchorSerialize,
    accounts: &[AccountInfo<'info>],
    signers: &[Pubkey],
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let (program, metas) = accounts
        .split_first()
        .ok_or(LayerZeroProverError::InvalidEndpoint)?;
    require_keys_eq!(program.key(), program_id, LayerZeroProverError::InvalidEndpoint);

    let data = discriminator
        .into_iter()
        .chain(anchor_lang::prelude::borsh::to_vec(params)?)
        .collect();
    let metas = metas
        .iter()
        .map(|account| AccountMeta {
            pubkey: account.key(),
            is_signer: account.is_signer || signers.contains(account.key),
            is_writable: account.is_writable,
        })
        .collect();

    invoke_signed(
        &Instruction {
            program_id,
            accounts: metas,
            data,
        },
        accounts,
        signer_seeds,
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz_receive_options_gas_only_layout() {
        let expected = "00030100110100000000000000000000000000030d40";
        let actual: String = lz_receive_options(200_000)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn endpoint_pdas_deterministic() {
        let oapp = Pubkey::new_from_array([7; 32]);
        let remote = [9u8; 32];
        goldie::assert_json!(vec![
            endpoint_settings_pda(),
            oapp_registry_pda(&oapp),
            nonce_pda(&oapp, 30184, &remote),
            pending_nonce_pda(&oapp, 30184, &remote),
            payload_hash_pda(&oapp, 30184, &remote, 1),
            send_library_config_pda(&oapp, 30184),
            default_send_library_config_pda(30184),
            receive_library_config_pda(&oapp, 30184),
            message_lib_info_pda(&uln_settings_pda().0),
            endpoint_event_authority(),
        ]);
    }

    #[test]
    fn uln_pdas_deterministic() {
        let oapp = Pubkey::new_from_array([7; 32]);
        goldie::assert_json!(vec![
            uln_settings_pda(),
            uln_send_config_pda(30184, &oapp),
            uln_receive_config_pda(30184, &oapp),
            uln_default_send_config_pda(30184),
            uln_default_receive_config_pda(30184),
            uln_event_authority(),
        ]);
    }
}
```

- [ ] **Step 5: Write `src/state.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;
use tiny_keccak::{Hasher, Keccak};

use crate::constants::MAX_INTENTS_PER_PROVE;
use crate::instructions::LayerZeroProverError;
use crate::layerzero::LZ_RECEIVE_TYPES_SEED;

pub const STORE_SEED: &[u8] = b"Store";
pub const PDA_PAYER_SEED: &[u8] = b"pda_payer";
pub const PENDING_SEND_SEED: &[u8] = b"pending_send";
pub const MAX_PEERS: usize = 16;
pub const MAX_PAYLOAD_LEN: usize = 8 + 64 * MAX_INTENTS_PER_PROVE;

/// A remote EVM chain this OApp talks to. Also the inbound whitelist and the
/// trusted `src_eid → chain_id` map.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// LayerZero endpoint ID of the EVM chain.
    pub eid: u32,
    /// That chain's EVM `LayerZeroProver`, left-padded to 32 bytes.
    pub address: Bytes32,
    /// The chain's EVM chain ID: the only `Proof.destination` it may claim.
    pub chain_id: u64,
}

/// The OApp address registered with the endpoint; signs `send` and `clear`.
#[account]
#[derive(InitSpace, Debug)]
pub struct Store {
    #[max_len(MAX_PEERS)]
    pub peers: Vec<Peer>,
    /// Lookup table the executor uses for `lz_receive`'s static accounts.
    pub alt: Pubkey,
}

impl Store {
    pub fn new(peers: Vec<Peer>) -> Result<Self> {
        require!(
            !peers.is_empty() && peers.len() <= MAX_PEERS,
            LayerZeroProverError::InvalidPeerSet
        );
        peers.iter().enumerate().try_for_each(|(i, peer)| {
            require!(
                peer.eid != 0 && peer.chain_id != 0 && *peer.address != [0u8; 32],
                LayerZeroProverError::InvalidPeerSet
            );
            require!(
                peers[..i]
                    .iter()
                    .all(|other| other.eid != peer.eid && other.chain_id != peer.chain_id),
                LayerZeroProverError::InvalidPeerSet
            );

            Ok(())
        })?;

        Ok(Self {
            peers,
            alt: Pubkey::default(),
        })
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[STORE_SEED], &crate::ID)
    }

    pub fn peer(&self, eid: u32) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.eid == eid)
    }
}

impl AccountExt for Store {}

/// Required by the executor at fixed seeds `[LzReceiveTypes, store]`.
#[account]
#[derive(InitSpace, Debug)]
pub struct LzReceiveTypesAccount {
    pub store: Pubkey,
}

impl LzReceiveTypesAccount {
    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[LZ_RECEIVE_TYPES_SEED, Store::pda().0.as_ref()],
            &crate::ID,
        )
    }
}

impl AccountExt for LzReceiveTypesAccount {}

/// Outbound commit written by `prove` (inside `portal::prove`) and consumed by
/// the top-level `send_message`. Its address commits to the content, so an
/// existing account at the derived address is necessarily the same batch.
#[account]
#[derive(InitSpace, Debug, PartialEq)]
pub struct PendingSend {
    pub dst_eid: u32,
    pub receiver: Bytes32,
    #[max_len(MAX_PAYLOAD_LEN)]
    pub payload: Vec<u8>,
    pub rent_payer: Pubkey,
}

impl PendingSend {
    pub fn key(dst_eid: u32, receiver: &Bytes32, payload: &[u8]) -> [u8; 32] {
        let mut hasher = Keccak::v256();
        hasher.update(&dst_eid.to_le_bytes());
        hasher.update(receiver.as_ref());
        hasher.update(payload);
        let mut key = [0u8; 32];
        hasher.finalize(&mut key);

        key
    }

    pub fn pda(dst_eid: u32, receiver: &Bytes32, payload: &[u8]) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[PENDING_SEND_SEED, &Self::key(dst_eid, receiver, payload)],
            &crate::ID,
        )
    }

    pub fn intent_count(&self) -> usize {
        (self.payload.len() - 8) / 64
    }
}

impl AccountExt for PendingSend {}

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub Proof);

impl AccountExt for ProofAccount {}

impl From<Proof> for ProofAccount {
    fn from(proof: Proof) -> Self {
        Self(proof)
    }
}

/// System-owned lamport reserve: pays `Proof` rent in `lz_receive` (refunded
/// by `close_proof`) and is the OApp's LayerZero delegate, which the endpoint
/// requires to sign *and* pay for path setup.
pub fn pda_payer_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PDA_PAYER_SEED], &crate::ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(eid: u32, chain_id: u64, byte: u8) -> Peer {
        Peer {
            eid,
            address: [byte; 32].into(),
            chain_id,
        }
    }

    #[test]
    fn pdas_deterministic() {
        goldie::assert_json!(vec![
            Store::pda(),
            LzReceiveTypesAccount::pda(),
            pda_payer_pda(),
            PendingSend::pda(30184, &[1; 32].into(), &[2; 72]),
        ]);
    }

    /// The portal authorities `prove` and `close_proof` accept, scoped to this
    /// program's ID. A seed change silently locks out portal — pin the address.
    #[test]
    fn accepted_portal_authorities_deterministic() {
        goldie::assert_json!(vec![
            portal::state::dispatcher_pda(&crate::ID),
            portal::state::proof_closer_pda(&crate::ID),
        ]);
    }

    #[test]
    fn store_new_accepts_valid_peers() {
        let peers = vec![peer(30184, 8453, 1), peer(30111, 10, 2)];
        assert_eq!(Store::new(peers.clone()).unwrap().peers, peers);
    }

    #[test]
    fn store_new_rejects_invalid_peer_sets() {
        let cases = [
            vec![],
            vec![peer(0, 8453, 1)],
            vec![peer(30184, 0, 1)],
            vec![peer(30184, 8453, 0)],
            vec![peer(30184, 8453, 1), peer(30184, 10, 2)],
            vec![peer(30184, 8453, 1), peer(30111, 8453, 2)],
            (1..=MAX_PEERS as u32 + 1)
                .map(|i| peer(i, i as u64, 1))
                .collect(),
        ];
        cases.into_iter().for_each(|peers| {
            assert!(Store::new(peers).is_err());
        });
    }

    #[test]
    fn pending_send_key_commits_to_every_field() {
        let receiver: Bytes32 = [1; 32].into();
        let base = PendingSend::key(30184, &receiver, &[2; 72]);
        assert_ne!(base, PendingSend::key(30111, &receiver, &[2; 72]));
        assert_ne!(base, PendingSend::key(30184, &[3; 32].into(), &[2; 72]));
        assert_ne!(base, PendingSend::key(30184, &receiver, &[4; 72]));
    }
}
```

- [ ] **Step 6: Write `src/instructions/mod.rs` and `close_proof.rs`**

`src/instructions/mod.rs` (later tasks add `mod`/`pub use` lines; the error enum is complete now so codes never shift):

```rust
use anchor_lang::prelude::*;

mod close_proof;

pub use close_proof::*;

#[error_code]
pub enum LayerZeroProverError {
    InvalidPortalDispatcher,
    InvalidPortalProofCloser,
    InvalidAuthority,
    InvalidStore,
    InvalidLzReceiveTypes,
    InvalidPdaPayer,
    InvalidPeerSet,
    UnknownPeer,
    InvalidReceiver,
    InvalidData,
    InvalidDomainId,
    EmptyProof,
    TooManyIntents,
    UnpinnedConfig,
    AltNotSet,
    InvalidEndpoint,
    InvalidUln,
    InvalidSender,
    ChainIdMismatch,
    InvalidProof,
    IntentAlreadyProven,
    InvalidPendingSend,
    InvalidRentPayer,
    InvalidQuote,
}
```

`src/instructions/close_proof.rs`:

```rust
use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::state::{pda_payer_pda, ProofAccount};

/// Takes no dependency beyond its own accounts so it cannot fail once the
/// program is finalized: `portal::refund` CPIs it on proven cancellations.
#[derive(Accounts)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&crate::ID).0 @ LayerZeroProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.pda_payer.to_account_info())
}
```

- [ ] **Step 7: Write `src/lib.rs`**

```rust
use anchor_lang::prelude::*;

declare_id!("<PROGRAM_ID>");

pub mod constants;
pub mod instructions;
pub mod layerzero;
pub mod state;

use instructions::*;

#[program]
pub mod layerzero_prover {
    use super::*;

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
```

- [ ] **Step 8: Run unit tests to verify they fail (no snapshots yet)**

Run: `cargo test --package layerzero-prover`
Expected: the goldie tests FAIL (missing `testdata/*.golden`); `lz_receive_options_gas_only_layout`, `lz_receive_gas_matches_evm_floor`, `store_new_*`, `pending_send_key_commits_to_every_field` PASS.

- [ ] **Step 9: Record snapshots and re-run**

Run: `GOLDIE_UPDATE=1 cargo test --package layerzero-prover && cargo test --package layerzero-prover`
Expected: PASS. Review the new files under `programs/layerzero-prover/src/{layerzero,state}/testdata/` — PDA tuples only.

- [ ] **Step 10: Build and lint**

Run: `anchor build --program-name layerzero-prover && cargo clippy --all-targets -- -D warnings && cargo +nightly fmt && cargo sort --workspace --check`
Expected: success; `target/deploy/layerzero_prover.so` exists.

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml Cargo.lock Anchor.toml programs/layerzero-prover
git commit -m "feat(layerzero-prover): scaffold program with LayerZero mirror, state and close_proof"
```

---

### Task 2: Mock endpoint, test context, discriminator pins, `close_proof` tests

**Files:**
- Create: `programs/mock-layerzero-endpoint/Cargo.toml`, `programs/mock-layerzero-endpoint/src/lib.rs`, `integration-tests/tests/common/layerzero_prover_context.rs`, `integration-tests/tests/close_proof_layerzero_prover.rs`
- Modify: `Cargo.toml`, `Anchor.toml`, `integration-tests/Cargo.toml`, `integration-tests/tests/common/mod.rs`

**Interfaces:**
- Consumes: Task 1 `layerzero::*`, `state::*`, `close_proof`.
- Produces:
  - Mock program `mock_layerzero_endpoint` (ID = `ENDPOINT_ID`) with instructions `mock_init(eid: u32)`, `mock_verify(MockVerifyParams)`, `register_oapp`, `init_nonce`, `init_send_library`, `init_receive_library`, `set_send_library`, `set_receive_library`, `init_config`, `set_config`, `clear`, `send_packet` (discriminator = `SEND_DISCRIMINATOR`), `quote`; account types `EndpointSettings, OAppRegistry, Nonce, PendingInboundNonce, PayloadHash, SendLibraryConfig, ReceiveLibraryConfig, MessageLibInfo` (byte-identical to LayerZero's); events `MockConfigInitialized { oapp, eid }`, `MockConfigSet { oapp, eid, config_type, config }`, `MockPacketSent { sender, dst_eid, receiver, message, options, native_fee, nonce }`; const `MOCK_NATIVE_FEE = 1_000_000`.
  - Test context `LayerZeroProver<'a>` via `Context::layerzero_prover()`, with: consts `BASE_EID=30184, BASE_CHAIN_ID=8453, OP_EID=30111, OP_CHAIN_ID=10, COMPUTE_UNIT_LIMIT=1_400_000, TREASURY`; free fns `evm_peer(eid, chain_id, byte) -> Peer`, `peers() -> Vec<Peer>`; methods `install(&mut self, authority: Pubkey)`, `program_data(&self) -> Pubkey`, `finalize(&mut self)`, `send(&mut self, Vec<Instruction>, &[&Keypair]) -> TransactionResult`, `funded_native_intent(&mut self, destination: u64, prover: Pubkey) -> (Reward, Bytes32, Bytes32)`.

- [ ] **Step 1: Create the mock crate**

`programs/mock-layerzero-endpoint/Cargo.toml`:

```toml
[package]
description = "Localnet stand-in for LayerZero's EndpointV2 program"
edition = "2021"
name = "mock-layerzero-endpoint"
version = "0.1.0"

[package.metadata.solana]
tools-version = "v1.52"

[lib]
crate-type = ["cdylib", "lib"]
name = "mock_layerzero_endpoint"

[features]
cpi = ["no-entrypoint"]
default = []
idl-build = ["anchor-lang/idl-build"]
no-entrypoint = []
no-idl = []
no-log-ix-name = []

[dependencies]
anchor-lang = { workspace = true, features = ["event-cpi"] }
tiny-keccak = { workspace = true }
```

`programs/mock-layerzero-endpoint/src/lib.rs`:

```rust
//! Test-only stand-in for LayerZero's EndpointV2 (localnet only; excluded from
//! devnet/mainnet builds). Instruction names, params and account layouts match
//! LayerZero-v2@9c741e7f so the discriminators, seeds and account orders
//! layerzero-prover mirrors resolve here unchanged. Library/worker accounts are
//! accepted and ignored; `set_config` and `send` emit events instead of
//! touching a ULN so tests can assert exactly what was configured or sent.
//! `mock_init` / `mock_verify` stand in for LayerZero's admin setup and DVN
//! verification.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::system_instruction;
use tiny_keccak::{Hasher, Keccak};

declare_id!("76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6");

pub const MOCK_NATIVE_FEE: u64 = 1_000_000;
const SEND_DISCRIMINATOR: [u8; 8] = [102, 251, 20, 187, 65, 75, 12, 69];

const ENDPOINT_SEED: &[u8] = b"Endpoint";
const OAPP_SEED: &[u8] = b"OApp";
const NONCE_SEED: &[u8] = b"Nonce";
const PENDING_NONCE_SEED: &[u8] = b"PendingNonce";
const PAYLOAD_HASH_SEED: &[u8] = b"PayloadHash";
const SEND_LIBRARY_CONFIG_SEED: &[u8] = b"SendLibraryConfig";
const RECEIVE_LIBRARY_CONFIG_SEED: &[u8] = b"ReceiveLibraryConfig";
const MESSAGE_LIB_SEED: &[u8] = b"MessageLib";

#[program]
pub mod mock_layerzero_endpoint {
    use super::*;

    pub fn mock_init(ctx: Context<MockInit>, eid: u32) -> Result<()> {
        ctx.accounts.endpoint.set_inner(EndpointSettings {
            eid,
            bump: ctx.bumps.endpoint,
            admin: ctx.accounts.payer.key(),
            lz_token_mint: None,
        });
        Ok(())
    }

    pub fn mock_verify(ctx: Context<MockVerify>, params: MockVerifyParams) -> Result<()> {
        ctx.accounts.payload_hash.set_inner(PayloadHash {
            hash: params.payload_hash,
            bump: ctx.bumps.payload_hash,
        });
        let nonce = &mut ctx.accounts.nonce;
        nonce.inbound_nonce = nonce.inbound_nonce.max(params.nonce);
        Ok(())
    }

    pub fn register_oapp(ctx: Context<RegisterOApp>, params: RegisterOAppParams) -> Result<()> {
        ctx.accounts.oapp_registry.set_inner(OAppRegistry {
            delegate: params.delegate,
            bump: ctx.bumps.oapp_registry,
        });
        Ok(())
    }

    pub fn init_nonce(ctx: Context<InitNonce>, _params: InitNonceParams) -> Result<()> {
        ctx.accounts.nonce.set_inner(Nonce {
            bump: ctx.bumps.nonce,
            outbound_nonce: 0,
            inbound_nonce: 0,
        });
        ctx.accounts.pending_inbound_nonce.set_inner(PendingInboundNonce {
            nonces: vec![],
            bump: ctx.bumps.pending_inbound_nonce,
        });
        Ok(())
    }

    pub fn init_send_library(ctx: Context<InitSendLibrary>, _params: InitSendLibraryParams) -> Result<()> {
        ctx.accounts.send_library_config.set_inner(SendLibraryConfig {
            message_lib: Pubkey::default(),
            bump: ctx.bumps.send_library_config,
        });
        Ok(())
    }

    pub fn init_receive_library(
        ctx: Context<InitReceiveLibrary>,
        _params: InitReceiveLibraryParams,
    ) -> Result<()> {
        ctx.accounts.receive_library_config.set_inner(ReceiveLibraryConfig {
            message_lib: Pubkey::default(),
            timeout: None,
            bump: ctx.bumps.receive_library_config,
        });
        Ok(())
    }

    pub fn set_send_library(ctx: Context<SetSendLibrary>, params: SetSendLibraryParams) -> Result<()> {
        ctx.accounts.send_library_config.message_lib = params.new_lib;
        Ok(())
    }

    pub fn set_receive_library(
        ctx: Context<SetReceiveLibrary>,
        params: SetReceiveLibraryParams,
    ) -> Result<()> {
        ctx.accounts.receive_library_config.message_lib = params.new_lib;
        Ok(())
    }

    pub fn init_config<'info>(
        ctx: Context<'info, InitConfig<'info>>,
        params: InitConfigParams,
    ) -> Result<()> {
        let payer = ctx
            .remaining_accounts
            .first()
            .ok_or(MockEndpointError::MissingAccount)?;
        require!(
            payer.is_signer && payer.is_writable,
            MockEndpointError::InvalidPayer
        );
        emit!(MockConfigInitialized {
            oapp: params.oapp,
            eid: params.eid,
        });
        Ok(())
    }

    pub fn set_config(_ctx: Context<SetConfig>, params: SetConfigParams) -> Result<()> {
        emit!(MockConfigSet {
            oapp: params.oapp,
            eid: params.eid,
            config_type: params.config_type,
            config: params.config,
        });
        Ok(())
    }

    pub fn clear(ctx: Context<Clear>, params: ClearParams) -> Result<[u8; 32]> {
        let mut hasher = Keccak::v256();
        hasher.update(&params.guid);
        hasher.update(&params.message);
        let mut hash = [0u8; 32];
        hasher.finalize(&mut hash);
        require!(
            hash == ctx.accounts.payload_hash.hash,
            MockEndpointError::PayloadHashNotFound
        );
        Ok(hash)
    }

    #[instruction(discriminator = &SEND_DISCRIMINATOR)]
    pub fn send_packet<'info>(
        ctx: Context<'info, SendPacket<'info>>,
        params: SendParams,
    ) -> Result<MessagingReceipt> {
        require!(
            params.native_fee >= MOCK_NATIVE_FEE,
            MockEndpointError::InsufficientFee
        );
        // ULN `send` tail: [uln, send_config, default_send_config, payer, treasury, system_program, ..]
        let [_uln, _send_config, _default_send_config, payer, treasury, system_program, ..] =
            ctx.remaining_accounts
        else {
            return err!(MockEndpointError::MissingAccount);
        };
        invoke(
            &system_instruction::transfer(payer.key, treasury.key, MOCK_NATIVE_FEE),
            &[payer.clone(), treasury.clone(), system_program.clone()],
        )?;

        let nonce = &mut ctx.accounts.nonce;
        nonce.outbound_nonce += 1;
        emit!(MockPacketSent {
            sender: ctx.accounts.sender.key(),
            dst_eid: params.dst_eid,
            receiver: params.receiver,
            message: params.message,
            options: params.options,
            native_fee: params.native_fee,
            nonce: nonce.outbound_nonce,
        });

        Ok(MessagingReceipt {
            guid: [0; 32],
            nonce: nonce.outbound_nonce,
            fee: MessagingFee {
                native_fee: MOCK_NATIVE_FEE,
                lz_token_fee: 0,
            },
        })
    }

    pub fn quote<'info>(ctx: Context<'info, Quote<'info>>, _params: QuoteParams) -> Result<MessagingFee> {
        require!(
            ctx.remaining_accounts.iter().all(|account| !account.is_writable),
            MockEndpointError::WritableAccountNotAllowed
        );
        Ok(MessagingFee {
            native_fee: MOCK_NATIVE_FEE,
            lz_token_fee: 0,
        })
    }
}

#[account]
#[derive(InitSpace)]
pub struct EndpointSettings {
    pub eid: u32,
    pub bump: u8,
    pub admin: Pubkey,
    pub lz_token_mint: Option<Pubkey>,
}

#[account]
#[derive(InitSpace)]
pub struct OAppRegistry {
    pub delegate: Pubkey,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Nonce {
    pub bump: u8,
    pub outbound_nonce: u64,
    pub inbound_nonce: u64,
}

#[account]
#[derive(InitSpace)]
pub struct PendingInboundNonce {
    #[max_len(256)]
    pub nonces: Vec<u64>,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct PayloadHash {
    pub hash: [u8; 32],
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct SendLibraryConfig {
    pub message_lib: Pubkey,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone)]
pub struct ReceiveLibraryTimeout {
    pub message_lib: Pubkey,
    pub expiry: u64,
}

#[account]
#[derive(InitSpace)]
pub struct ReceiveLibraryConfig {
    pub message_lib: Pubkey,
    pub timeout: Option<ReceiveLibraryTimeout>,
    pub bump: u8,
}

/// Not used by the mock's instructions; mirrored so tests can stage the
/// account the real endpoint reads in `set_*_library`.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, PartialEq)]
pub enum MessageLibType {
    Send,
    Receive,
    SendAndReceive,
}

#[account]
#[derive(InitSpace)]
pub struct MessageLibInfo {
    pub message_lib_type: MessageLibType,
    pub bump: u8,
    pub message_lib_bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MockVerifyParams {
    pub receiver: Pubkey,
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub payload_hash: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct RegisterOAppParams {
    pub delegate: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitNonceParams {
    pub local_oapp: Pubkey,
    pub remote_eid: u32,
    pub remote_oapp: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SetSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SetReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
    pub grace_period: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SetConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
    pub config_type: u32,
    pub config: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ClearParams {
    pub receiver: Pubkey,
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
    pub message: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SendParams {
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct QuoteParams {
    pub sender: Pubkey,
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub pay_in_lz_token: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MessagingFee {
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MessagingReceipt {
    pub guid: [u8; 32],
    pub nonce: u64,
    pub fee: MessagingFee,
}

#[event]
pub struct MockConfigInitialized {
    pub oapp: Pubkey,
    pub eid: u32,
}

#[event]
pub struct MockConfigSet {
    pub oapp: Pubkey,
    pub eid: u32,
    pub config_type: u32,
    pub config: Vec<u8>,
}

#[event]
pub struct MockPacketSent {
    pub sender: Pubkey,
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub native_fee: u64,
    pub nonce: u64,
}

#[error_code]
pub enum MockEndpointError {
    Unauthorized,
    SameValue,
    ReadOnlyAccount,
    InvalidNonce,
    PayloadHashNotFound,
    InsufficientFee,
    MissingAccount,
    InvalidPayer,
    WritableAccountNotAllowed,
}

#[derive(Accounts)]
pub struct MockInit<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(init, payer = payer, space = 8 + EndpointSettings::INIT_SPACE, seeds = [ENDPOINT_SEED], bump)]
    pub endpoint: Account<'info, EndpointSettings>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: MockVerifyParams)]
pub struct MockVerify<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [NONCE_SEED, params.receiver.as_ref(), &params.src_eid.to_be_bytes(), &params.sender[..]],
        bump = nonce.bump
    )]
    pub nonce: Account<'info, Nonce>,
    #[account(
        init,
        payer = payer,
        space = 8 + PayloadHash::INIT_SPACE,
        seeds = [
            PAYLOAD_HASH_SEED,
            params.receiver.as_ref(),
            &params.src_eid.to_be_bytes(),
            &params.sender[..],
            &params.nonce.to_be_bytes()
        ],
        bump
    )]
    pub payload_hash: Account<'info, PayloadHash>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
pub struct RegisterOApp<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub oapp: Signer<'info>,
    #[account(init, payer = payer, space = 8 + OAppRegistry::INIT_SPACE, seeds = [OAPP_SEED, oapp.key().as_ref()], bump)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: InitNonceParams)]
pub struct InitNonce<'info> {
    #[account(mut)]
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.local_oapp.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        init,
        payer = delegate,
        space = 8 + Nonce::INIT_SPACE,
        seeds = [NONCE_SEED, params.local_oapp.as_ref(), &params.remote_eid.to_be_bytes(), &params.remote_oapp[..]],
        bump
    )]
    pub nonce: Account<'info, Nonce>,
    #[account(
        init,
        payer = delegate,
        space = 8 + PendingInboundNonce::INIT_SPACE,
        seeds = [PENDING_NONCE_SEED, params.local_oapp.as_ref(), &params.remote_eid.to_be_bytes(), &params.remote_oapp[..]],
        bump
    )]
    pub pending_inbound_nonce: Account<'info, PendingInboundNonce>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: InitSendLibraryParams)]
pub struct InitSendLibrary<'info> {
    #[account(mut)]
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.sender.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        init,
        payer = delegate,
        space = 8 + SendLibraryConfig::INIT_SPACE,
        seeds = [SEND_LIBRARY_CONFIG_SEED, params.sender.as_ref(), &params.eid.to_be_bytes()],
        bump
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: InitReceiveLibraryParams)]
pub struct InitReceiveLibrary<'info> {
    #[account(mut)]
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.receiver.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        init,
        payer = delegate,
        space = 8 + ReceiveLibraryConfig::INIT_SPACE,
        seeds = [RECEIVE_LIBRARY_CONFIG_SEED, params.receiver.as_ref(), &params.eid.to_be_bytes()],
        bump
    )]
    pub receive_library_config: Account<'info, ReceiveLibraryConfig>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: SetSendLibraryParams)]
pub struct SetSendLibrary<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.sender.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.sender || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        mut,
        seeds = [SEND_LIBRARY_CONFIG_SEED, params.sender.as_ref(), &params.eid.to_be_bytes()],
        bump = send_library_config.bump,
        constraint = send_library_config.message_lib != params.new_lib @ MockEndpointError::SameValue
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    /// CHECK: address only — the mock keeps no library registry
    #[account(seeds = [MESSAGE_LIB_SEED, params.new_lib.as_ref()], bump)]
    pub message_lib_info: UncheckedAccount<'info>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: SetReceiveLibraryParams)]
pub struct SetReceiveLibrary<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.receiver.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.receiver || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        mut,
        seeds = [RECEIVE_LIBRARY_CONFIG_SEED, params.receiver.as_ref(), &params.eid.to_be_bytes()],
        bump = receive_library_config.bump,
        constraint = receive_library_config.message_lib != params.new_lib @ MockEndpointError::SameValue
    )]
    pub receive_library_config: Account<'info, ReceiveLibraryConfig>,
    /// CHECK: address only — the mock keeps no library registry
    #[account(seeds = [MESSAGE_LIB_SEED, params.new_lib.as_ref()], bump)]
    pub message_lib_info: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(params: InitConfigParams)]
pub struct InitConfig<'info> {
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.oapp.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    /// CHECK: the real endpoint signs into the library with it; must be read-only
    #[account(constraint = !message_lib_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(params: SetConfigParams)]
pub struct SetConfig<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.oapp.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.oapp || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    /// CHECK: must be read-only, as on the real endpoint
    #[account(constraint = !message_lib_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib_program: UncheckedAccount<'info>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: ClearParams)]
pub struct Clear<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.receiver.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.receiver || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        seeds = [NONCE_SEED, params.receiver.as_ref(), &params.src_eid.to_be_bytes(), &params.sender[..]],
        bump = nonce.bump,
        constraint = params.nonce <= nonce.inbound_nonce @ MockEndpointError::InvalidNonce
    )]
    pub nonce: Account<'info, Nonce>,
    #[account(
        mut,
        seeds = [
            PAYLOAD_HASH_SEED,
            params.receiver.as_ref(),
            &params.src_eid.to_be_bytes(),
            &params.sender[..],
            &params.nonce.to_be_bytes()
        ],
        bump = payload_hash.bump,
        close = endpoint
    )]
    pub payload_hash: Account<'info, PayloadHash>,
    #[account(mut, seeds = [ENDPOINT_SEED], bump = endpoint.bump)]
    pub endpoint: Account<'info, EndpointSettings>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: SendParams)]
pub struct SendPacket<'info> {
    pub sender: Signer<'info>,
    /// CHECK: the real endpoint asserts it is the configured send library
    pub send_library_program: UncheckedAccount<'info>,
    #[account(
        seeds = [SEND_LIBRARY_CONFIG_SEED, sender.key().as_ref(), &params.dst_eid.to_be_bytes()],
        bump = send_library_config.bump
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    /// CHECK: LayerZero-admin default; not staged by the mock
    pub default_send_library_config: UncheckedAccount<'info>,
    /// CHECK: must be read-only, as on the real endpoint
    #[account(constraint = !send_library_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub send_library_info: UncheckedAccount<'info>,
    #[account(seeds = [ENDPOINT_SEED], bump = endpoint.bump)]
    pub endpoint: Account<'info, EndpointSettings>,
    #[account(
        mut,
        seeds = [NONCE_SEED, sender.key().as_ref(), &params.dst_eid.to_be_bytes(), &params.receiver[..]],
        bump = nonce.bump
    )]
    pub nonce: Account<'info, Nonce>,
}

#[derive(Accounts)]
#[instruction(params: QuoteParams)]
pub struct Quote<'info> {
    /// CHECK: the real endpoint asserts it is the configured send library
    pub send_library_program: UncheckedAccount<'info>,
    #[account(
        seeds = [SEND_LIBRARY_CONFIG_SEED, params.sender.as_ref(), &params.dst_eid.to_be_bytes()],
        bump = send_library_config.bump
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    /// CHECK: LayerZero-admin default; not staged by the mock
    pub default_send_library_config: UncheckedAccount<'info>,
    /// CHECK: must be read-only, as on the real endpoint
    #[account(constraint = !send_library_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub send_library_info: UncheckedAccount<'info>,
    #[account(seeds = [ENDPOINT_SEED], bump = endpoint.bump)]
    pub endpoint: Account<'info, EndpointSettings>,
    #[account(
        seeds = [NONCE_SEED, params.sender.as_ref(), &params.dst_eid.to_be_bytes(), &params.receiver[..]],
        bump = nonce.bump
    )]
    pub nonce: Account<'info, Nonce>,
}
```

Workspace and Anchor wiring:
- root `Cargo.toml` `[workspace.dependencies]`: `mock-layerzero-endpoint = { path = "programs/mock-layerzero-endpoint" }`
- `Anchor.toml` `[programs.localnet]`: `mock-layerzero-endpoint = "76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6"` (localnet only, like `mock-polymer-prover`).
- `integration-tests/Cargo.toml` `[dependencies]`: `layerzero-prover = { workspace = true, features = ["no-entrypoint"] }` and `mock-layerzero-endpoint = { workspace = true, features = ["no-entrypoint"] }`; add `"layerzero-prover/mainnet",` to the `mainnet` feature list.

- [ ] **Step 2: Wire the mock into the shared test `Context`**

In `integration-tests/tests/common/mod.rs`:

```rust
// next to the other module declarations
pub mod layerzero_prover_context;

// next to the other *_BIN consts
const MOCK_LAYERZERO_ENDPOINT_BIN: &[u8] =
    include_bytes!("../../../target/deploy/mock_layerzero_endpoint.so");
```

and in `impl Default for Context`, after the polymer mock is added:

```rust
        // The mock declares LayerZero's EndpointV2 ID (identical on mainnet and
        // devnet). `layerzero_prover_real` replaces it with the dumped binary.
        svm.add_program(
            layerzero_prover::layerzero::ENDPOINT_ID,
            MOCK_LAYERZERO_ENDPOINT_BIN,
        )
        .unwrap();
```

- [ ] **Step 3: Create `integration-tests/tests/common/layerzero_prover_context.rs`**

```rust
use std::iter;

use anchor_lang::{system_program, AccountSerialize, InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::Bytes32;
use layerzero_prover::layerzero::{self, DEVNET_SOLANA_EID};
use layerzero_prover::state::{pda_payer_pda, Peer};
use portal::types::Reward;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{sol_amount, Context, TransactionResult};

const LAYERZERO_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/layerzero_prover.so");

pub const BASE_EID: u32 = 30184;
pub const BASE_CHAIN_ID: u64 = 8453;
pub const OP_EID: u32 = 30111;
pub const OP_CHAIN_ID: u64 = 10;
pub const COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
/// Receives the mock endpoint's flat fee in `send_message` tests.
pub const TREASURY: Pubkey = Pubkey::new_from_array([0x7e; 32]);

/// An EVM peer whose address is a left-padded 20-byte address of `byte`s.
pub fn evm_peer(eid: u32, chain_id: u64, byte: u8) -> Peer {
    let mut address = [0u8; 32];
    address[12..].fill(byte);

    Peer {
        eid,
        address: address.into(),
        chain_id,
    }
}

pub fn peers() -> Vec<Peer> {
    vec![
        evm_peer(BASE_EID, BASE_CHAIN_ID, 0xba),
        evm_peer(OP_EID, OP_CHAIN_ID, 0x0b),
    ]
}

#[derive(Deref, DerefMut)]
pub struct LayerZeroProver<'a>(&'a mut Context);

impl Context {
    pub fn layerzero_prover(&mut self) -> LayerZeroProver<'_> {
        LayerZeroProver(self)
    }
}

impl LayerZeroProver<'_> {
    /// Adds the program with `authority` as upgrade authority, initializes the
    /// mock endpoint's settings account and funds `pda_payer`.
    pub fn install(&mut self, authority: Pubkey) {
        self.add_program(layerzero_prover::ID, LAYERZERO_PROVER_BIN)
            .unwrap();
        self.set_upgrade_authority(Some(authority));

        let payer = self.payer.pubkey();
        let instruction = Instruction {
            program_id: layerzero::ENDPOINT_ID,
            accounts: mock_layerzero_endpoint::accounts::MockInit {
                payer,
                endpoint: layerzero::endpoint_settings_pda().0,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mock_layerzero_endpoint::instruction::MockInit {
                eid: DEVNET_SOLANA_EID,
            }
            .data(),
        };
        self.send(vec![instruction], &[]).unwrap();

        self.airdrop(&pda_payer_pda().0, sol_amount(10.0)).unwrap();
    }

    pub fn program_data(&self) -> Pubkey {
        let program = self.get_account(&layerzero_prover::ID).unwrap();
        Pubkey::find_program_address(&[layerzero_prover::ID.as_ref()], &program.owner).0
    }

    pub fn set_upgrade_authority(&mut self, authority: Option<Pubkey>) {
        let address = self.program_data();
        let mut program_data = self.get_account(&address).unwrap();
        let metadata = bincode::serialize(&UpgradeableLoaderState::ProgramData {
            slot: 0,
            upgrade_authority_address: authority,
        })
        .unwrap();
        program_data.data[..metadata.len()].copy_from_slice(&metadata);
        self.set_account(address, program_data).unwrap();
    }

    /// Simulates `solana program set-upgrade-authority --final`.
    pub fn finalize(&mut self) {
        self.set_upgrade_authority(None);
    }

    /// Sends `instructions` after a 1.4M CU limit, paid by the context payer
    /// plus any extra `signers`.
    pub fn send(&mut self, instructions: Vec<Instruction>, signers: &[&Keypair]) -> TransactionResult {
        let payer = self.payer.insecure_clone();
        let instructions: Vec<_> =
            iter::once(ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT))
                .chain(instructions)
                .collect();
        let signers: Vec<&Keypair> = iter::once(&payer).chain(signers.iter().copied()).collect();
        let transaction = Transaction::new(
            &signers,
            Message::new(&instructions, Some(&payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// A funded, native-only intent on this (source) chain naming `prover`.
    pub fn funded_native_intent(&mut self, destination: u64, prover: Pubkey) -> (Reward, Bytes32, Bytes32) {
        let (_, _, mut reward) = self.rand_intent();
        reward.prover = prover;
        reward.tokens.clear();
        let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
        let hash = portal::types::intent_hash(destination, &route_hash, &reward.hash());
        let vault = portal::state::vault_pda(&hash).0;
        let funder = self.funder.pubkey();
        self.airdrop(&funder, reward.native_amount).unwrap();
        self.portal()
            .fund_intent(destination, reward.clone(), vault, route_hash, false, Vec::<AccountMeta>::new())
            .unwrap();

        (reward, route_hash, hash)
    }
}

/// Serializes an Anchor account (mock or ours) for `set_account`.
pub fn anchor_account_data<T: AccountSerialize>(account: &T) -> Vec<u8> {
    let mut data = Vec::new();
    account.try_serialize(&mut data).unwrap();
    data
}
```

- [ ] **Step 4: Write the failing tests `integration-tests/tests/close_proof_layerzero_prover.rs`**

```rust
use std::iter;

use anchor_lang::{Discriminator, InstructionData, ToAccountMetas};
use eco_svm_std::prover::Proof;
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::layerzero;
use layerzero_prover::state::pda_payer_pda;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::BASE_CHAIN_ID;

pub mod common;

/// Our hand-written endpoint discriminators equal the Anchor-derived ones of
/// the identically named mock instructions (both `sha256("global:<name>")`),
/// and `lz_receive` equals the constant LayerZero's executor hard-codes.
#[test]
fn mirrored_discriminators_match_anchor_derivation() {
    use mock_layerzero_endpoint::instruction as mock;
    let pairs: [(&[u8], [u8; 8]); 10] = [
        (mock::RegisterOapp::DISCRIMINATOR, layerzero::REGISTER_OAPP_DISCRIMINATOR),
        (mock::InitNonce::DISCRIMINATOR, layerzero::INIT_NONCE_DISCRIMINATOR),
        (mock::InitSendLibrary::DISCRIMINATOR, layerzero::INIT_SEND_LIBRARY_DISCRIMINATOR),
        (mock::InitReceiveLibrary::DISCRIMINATOR, layerzero::INIT_RECEIVE_LIBRARY_DISCRIMINATOR),
        (mock::SetSendLibrary::DISCRIMINATOR, layerzero::SET_SEND_LIBRARY_DISCRIMINATOR),
        (mock::SetReceiveLibrary::DISCRIMINATOR, layerzero::SET_RECEIVE_LIBRARY_DISCRIMINATOR),
        (mock::InitConfig::DISCRIMINATOR, layerzero::INIT_CONFIG_DISCRIMINATOR),
        (mock::SetConfig::DISCRIMINATOR, layerzero::SET_CONFIG_DISCRIMINATOR),
        (mock::Clear::DISCRIMINATOR, layerzero::CLEAR_DISCRIMINATOR),
        (mock::Quote::DISCRIMINATOR, layerzero::QUOTE_DISCRIMINATOR),
    ];
    pairs
        .iter()
        .for_each(|(anchor, mirrored)| assert_eq!(*anchor, mirrored.as_slice()));
}

#[test]
fn close_proof_rejects_foreign_closer() {
    let mut context = common::Context::default();
    context.layerzero_prover().install(Pubkey::new_unique());
    let hash = [5u8; 32].into();
    let proof = Proof::pda(&hash, &layerzero_prover::ID).0;
    context.set_proof(proof, Proof::new(BASE_CHAIN_ID, Pubkey::new_unique()), layerzero_prover::ID);
    let impostor = Keypair::new();

    let instruction = Instruction {
        program_id: layerzero_prover::ID,
        accounts: layerzero_prover::accounts::CloseProof {
            portal_proof_closer: impostor.pubkey(),
            proof,
            pda_payer: pda_payer_pda().0,
        }
        .to_account_metas(None),
        data: layerzero_prover::instruction::CloseProof {}.data(),
    };
    let result = context.layerzero_prover().send(vec![instruction], &[&impostor]);

    assert!(result.is_err_and(common::is_error(
        LayerZeroProverError::InvalidPortalProofCloser
    )));
}

#[test]
fn withdraw_closes_proof_and_refunds_pda_payer() {
    let mut context = common::Context::default();
    context.layerzero_prover().install(Pubkey::new_unique());
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let claimant = Pubkey::new_unique();
    let proof = Proof::pda(&hash, &layerzero_prover::ID).0;
    context.set_proof(proof, Proof::new(BASE_CHAIN_ID, claimant), layerzero_prover::ID);
    let proof_rent = context.get_account(&proof).unwrap().lamports;
    let pda_payer_before = context.balance(&pda_payer_pda().0);

    let result = context.portal().withdraw_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        claimant,
        proof,
        WithdrawnMarker::pda(&hash).0,
        proof_closer_pda(&layerzero_prover::ID).0,
        Vec::<AccountMeta>::new(),
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );

    assert!(result.is_ok());
    assert!(context.get_account(&proof).is_none());
    assert_eq!(context.balance(&pda_payer_pda().0), pda_payer_before + proof_rent);
    assert_eq!(context.balance(&claimant), reward.native_amount);
}
```

- [ ] **Step 5: Run to verify failure, then build and pass**

Run: `cargo test --test close_proof_layerzero_prover`
Expected: FAIL to compile (missing `target/deploy/mock_layerzero_endpoint.so`).
Run: `anchor build && cargo test --test close_proof_layerzero_prover`
Expected: PASS (3 tests). If `mirrored_discriminators_match_anchor_derivation` fails for one name, the Task 1 constant is wrong — recompute with `printf 'global:<name>' | shasum -a 256 | cut -c1-16` and fix `layerzero.rs`, not the test.

- [ ] **Step 6: Lint and commit**

Run: `cargo clippy --all-targets -- -D warnings && cargo +nightly fmt && cargo sort --workspace --check`

```bash
git add Cargo.toml Cargo.lock Anchor.toml programs/mock-layerzero-endpoint integration-tests/Cargo.toml integration-tests/tests/common/mod.rs integration-tests/tests/common/layerzero_prover_context.rs integration-tests/tests/close_proof_layerzero_prover.rs
git commit -m "test(layerzero-prover): add localnet mock endpoint, test context and close_proof tests"
```

---

### Task 3: Setup instructions — `init`, `init_path`, `set_path_config`, `set_alt`

**Files:**
- Create: `programs/layerzero-prover/src/instructions/{init,init_path,set_path_config,set_alt}.rs`, `integration-tests/tests/init_layerzero_prover.rs`
- Modify: `programs/layerzero-prover/src/instructions/mod.rs`, `programs/layerzero-prover/src/lib.rs`, `integration-tests/tests/common/layerzero_prover_context.rs`

**Interfaces:**
- Consumes: Task 1 mirror/state, Task 2 context (`install`, `send`, `finalize`).
- Produces: instructions `init(InitArgs { peers: Vec<Peer> })`, `init_path(eid: u32)`, `set_path_config(eid: u32, config: PathConfig)`, `set_alt()` (alt passed as account); types `InitArgs`, `PathConfig { send_uln: UlnConfig, receive_uln: UlnConfig, executor: ExecutorConfig }`; const `ADDRESS_LOOKUP_TABLE_PROGRAM_ID`. Context methods `init(&Keypair, Vec<Peer>)`, `init_path(&Keypair, &Peer)`, `set_path_config(&Keypair, u32, PathConfig)`, `create_alt() -> Pubkey`, `set_alt(&Keypair, Pubkey)`, `setup() -> Keypair` (install + init + every path + ALT); free fn `path_config() -> PathConfig`.

- [ ] **Step 1: Write the failing tests `integration-tests/tests/init_layerzero_prover.rs`**

```rust
use anchor_lang::prelude::borsh;
use layerzero_prover::instructions::{LayerZeroProverError, PathConfig};
use layerzero_prover::layerzero::{self, CONFIG_TYPE_EXECUTOR, CONFIG_TYPE_RECEIVE_ULN, CONFIG_TYPE_SEND_ULN, NIL_DVN_COUNT};
use layerzero_prover::state::{pda_payer_pda, Store};
use mock_layerzero_endpoint::{MockConfigInitialized, MockConfigSet, Nonce, OAppRegistry, ReceiveLibraryConfig, SendLibraryConfig};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{evm_peer, path_config, peers, BASE_EID};

pub mod common;

fn installed() -> (common::Context, Keypair) {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    (context, authority)
}

#[test]
fn init_registers_store_with_pda_payer_as_delegate() {
    let (mut context, authority) = installed();

    context.layerzero_prover().init(&authority, peers()).unwrap();

    let store = context.account::<Store>(&Store::pda().0).unwrap();
    assert_eq!(store.peers, peers());
    assert_eq!(store.alt, Pubkey::default());
    let registry = context
        .account::<OAppRegistry>(&layerzero::oapp_registry_pda(&Store::pda().0).0)
        .unwrap();
    assert_eq!(registry.delegate, pda_payer_pda().0);
}

#[test]
fn init_rejects_non_authority_and_repeat() {
    let (mut context, authority) = installed();
    let impostor = Keypair::new();

    let result = context.layerzero_prover().init(&impostor, peers());
    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidAuthority)));

    context.layerzero_prover().init(&authority, peers()).unwrap();
    context.expire_blockhash();
    assert!(context.layerzero_prover().init(&authority, peers()).is_err());
}

#[test]
fn init_rejects_invalid_peer_set() {
    let (mut context, authority) = installed();
    let duplicate = vec![evm_peer(BASE_EID, 8453, 1), evm_peer(BASE_EID, 10, 2)];

    let result = context.layerzero_prover().init(&authority, duplicate);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidPeerSet)));
}

#[test]
fn init_path_creates_nonce_and_pins_uln_for_both_directions() {
    let (mut context, authority) = installed();
    context.layerzero_prover().init(&authority, peers()).unwrap();
    let peer = peers()[0];
    let store = Store::pda().0;

    context.layerzero_prover().init_path(&authority, &peer).unwrap();

    let uln = layerzero::uln_settings_pda().0;
    assert!(context
        .account::<Nonce>(&layerzero::nonce_pda(&store, peer.eid, &peer.address).0)
        .is_some());
    let send_library = context
        .account::<SendLibraryConfig>(&layerzero::send_library_config_pda(&store, peer.eid).0)
        .unwrap();
    assert_eq!(send_library.message_lib, uln);
    let receive_library = context
        .account::<ReceiveLibraryConfig>(&layerzero::receive_library_config_pda(&store, peer.eid).0)
        .unwrap();
    assert_eq!(receive_library.message_lib, uln);
}

#[test]
fn init_path_rejects_unknown_peer() {
    let (mut context, authority) = installed();
    context.layerzero_prover().init(&authority, peers()).unwrap();

    let result = context
        .layerzero_prover()
        .init_path(&authority, &evm_peer(40_245, 84532, 0x11));

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));
}

#[test]
fn set_path_config_pins_send_receive_and_executor() {
    let (mut context, authority) = installed();
    context.layerzero_prover().init(&authority, peers()).unwrap();
    context.layerzero_prover().init_path(&authority, &peers()[0]).unwrap();
    let config = path_config();
    let oapp = Store::pda().0;

    let result = context
        .layerzero_prover()
        .set_path_config(&authority, BASE_EID, config.clone())
        .unwrap();

    assert!(common::contains_event(MockConfigInitialized { oapp, eid: BASE_EID })(result.clone()));
    [
        (CONFIG_TYPE_SEND_ULN, borsh::to_vec(&config.send_uln).unwrap()),
        (CONFIG_TYPE_RECEIVE_ULN, borsh::to_vec(&config.receive_uln).unwrap()),
        (CONFIG_TYPE_EXECUTOR, borsh::to_vec(&config.executor).unwrap()),
    ]
    .into_iter()
    .for_each(|(config_type, config)| {
        assert!(common::contains_event(MockConfigSet { oapp, eid: BASE_EID, config_type, config })(result.clone()));
    });
}

#[test]
fn set_path_config_rejects_anything_left_on_layerzero_defaults() {
    let mutations: [fn(&mut PathConfig); 6] = [
        |c| c.send_uln.confirmations = 0,
        |c| c.receive_uln.required_dvn_count = 0,
        |c| c.receive_uln.required_dvn_count = NIL_DVN_COUNT,
        |c| {
            c.send_uln.required_dvns.pop();
        },
        |c| c.executor.max_message_size = 0,
        |c| c.executor.executor = Pubkey::default(),
    ];
    mutations.into_iter().for_each(|mutate| {
        let (mut context, authority) = installed();
        context.layerzero_prover().init(&authority, peers()).unwrap();
        context.layerzero_prover().init_path(&authority, &peers()[0]).unwrap();
        let mut config = path_config();
        mutate(&mut config);

        let result = context
            .layerzero_prover()
            .set_path_config(&authority, BASE_EID, config);

        assert!(result.is_err_and(common::is_error(LayerZeroProverError::UnpinnedConfig)));
    });
}

#[test]
fn set_alt_records_lookup_table() {
    let (mut context, authority) = installed();
    context.layerzero_prover().init(&authority, peers()).unwrap();
    let alt = context.layerzero_prover().create_alt();

    context.layerzero_prover().set_alt(&authority, alt).unwrap();

    assert_eq!(context.account::<Store>(&Store::pda().0).unwrap().alt, alt);
}

#[test]
fn set_alt_rejects_non_lookup_table_account() {
    let (mut context, authority) = installed();
    context.layerzero_prover().init(&authority, peers()).unwrap();
    let not_a_table = Pubkey::new_unique();
    context.airdrop(&not_a_table, 1_000_000_000).unwrap();

    assert!(context.layerzero_prover().set_alt(&authority, not_a_table).is_err());
}

#[test]
fn finalized_program_rejects_every_setup_instruction() {
    let (mut context, authority) = installed();
    context.layerzero_prover().init(&authority, peers()).unwrap();
    let alt = context.layerzero_prover().create_alt();
    context.layerzero_prover().finalize();

    let peer = peers()[0];
    let results = [
        context.layerzero_prover().init_path(&authority, &peer),
        context.layerzero_prover().set_path_config(&authority, peer.eid, path_config()),
        context.layerzero_prover().set_alt(&authority, alt),
    ];

    results.into_iter().for_each(|result| {
        assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidAuthority)));
    });
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test init_layerzero_prover`
Expected: FAIL to compile (`InitArgs`, `PathConfig`, context `init` … not found).

- [ ] **Step 3: Implement `init.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{self, RegisterOAppParams, ENDPOINT_ID, LZ_RECEIVE_TYPES_SEED, REGISTER_OAPP_DISCRIMINATOR};
use crate::state::{pda_payer_pda, LzReceiveTypesAccount, Peer, Store, STORE_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitArgs {
    pub peers: Vec<Peer>,
}

#[derive(Accounts)]
pub struct Init<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    /// CHECK: canonical PDA, created here
    #[account(mut, address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: UncheckedAccount<'info>,
    /// CHECK: canonical PDA, created here
    #[account(mut, address = LzReceiveTypesAccount::pda().0 @ LayerZeroProverError::InvalidLzReceiveTypes)]
    pub lz_receive_types: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint, which validates its seeds
    #[account(mut)]
    pub oapp_registry: UncheckedAccount<'info>,
    /// CHECK: validated by the endpoint's event_cpi
    pub endpoint_event_authority: UncheckedAccount<'info>,
}

pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
    let (store, store_bump) = Store::pda();
    let store_seeds: &[&[u8]] = &[STORE_SEED, &[store_bump]];
    Store::new(args.peers)?.init(
        &ctx.accounts.store,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[store_seeds],
    )?;

    let (_, lz_receive_types_bump) = LzReceiveTypesAccount::pda();
    LzReceiveTypesAccount { store }.init(
        &ctx.accounts.lz_receive_types,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[LZ_RECEIVE_TYPES_SEED, store.as_ref(), &[lz_receive_types_bump]]],
    )?;

    let accounts = ctx.accounts;
    layerzero::invoke(
        ENDPOINT_ID,
        REGISTER_OAPP_DISCRIMINATOR,
        &RegisterOAppParams {
            delegate: pda_payer_pda().0,
        },
        &[
            accounts.endpoint_program.to_account_info(),
            accounts.payer.to_account_info(),
            accounts.store.to_account_info(),
            accounts.oapp_registry.to_account_info(),
            accounts.system_program.to_account_info(),
            accounts.endpoint_event_authority.to_account_info(),
            accounts.endpoint_program.to_account_info(),
        ],
        &[store],
        &[store_seeds],
    )
}
```

- [ ] **Step 4: Implement `init_path.rs`**

```rust
use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    self, message_lib_info_pda, uln_settings_pda, InitNonceParams, InitReceiveLibraryParams,
    InitSendLibraryParams, SetReceiveLibraryParams, SetSendLibraryParams, ENDPOINT_ID,
    INIT_NONCE_DISCRIMINATOR, INIT_RECEIVE_LIBRARY_DISCRIMINATOR, INIT_SEND_LIBRARY_DISCRIMINATOR,
    SET_RECEIVE_LIBRARY_DISCRIMINATOR, SET_SEND_LIBRARY_DISCRIMINATOR,
};
use crate::state::{pda_payer_pda, Store, PDA_PAYER_SEED};

/// Opens one peer's path in both directions and pins ULN302 as its send and
/// receive library. The endpoint validates every PDA it is handed against
/// `store` and `eid`; we pin only what it cannot: the library.
#[derive(Accounts)]
pub struct InitPath<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: system-owned lamport reserve; the OApp's delegate
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: seeds validated by the endpoint
    pub oapp_registry: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub nonce: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub pending_nonce: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub send_library_config: UncheckedAccount<'info>,
    /// CHECK: created by the endpoint
    #[account(mut)]
    pub receive_library_config: UncheckedAccount<'info>,
    /// CHECK: pinned to the endpoint's record for ULN302
    #[account(address = message_lib_info_pda(&uln_settings_pda().0).0 @ LayerZeroProverError::InvalidUln)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: validated by the endpoint's event_cpi
    pub endpoint_event_authority: UncheckedAccount<'info>,
}

pub fn init_path(ctx: Context<InitPath>, eid: u32) -> Result<()> {
    let peer = *ctx
        .accounts
        .store
        .peer(eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    let store = ctx.accounts.store.key();
    let delegate = ctx.accounts.pda_payer.key();
    let (_, bump) = pda_payer_pda();
    let seeds: &[&[u8]] = &[PDA_PAYER_SEED, &[bump]];
    let new_lib = uln_settings_pda().0;
    let a = &ctx.accounts;
    let endpoint = a.endpoint_program.to_account_info();
    let pda_payer = a.pda_payer.to_account_info();
    let registry = a.oapp_registry.to_account_info();
    let system = a.system_program.to_account_info();
    let event_authority = a.endpoint_event_authority.to_account_info();
    let message_lib_info = a.message_lib_info.to_account_info();

    layerzero::invoke(
        ENDPOINT_ID,
        INIT_NONCE_DISCRIMINATOR,
        &InitNonceParams {
            local_oapp: store,
            remote_eid: eid,
            remote_oapp: peer.address.into(),
        },
        &[endpoint.clone(), pda_payer.clone(), registry.clone(), a.nonce.to_account_info(), a.pending_nonce.to_account_info(), system.clone()],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        INIT_SEND_LIBRARY_DISCRIMINATOR,
        &InitSendLibraryParams { sender: store, eid },
        &[endpoint.clone(), pda_payer.clone(), registry.clone(), a.send_library_config.to_account_info(), system.clone()],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        INIT_RECEIVE_LIBRARY_DISCRIMINATOR,
        &InitReceiveLibraryParams { receiver: store, eid },
        &[endpoint.clone(), pda_payer.clone(), registry.clone(), a.receive_library_config.to_account_info(), system],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        SET_SEND_LIBRARY_DISCRIMINATOR,
        &SetSendLibraryParams { sender: store, eid, new_lib },
        &[endpoint.clone(), pda_payer.clone(), registry.clone(), a.send_library_config.to_account_info(), message_lib_info.clone(), event_authority.clone(), endpoint.clone()],
        &[delegate],
        &[seeds],
    )?;
    layerzero::invoke(
        ENDPOINT_ID,
        SET_RECEIVE_LIBRARY_DISCRIMINATOR,
        &SetReceiveLibraryParams { receiver: store, eid, new_lib, grace_period: 0 },
        &[endpoint.clone(), pda_payer, registry, a.receive_library_config.to_account_info(), message_lib_info, event_authority, endpoint],
        &[delegate],
        &[seeds],
    )
}
```

- [ ] **Step 5: Implement `set_path_config.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::event_authority_pda;

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    self, message_lib_info_pda, uln_settings_pda, ExecutorConfig, InitConfigParams, SetConfigParams,
    UlnConfig, CONFIG_TYPE_EXECUTOR, CONFIG_TYPE_RECEIVE_ULN, CONFIG_TYPE_SEND_ULN, ENDPOINT_ID,
    INIT_CONFIG_DISCRIMINATOR, NIL_DVN_COUNT, SET_CONFIG_DISCRIMINATOR, ULN_ID,
};
use crate::state::{pda_payer_pda, Store, PDA_PAYER_SEED};

/// The path's full security config. Nothing may resolve to a LayerZero default
/// (count/confirmations 0, executor default), or LayerZero governance would
/// control our DVN set after we finalize.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct PathConfig {
    pub send_uln: UlnConfig,
    pub receive_uln: UlnConfig,
    pub executor: ExecutorConfig,
}

impl PathConfig {
    fn validate(&self) -> Result<()> {
        [&self.send_uln, &self.receive_uln].iter().try_for_each(|uln| {
            require!(
                uln.confirmations != 0
                    && uln.required_dvn_count != 0
                    && uln.required_dvn_count != NIL_DVN_COUNT
                    && uln.required_dvns.len() == uln.required_dvn_count as usize,
                LayerZeroProverError::UnpinnedConfig
            );
            Ok(())
        })?;
        require!(
            self.executor.max_message_size != 0 && self.executor.executor != Pubkey::default(),
            LayerZeroProverError::UnpinnedConfig
        );

        Ok(())
    }
}

#[derive(Accounts)]
pub struct SetPathConfig<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: system-owned lamport reserve; the OApp's delegate and ULN config payer
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: seeds validated by the endpoint
    pub oapp_registry: UncheckedAccount<'info>,
    /// CHECK: pinned to the endpoint's record for ULN302 (must stay read-only)
    #[account(address = message_lib_info_pda(&uln_settings_pda().0).0 @ LayerZeroProverError::InvalidUln)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: pinned
    #[account(address = uln_settings_pda().0 @ LayerZeroProverError::InvalidUln)]
    pub uln_settings: UncheckedAccount<'info>,
    /// CHECK: pinned
    #[account(address = ULN_ID @ LayerZeroProverError::InvalidUln)]
    pub uln_program: UncheckedAccount<'info>,
    /// CHECK: created/validated by ULN302
    #[account(mut)]
    pub uln_send_config: UncheckedAccount<'info>,
    /// CHECK: created/validated by ULN302
    #[account(mut)]
    pub uln_receive_config: UncheckedAccount<'info>,
    /// CHECK: validated by ULN302
    pub uln_default_send_config: UncheckedAccount<'info>,
    /// CHECK: validated by ULN302
    pub uln_default_receive_config: UncheckedAccount<'info>,
    /// CHECK: pinned
    #[account(address = event_authority_pda(&ULN_ID).0 @ LayerZeroProverError::InvalidUln)]
    pub uln_event_authority: UncheckedAccount<'info>,
}

pub fn set_path_config(ctx: Context<SetPathConfig>, eid: u32, config: PathConfig) -> Result<()> {
    require!(
        ctx.accounts.store.peer(eid).is_some(),
        LayerZeroProverError::UnknownPeer
    );
    config.validate()?;
    let oapp = ctx.accounts.store.key();
    let delegate = ctx.accounts.pda_payer.key();
    let (_, bump) = pda_payer_pda();
    let seeds: &[&[u8]] = &[PDA_PAYER_SEED, &[bump]];
    let a = &ctx.accounts;
    let endpoint_head = [
        a.endpoint_program.to_account_info(),
        a.pda_payer.to_account_info(),
        a.oapp_registry.to_account_info(),
        a.message_lib_info.to_account_info(),
        a.uln_settings.to_account_info(),
        a.uln_program.to_account_info(),
    ];

    // ULN302 `init_config` tail: [payer, uln, send_config, receive_config, system_program]
    let init_tail = [
        a.pda_payer.to_account_info(),
        a.uln_settings.to_account_info(),
        a.uln_send_config.to_account_info(),
        a.uln_receive_config.to_account_info(),
        a.system_program.to_account_info(),
    ];
    layerzero::invoke(
        ENDPOINT_ID,
        INIT_CONFIG_DISCRIMINATOR,
        &InitConfigParams { oapp, eid },
        &[endpoint_head.as_slice(), &init_tail].concat(),
        &[delegate],
        &[seeds],
    )?;

    // ULN302 `set_config` tail: [uln, send_config, receive_config, default_send,
    // default_receive, uln event_authority, uln program]
    let set_tail = [
        a.uln_settings.to_account_info(),
        a.uln_send_config.to_account_info(),
        a.uln_receive_config.to_account_info(),
        a.uln_default_send_config.to_account_info(),
        a.uln_default_receive_config.to_account_info(),
        a.uln_event_authority.to_account_info(),
        a.uln_program.to_account_info(),
    ];
    let accounts = [endpoint_head.as_slice(), &set_tail].concat();
    [
        (CONFIG_TYPE_SEND_ULN, borsh::to_vec(&config.send_uln)?),
        (CONFIG_TYPE_RECEIVE_ULN, borsh::to_vec(&config.receive_uln)?),
        (CONFIG_TYPE_EXECUTOR, borsh::to_vec(&config.executor)?),
    ]
    .into_iter()
    .try_for_each(|(config_type, config)| {
        layerzero::invoke(
            ENDPOINT_ID,
            SET_CONFIG_DISCRIMINATOR,
            &SetConfigParams { oapp, eid, config_type, config },
            &accounts,
            &[delegate],
            &[seeds],
        )
    })
}
```

- [ ] **Step 6: Implement `set_alt.rs`**

```rust
use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::state::Store;

pub const ADDRESS_LOOKUP_TABLE_PROGRAM_ID: Pubkey =
    pubkey!("AddressLookupTab1e1111111111111111111111111");

/// Records the lookup table returned to the executor in
/// `lz_receive_types_v2`. Freeze the table before finalizing the program.
#[derive(Accounts)]
pub struct SetAlt<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(mut, address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: owner is validated
    #[account(owner = ADDRESS_LOOKUP_TABLE_PROGRAM_ID)]
    pub alt: UncheckedAccount<'info>,
}

pub fn set_alt(ctx: Context<SetAlt>) -> Result<()> {
    ctx.accounts.store.alt = ctx.accounts.alt.key();
    Ok(())
}
```

- [ ] **Step 7: Wire modules and program entrypoints**

`instructions/mod.rs` — add:

```rust
mod init;
mod init_path;
mod set_alt;
mod set_path_config;

pub use init::*;
pub use init_path::*;
pub use set_alt::*;
pub use set_path_config::*;
```

`lib.rs` inside `pub mod layerzero_prover`:

```rust
    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn init_path(ctx: Context<InitPath>, eid: u32) -> Result<()> {
        instructions::init_path(ctx, eid)
    }

    pub fn set_path_config(ctx: Context<SetPathConfig>, eid: u32, config: PathConfig) -> Result<()> {
        instructions::set_path_config(ctx, eid, config)
    }

    pub fn set_alt(ctx: Context<SetAlt>) -> Result<()> {
        instructions::set_alt(ctx)
    }
```

- [ ] **Step 8: Add the context builders**

Append to `integration-tests/tests/common/layerzero_prover_context.rs` (add imports `layerzero_prover::instructions::{InitArgs, PathConfig, ADDRESS_LOOKUP_TABLE_PROGRAM_ID}`, `layerzero_prover::layerzero::{ExecutorConfig, UlnConfig, ULN_ID, ENDPOINT_ID}`, `layerzero_prover::state::{LzReceiveTypesAccount, Store}`, `solana_sdk::account::Account`):

```rust
/// A fully pinned path config: two required DVNs (sorted, as ULN302 requires),
/// explicit confirmations and executor.
pub fn path_config() -> PathConfig {
    let mut dvns = vec![Pubkey::new_from_array([1; 32]), Pubkey::new_from_array([2; 32])];
    dvns.sort();
    let uln = UlnConfig {
        confirmations: 15,
        required_dvn_count: 2,
        optional_dvn_count: 0,
        optional_dvn_threshold: 0,
        required_dvns: dvns,
        optional_dvns: vec![],
    };

    PathConfig {
        send_uln: uln.clone(),
        receive_uln: uln,
        executor: ExecutorConfig {
            max_message_size: 10_000,
            executor: Pubkey::new_from_array([3; 32]),
        },
    }
}

impl LayerZeroProver<'_> {
    pub fn init(&mut self, authority: &Keypair, peers: Vec<Peer>) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::Init {
                payer: self.payer.pubkey(),
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                lz_receive_types: LzReceiveTypesAccount::pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                endpoint_event_authority: layerzero::endpoint_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::Init { args: InitArgs { peers } }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    pub fn init_path(&mut self, authority: &Keypair, peer: &Peer) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::InitPath {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                pda_payer: pda_payer_pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                nonce: layerzero::nonce_pda(&store, peer.eid, &peer.address).0,
                pending_nonce: layerzero::pending_nonce_pda(&store, peer.eid, &peer.address).0,
                send_library_config: layerzero::send_library_config_pda(&store, peer.eid).0,
                receive_library_config: layerzero::receive_library_config_pda(&store, peer.eid).0,
                message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
                endpoint_event_authority: layerzero::endpoint_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::InitPath { eid: peer.eid }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    pub fn set_path_config(&mut self, authority: &Keypair, eid: u32, config: PathConfig) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::SetPathConfig {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                pda_payer: pda_payer_pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
                uln_settings: layerzero::uln_settings_pda().0,
                uln_program: ULN_ID,
                uln_send_config: layerzero::uln_send_config_pda(eid, &store).0,
                uln_receive_config: layerzero::uln_receive_config_pda(eid, &store).0,
                uln_default_send_config: layerzero::uln_default_send_config_pda(eid).0,
                uln_default_receive_config: layerzero::uln_default_receive_config_pda(eid).0,
                uln_event_authority: layerzero::uln_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::SetPathConfig { eid, config }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    /// Stages an account owned by the lookup-table program (contents are not
    /// read on-chain; the executor reads the table off-chain).
    pub fn create_alt(&mut self) -> Pubkey {
        let alt = Pubkey::new_unique();
        self.set_account(
            alt,
            Account {
                lamports: 1_000_000_000,
                data: vec![0; 56],
                owner: ADDRESS_LOOKUP_TABLE_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        alt
    }

    pub fn set_alt(&mut self, authority: &Keypair, alt: Pubkey) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::SetAlt {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store: Store::pda().0,
                alt,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::SetAlt {}.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    /// install + init + every peer's path and config + ALT.
    pub fn setup(&mut self) -> Keypair {
        let authority = Keypair::new();
        self.install(authority.pubkey());
        self.init(&authority, peers()).unwrap();
        peers().iter().for_each(|peer| {
            self.init_path(&authority, peer).unwrap();
            self.set_path_config(&authority, peer.eid, path_config()).unwrap();
        });
        let alt = self.create_alt();
        self.set_alt(&authority, alt).unwrap();

        authority
    }
}
```

- [ ] **Step 9: Build and run the tests**

Run: `anchor build && cargo test --test init_layerzero_prover`
Expected: PASS (10 tests).

- [ ] **Step 10: Lint and commit**

Run: `cargo clippy --all-targets -- -D warnings && cargo +nightly fmt`

```bash
git add programs/layerzero-prover integration-tests/tests/common/layerzero_prover_context.rs integration-tests/tests/init_layerzero_prover.rs
git commit -m "feat(layerzero-prover): register OApp and pin paths behind the upgrade authority"
```

---

### Task 4: `prove` — outbound commit inside `portal::prove`

**Files:**
- Create: `programs/layerzero-prover/src/instructions/prove.rs`, `integration-tests/tests/prove_layerzero_prover.rs`
- Modify: `instructions/mod.rs`, `lib.rs`, `integration-tests/tests/common/layerzero_prover_context.rs`

**Interfaces:**
- Consumes: `state::{Store, PendingSend}`, `constants::MAX_INTENTS_PER_PROVE`, context `setup()`.
- Produces: instruction `prove(ProveArgs)` (handler `prove_intent`); `instructions::check_intent_count(usize) -> Result<()>` (reused by Task 5). Context: `payload(&self, &[Bytes32]) -> Vec<u8>`, `prove(&mut self, Vec<Bytes32>, dst_eid: u64, data: Vec<u8>) -> TransactionResult`, `pending_send_for(&self, dst_eid: u32, receiver: &Bytes32, hashes: &[Bytes32]) -> Pubkey`.

- [ ] **Step 1: Write the failing tests `integration-tests/tests/prove_layerzero_prover.rs`**

```rust
use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::prover::{IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::{Bytes32, CHAIN_ID};
use layerzero_prover::constants::MAX_INTENTS_PER_PROVE;
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::state::{PendingSend, Store};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{peers, BASE_EID};

pub mod common;

fn fulfilled(context: &mut common::Context, count: usize) -> Vec<Bytes32> {
    context
        .fulfill_rand_intents(count, layerzero_prover::ID)
        .iter()
        .map(|intent| intent.intent_hash)
        .collect()
}

#[test]
fn prove_commits_pending_send() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 2);
    let receiver = peers()[0].address;

    context
        .layerzero_prover()
        .prove(hashes.clone(), BASE_EID.into(), receiver.to_vec())
        .unwrap();

    let payload = context.layerzero_prover().payload(&hashes);
    let pending = context
        .account::<PendingSend>(&PendingSend::pda(BASE_EID, &receiver, &payload).0)
        .unwrap();
    assert_eq!(
        pending,
        PendingSend {
            dst_eid: BASE_EID,
            receiver,
            payload: payload.clone(),
            rent_payer: context.payer.pubkey(),
        }
    );
    assert_eq!(ProofData::from_bytes(&payload).unwrap().destination, CHAIN_ID);
}

#[test]
fn reproving_a_pending_batch_is_a_noop() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 2);
    let receiver = peers()[0].address;
    let address = context.layerzero_prover().pending_send_for(BASE_EID, &receiver, &hashes);
    context.layerzero_prover().prove(hashes.clone(), BASE_EID.into(), receiver.to_vec()).unwrap();
    let before = context.get_account(&address).unwrap();
    context.expire_blockhash();

    context.layerzero_prover().prove(hashes, BASE_EID.into(), receiver.to_vec()).unwrap();

    assert_eq!(context.get_account(&address).unwrap(), before);
}

#[test]
fn prove_rejects_unknown_eid_and_wrong_receiver() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 1);
    let receiver = peers()[0].address;

    let unknown = context.layerzero_prover().prove(hashes.clone(), 40_245, receiver.to_vec());
    assert!(unknown.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));

    let other_peer = peers()[1].address;
    let wrong = context.layerzero_prover().prove(hashes.clone(), BASE_EID.into(), other_peer.to_vec());
    assert!(wrong.is_err_and(common::is_error(LayerZeroProverError::InvalidReceiver)));

    let short = context.layerzero_prover().prove(hashes, BASE_EID.into(), receiver[..31].to_vec());
    assert!(short.is_err_and(common::is_error(LayerZeroProverError::InvalidData)));
}

#[test]
fn domain_id_above_u32_rejected() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, 1);
    let wrapped = u64::from(BASE_EID) + (1u64 << 32);

    let result = context
        .layerzero_prover()
        .prove(hashes, wrapped, peers()[0].address.to_vec());

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidDomainId)));
}

#[test]
fn prove_rejects_batch_over_cap() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes = fulfilled(&mut context, MAX_INTENTS_PER_PROVE + 1);

    let result = context
        .layerzero_prover()
        .prove(hashes, BASE_EID.into(), peers()[0].address.to_vec());

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::TooManyIntents)));
}

/// Without portal's dispatcher signature `prove` is unreachable.
#[test]
fn prove_rejects_non_portal_caller() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let impostor = Keypair::new();
    let receiver = peers()[0].address;
    let proof_data = ProofData::new(CHAIN_ID, vec![IntentHashClaimant::new([1; 32].into(), [2; 32].into())]);
    let pending = PendingSend::pda(BASE_EID, &receiver, &proof_data.clone().to_bytes()).0;
    let instruction = Instruction {
        program_id: layerzero_prover::ID,
        accounts: layerzero_prover::accounts::Prove {
            portal_dispatcher: impostor.pubkey(),
            payer: context.payer.pubkey(),
            store: Store::pda().0,
            pending_send: pending,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: layerzero_prover::instruction::Prove {
            args: ProveArgs::new(BASE_EID.into(), proof_data, receiver.to_vec()),
        }
        .data(),
    };

    let result = context.layerzero_prover().send(vec![instruction], &[&impostor]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidPortalDispatcher)));
}
```

Add to `programs/layerzero-prover/src/instructions/prove.rs` (below) a unit test module for `check_intent_count` (portal rejects empty batches first, so `EmptyProof` is only reachable as defence in depth):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_intent_count_bounds() {
        assert!(check_intent_count(0).is_err());
        assert!(check_intent_count(1).is_ok());
        assert!(check_intent_count(MAX_INTENTS_PER_PROVE).is_ok());
        assert!(check_intent_count(MAX_INTENTS_PER_PROVE + 1).is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test prove_layerzero_prover`
Expected: FAIL to compile (`accounts::Prove`, context `prove` not found).

- [ ] **Step 3: Implement `prove.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::ProveArgs;
use eco_svm_std::Bytes32;

use crate::constants::MAX_INTENTS_PER_PROVE;
use crate::instructions::LayerZeroProverError;
use crate::state::{PendingSend, Store, PENDING_SEND_SEED};

#[derive(Accounts)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda(&crate::ID).0 @ LayerZeroProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: derived from the batch and validated in the handler
    #[account(mut)]
    pub pending_send: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn check_intent_count(count: usize) -> Result<()> {
    require!(count > 0, LayerZeroProverError::EmptyProof);
    require!(
        count <= MAX_INTENTS_PER_PROVE,
        LayerZeroProverError::TooManyIntents
    );

    Ok(())
}

/// Commits the batch; `send_message` dispatches it as a top-level
/// instruction because `endpoint::send` nests four frames below its caller and
/// `portal::prove` → here already uses two.
pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        domain_id,
        proof_data,
        data,
    } = args;

    let dst_eid: u32 = domain_id
        .try_into()
        .map_err(|_| LayerZeroProverError::InvalidDomainId)?;
    let peer = *ctx
        .accounts
        .store
        .peer(dst_eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    let receiver: Bytes32 = <[u8; 32]>::try_from(data)
        .map_err(|_| LayerZeroProverError::InvalidData)?
        .into();
    require!(receiver == peer.address, LayerZeroProverError::InvalidReceiver);
    check_intent_count(proof_data.intent_hashes_claimants.len())?;

    let payload = proof_data.to_bytes();
    let (address, bump) = PendingSend::pda(dst_eid, &receiver, &payload);
    require_keys_eq!(
        ctx.accounts.pending_send.key(),
        address,
        LayerZeroProverError::InvalidPendingSend
    );
    // The address commits to (dst_eid, receiver, payload): an account already
    // there is this same batch, still waiting for `send_message`.
    if ctx.accounts.pending_send.owner == &crate::ID {
        return Ok(());
    }

    let key = PendingSend::key(dst_eid, &receiver, &payload);
    PendingSend {
        dst_eid,
        receiver,
        payload,
        rent_payer: ctx.accounts.payer.key(),
    }
    .init(
        &ctx.accounts.pending_send,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[PENDING_SEND_SEED, &key, &[bump]]],
    )
}
```

(Append the Step 1 `#[cfg(test)]` module to this file.)

- [ ] **Step 4: Wire it**

`instructions/mod.rs`: `mod prove;` / `pub use prove::*;`. `lib.rs`:

```rust
    pub fn prove(ctx: Context<Prove>, args: eco_svm_std::prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }
```

- [ ] **Step 5: Add the context helpers**

Append to `layerzero_prover_context.rs` (imports: `eco_svm_std::prover::{IntentHashClaimant, ProofData}`, `eco_svm_std::CHAIN_ID`, `portal::state::FulfillMarker`, `layerzero_prover::state::PendingSend`, `solana_sdk::instruction::AccountMeta`):

```rust
impl LayerZeroProver<'_> {
    /// The bytes portal hands `prove` for these fulfilled intents.
    pub fn payload(&self, intent_hashes: &[Bytes32]) -> Vec<u8> {
        let pairs = intent_hashes
            .iter()
            .map(|hash| {
                let marker = self
                    .account::<FulfillMarker>(&FulfillMarker::pda(hash).0)
                    .unwrap();
                IntentHashClaimant::new(*hash, marker.claimant)
            })
            .collect();

        ProofData::new(CHAIN_ID, pairs).to_bytes()
    }

    pub fn pending_send_for(&self, dst_eid: u32, receiver: &Bytes32, intent_hashes: &[Bytes32]) -> Pubkey {
        PendingSend::pda(dst_eid, receiver, &self.payload(intent_hashes)).0
    }

    /// `portal::prove` targeting this prover. `data` is normally the 32-byte
    /// EVM receiver; tests pass malformed data on purpose.
    pub fn prove(&mut self, intent_hashes: Vec<Bytes32>, dst_eid: u64, data: Vec<u8>) -> TransactionResult {
        let fulfill_markers = intent_hashes
            .iter()
            .map(|hash| FulfillMarker::pda(hash).0)
            .collect();
        let receiver: Bytes32 = <[u8; 32]>::try_from(data.as_slice())
            .unwrap_or([0; 32])
            .into();
        let pending = self.pending_send_for(dst_eid as u32, &receiver, &intent_hashes);
        let payer = self.payer.pubkey();

        self.portal().prove_intent_via_program_with_compute_limit(
            layerzero_prover::ID,
            intent_hashes,
            dst_eid,
            fulfill_markers,
            portal::state::dispatcher_pda(&layerzero_prover::ID).0,
            data,
            vec![
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(Store::pda().0, false),
                AccountMeta::new(pending, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
            COMPUTE_UNIT_LIMIT,
        )
    }
}
```

- [ ] **Step 6: Build and run**

Run: `anchor build && cargo test --package layerzero-prover check_intent_count && cargo test --test prove_layerzero_prover`
Expected: PASS.

- [ ] **Step 7: Lint and commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo +nightly fmt
git add programs/layerzero-prover integration-tests/tests/common/layerzero_prover_context.rs integration-tests/tests/prove_layerzero_prover.rs
git commit -m "feat(layerzero-prover): commit outbound batches from portal::prove"
```

---

### Task 5: `send_message` and `quote_message`

**Files:**
- Create: `programs/layerzero-prover/src/instructions/{send_message,quote_message}.rs`, `integration-tests/tests/send_layerzero_prover.rs`
- Modify: `instructions/mod.rs`, `lib.rs`, `layerzero_prover_context.rs`

**Interfaces:**
- Consumes: `PendingSend`, `check_intent_count`, `lz_receive_options`, `lz_receive_gas`, Task 4 context `prove`.
- Produces: instructions `send_message(max_native_fee: u64)`, `quote_message(QuoteMessageArgs { dst_eid: u32, receiver: Bytes32, payload: Vec<u8> }) -> MessagingFee`. Context: `send_accounts(store, payer, dst_eid, &receiver) -> Vec<AccountMeta>`, `quote_accounts(store, dst_eid, &receiver) -> Vec<AccountMeta>`, free fn `build_send_message_instruction(pending_send, rent_payer, fee_payer: Pubkey, dst_eid, &receiver, max_native_fee) -> Instruction`, `send_message(&mut self, pending_send, rent_payer, fee_payer: &Keypair, dst_eid, &receiver, max_native_fee) -> TransactionResult`, `quote_message(&mut self, QuoteMessageArgs) -> Result<MessagingFee, _>`.

- [ ] **Step 1: Write the failing tests `integration-tests/tests/send_layerzero_prover.rs`**

```rust
use anchor_lang::error::ErrorCode;
use eco_svm_std::Bytes32;
use layerzero_prover::constants::lz_receive_gas;
use layerzero_prover::instructions::{LayerZeroProverError, QuoteMessageArgs};
use layerzero_prover::layerzero::{lz_receive_options, MessagingFee};
use layerzero_prover::state::{PendingSend, Store};
use mock_layerzero_endpoint::{MockEndpointError, MockPacketSent, MOCK_NATIVE_FEE};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{peers, BASE_EID, TREASURY};

pub mod common;

/// Two fulfilled intents proven to Base; returns (context, pending, payload, fee payer).
fn committed() -> (common::Context, Pubkey, Vec<u8>, Keypair) {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes: Vec<Bytes32> = context
        .fulfill_rand_intents(2, layerzero_prover::ID)
        .iter()
        .map(|intent| intent.intent_hash)
        .collect();
    let receiver = peers()[0].address;
    context.layerzero_prover().prove(hashes.clone(), BASE_EID.into(), receiver.to_vec()).unwrap();
    let payload = context.layerzero_prover().payload(&hashes);
    let pending = PendingSend::pda(BASE_EID, &receiver, &payload).0;
    let fee_payer = Keypair::new();
    context.airdrop(&fee_payer.pubkey(), 1_000_000_000).unwrap();

    (context, pending, payload, fee_payer)
}

#[test]
fn send_message_dispatches_with_floor_options_and_refunds_commit_rent() {
    let (mut context, pending, payload, fee_payer) = committed();
    let rent_payer = context.payer.pubkey();
    let receiver = peers()[0].address;
    let commit_rent = context.get_account(&pending).unwrap().lamports;
    let rent_payer_before = context.balance(&rent_payer);

    let result = context
        .layerzero_prover()
        .send_message(pending, rent_payer, &fee_payer, BASE_EID, &receiver, MOCK_NATIVE_FEE)
        .unwrap();

    assert!(common::contains_event(MockPacketSent {
        sender: Store::pda().0,
        dst_eid: BASE_EID,
        receiver: receiver.into(),
        message: payload,
        options: lz_receive_options(lz_receive_gas(2)),
        native_fee: MOCK_NATIVE_FEE,
        nonce: 1,
    })(result));
    assert!(context.get_account(&pending).is_none());
    assert_eq!(context.balance(&rent_payer), rent_payer_before + commit_rent);
    assert_eq!(context.balance(&TREASURY), MOCK_NATIVE_FEE);
}

#[test]
fn second_send_fails_once_commit_closed() {
    let (mut context, pending, _, fee_payer) = committed();
    let rent_payer = context.payer.pubkey();
    let receiver = peers()[0].address;
    context
        .layerzero_prover()
        .send_message(pending, rent_payer, &fee_payer, BASE_EID, &receiver, MOCK_NATIVE_FEE)
        .unwrap();
    context.expire_blockhash();

    let result = context
        .layerzero_prover()
        .send_message(pending, rent_payer, &fee_payer, BASE_EID, &receiver, MOCK_NATIVE_FEE);

    assert!(result.is_err_and(common::is_error(ErrorCode::AccountNotInitialized)));
    assert_eq!(context.balance(&TREASURY), MOCK_NATIVE_FEE);
}

#[test]
fn send_message_rejects_foreign_rent_payer() {
    let (mut context, pending, _, fee_payer) = committed();
    let receiver = peers()[0].address;

    let result = context.layerzero_prover().send_message(
        pending,
        fee_payer.pubkey(),
        &fee_payer,
        BASE_EID,
        &receiver,
        MOCK_NATIVE_FEE,
    );

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidRentPayer)));
}

#[test]
fn short_max_fee_fails_and_keeps_commit() {
    let (mut context, pending, _, fee_payer) = committed();
    let rent_payer = context.payer.pubkey();
    let receiver = peers()[0].address;

    let result = context.layerzero_prover().send_message(
        pending,
        rent_payer,
        &fee_payer,
        BASE_EID,
        &receiver,
        MOCK_NATIVE_FEE - 1,
    );

    assert!(result.is_err_and(common::is_error(MockEndpointError::InsufficientFee)));
    assert!(context.get_account(&pending).is_some());
}

#[test]
fn quote_message_returns_endpoint_fee() {
    let (mut context, _, payload, _) = committed();

    let fee = context
        .layerzero_prover()
        .quote_message(QuoteMessageArgs {
            dst_eid: BASE_EID,
            receiver: peers()[0].address,
            payload,
        })
        .unwrap();

    assert_eq!(fee, MessagingFee { native_fee: MOCK_NATIVE_FEE, lz_token_fee: 0 });
}

#[test]
fn quote_message_rejects_unknown_peer_and_wrong_receiver() {
    let (mut context, _, payload, _) = committed();

    let unknown = context.layerzero_prover().quote_message(QuoteMessageArgs {
        dst_eid: 40_245,
        receiver: peers()[0].address,
        payload: payload.clone(),
    });
    assert!(unknown.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));

    let wrong = context.layerzero_prover().quote_message(QuoteMessageArgs {
        dst_eid: BASE_EID,
        receiver: peers()[1].address,
        payload,
    });
    assert!(wrong.is_err_and(common::is_error(LayerZeroProverError::InvalidReceiver)));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test send_layerzero_prover`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `send_message.rs`**

```rust
use anchor_lang::prelude::*;

use crate::constants::lz_receive_gas;
use crate::instructions::LayerZeroProverError;
use crate::layerzero::{self, lz_receive_options, SendParams, ENDPOINT_ID, SEND_DISCRIMINATOR};
use crate::state::{PendingSend, Store, STORE_SEED};

/// Permissionless: only portal's dispatcher can create a `PendingSend`, so this
/// can only ever send a portal-attested batch to its configured peer, with
/// options computed here. Remaining accounts are the endpoint `send` accounts
/// after `[program, sender]`: send library program, send library config,
/// default send library config, send library info (read-only), endpoint
/// settings, nonce (mut), endpoint event authority, endpoint program, then the
/// ULN302 send accounts (payer = the fee payer, signer) and worker accounts.
#[derive(Accounts)]
pub struct SendMessage<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    #[account(mut, close = rent_payer, has_one = rent_payer @ LayerZeroProverError::InvalidRentPayer)]
    pub pending_send: Account<'info, PendingSend>,
    /// CHECK: must equal `pending_send.rent_payer`
    #[account(mut)]
    pub rent_payer: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
}

pub fn send_message<'info>(
    ctx: Context<'info, SendMessage<'info>>,
    max_native_fee: u64,
) -> Result<()> {
    let pending = &ctx.accounts.pending_send;
    let params = SendParams {
        dst_eid: pending.dst_eid,
        receiver: pending.receiver.into(),
        message: pending.payload.clone(),
        options: lz_receive_options(lz_receive_gas(pending.intent_count())),
        native_fee: max_native_fee,
        lz_token_fee: 0,
    };
    let (store, bump) = Store::pda();
    let accounts: Vec<AccountInfo<'info>> = [
        ctx.accounts.endpoint_program.to_account_info(),
        ctx.accounts.store.to_account_info(),
    ]
    .into_iter()
    .chain(ctx.remaining_accounts.iter().cloned())
    .collect();

    layerzero::invoke(
        ENDPOINT_ID,
        SEND_DISCRIMINATOR,
        &params,
        &accounts,
        &[store],
        &[&[STORE_SEED, &[bump]]],
    )
}
```

- [ ] **Step 4: Implement `quote_message.rs`**

```rust
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::get_return_data;
use eco_svm_std::prover::ProofData;
use eco_svm_std::Bytes32;

use crate::constants::lz_receive_gas;
use crate::instructions::{check_intent_count, LayerZeroProverError};
use crate::layerzero::{self, lz_receive_options, MessagingFee, QuoteParams, ENDPOINT_ID, QUOTE_DISCRIMINATOR};
use crate::state::Store;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct QuoteMessageArgs {
    pub dst_eid: u32,
    pub receiver: Bytes32,
    pub payload: Vec<u8>,
}

/// Read-only fee quote for a batch with the exact options `send_message`
/// will use; run it with `simulateTransaction` and read the return data.
/// Remaining accounts are the endpoint `quote` accounts (send library program,
/// send library config, default, send library info, endpoint settings, nonce)
/// and the ULN302 quote/worker accounts — all read-only.
#[derive(Accounts)]
pub struct QuoteMessage<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: address is validated
    #[account(address = ENDPOINT_ID @ LayerZeroProverError::InvalidEndpoint)]
    pub endpoint_program: UncheckedAccount<'info>,
}

pub fn quote_message<'info>(
    ctx: Context<'info, QuoteMessage<'info>>,
    args: QuoteMessageArgs,
) -> Result<MessagingFee> {
    let peer = *ctx
        .accounts
        .store
        .peer(args.dst_eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    require!(args.receiver == peer.address, LayerZeroProverError::InvalidReceiver);
    let intents = ProofData::from_bytes(&args.payload)?.intent_hashes_claimants.len();
    check_intent_count(intents)?;

    let params = QuoteParams {
        sender: ctx.accounts.store.key(),
        dst_eid: args.dst_eid,
        receiver: args.receiver.into(),
        message: args.payload,
        options: lz_receive_options(lz_receive_gas(intents)),
        pay_in_lz_token: false,
    };
    let accounts: Vec<AccountInfo<'info>> = std::iter::once(ctx.accounts.endpoint_program.to_account_info())
        .chain(ctx.remaining_accounts.iter().cloned())
        .collect();
    layerzero::invoke(ENDPOINT_ID, QUOTE_DISCRIMINATOR, &params, &accounts, &[], &[])?;

    let (program, data) = get_return_data().ok_or(LayerZeroProverError::InvalidQuote)?;
    require_keys_eq!(program, ENDPOINT_ID, LayerZeroProverError::InvalidQuote);
    MessagingFee::try_from_slice(&data).map_err(|_| LayerZeroProverError::InvalidQuote.into())
}
```

- [ ] **Step 5: Wire it**

`instructions/mod.rs`: `mod quote_message; mod send_message;` and matching `pub use`. `lib.rs`:

```rust
    pub fn send_message<'info>(ctx: Context<'info, SendMessage<'info>>, max_native_fee: u64) -> Result<()> {
        instructions::send_message(ctx, max_native_fee)
    }

    pub fn quote_message<'info>(
        ctx: Context<'info, QuoteMessage<'info>>,
        args: QuoteMessageArgs,
    ) -> Result<layerzero::MessagingFee> {
        instructions::quote_message(ctx, args)
    }
```

- [ ] **Step 6: Add the context helpers**

```rust
/// Endpoint `send` accounts after `[program, sender]`, then the ULN302 send
/// tail. Worker (executor/DVN) accounts are omitted: the mock ignores them.
pub fn send_accounts(store: Pubkey, payer: Pubkey, dst_eid: u32, receiver: &Bytes32) -> Vec<AccountMeta> {
    let uln = layerzero::uln_settings_pda().0;
    vec![
        AccountMeta::new_readonly(ULN_ID, false),
        AccountMeta::new_readonly(layerzero::send_library_config_pda(&store, dst_eid).0, false),
        AccountMeta::new_readonly(layerzero::default_send_library_config_pda(dst_eid).0, false),
        AccountMeta::new_readonly(layerzero::message_lib_info_pda(&uln).0, false),
        AccountMeta::new_readonly(layerzero::endpoint_settings_pda().0, false),
        AccountMeta::new(layerzero::nonce_pda(&store, dst_eid, receiver).0, false),
        AccountMeta::new_readonly(layerzero::endpoint_event_authority().0, false),
        AccountMeta::new_readonly(ENDPOINT_ID, false),
        AccountMeta::new_readonly(uln, false),
        AccountMeta::new_readonly(layerzero::uln_send_config_pda(dst_eid, &store).0, false),
        AccountMeta::new_readonly(layerzero::uln_default_send_config_pda(dst_eid).0, false),
        AccountMeta::new(payer, true),
        AccountMeta::new(TREASURY, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(layerzero::uln_event_authority().0, false),
        AccountMeta::new_readonly(ULN_ID, false),
    ]
}

/// Endpoint `quote` accounts plus the ULN302 quote head, all read-only.
pub fn quote_accounts(store: Pubkey, dst_eid: u32, receiver: &Bytes32) -> Vec<AccountMeta> {
    let uln = layerzero::uln_settings_pda().0;
    vec![
        AccountMeta::new_readonly(ULN_ID, false),
        AccountMeta::new_readonly(layerzero::send_library_config_pda(&store, dst_eid).0, false),
        AccountMeta::new_readonly(layerzero::default_send_library_config_pda(dst_eid).0, false),
        AccountMeta::new_readonly(layerzero::message_lib_info_pda(&uln).0, false),
        AccountMeta::new_readonly(layerzero::endpoint_settings_pda().0, false),
        AccountMeta::new_readonly(layerzero::nonce_pda(&store, dst_eid, receiver).0, false),
        AccountMeta::new_readonly(uln, false),
        AccountMeta::new_readonly(layerzero::uln_send_config_pda(dst_eid, &store).0, false),
        AccountMeta::new_readonly(layerzero::uln_default_send_config_pda(dst_eid).0, false),
    ]
}

/// Free function (no context needed) so the batch-size tests can build it too.
pub fn build_send_message_instruction(
    pending_send: Pubkey,
    rent_payer: Pubkey,
    fee_payer: Pubkey,
    dst_eid: u32,
    receiver: &Bytes32,
    max_native_fee: u64,
) -> Instruction {
    let store = Store::pda().0;
    let accounts = layerzero_prover::accounts::SendMessage {
        payer: fee_payer,
        store,
        pending_send,
        rent_payer,
        endpoint_program: ENDPOINT_ID,
    }
    .to_account_metas(None)
    .into_iter()
    .chain(send_accounts(store, fee_payer, dst_eid, receiver))
    .collect();

    Instruction {
        program_id: layerzero_prover::ID,
        accounts,
        data: layerzero_prover::instruction::SendMessage { max_native_fee }.data(),
    }
}

impl LayerZeroProver<'_> {
    #[allow(clippy::too_many_arguments)]
    pub fn send_message(
        &mut self,
        pending_send: Pubkey,
        rent_payer: Pubkey,
        fee_payer: &Keypair,
        dst_eid: u32,
        receiver: &Bytes32,
        max_native_fee: u64,
    ) -> TransactionResult {
        let instruction = build_send_message_instruction(
            pending_send,
            rent_payer,
            fee_payer.pubkey(),
            dst_eid,
            receiver,
            max_native_fee,
        );
        let instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
            instruction,
        ];
        let transaction = Transaction::new(
            &[fee_payer],
            Message::new(&instructions, Some(&fee_payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn quote_message(
        &mut self,
        args: QuoteMessageArgs,
    ) -> Result<MessagingFee, Box<litesvm::types::FailedTransactionMetadata>> {
        let store = Store::pda().0;
        let accounts = layerzero_prover::accounts::QuoteMessage {
            store,
            endpoint_program: ENDPOINT_ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(quote_accounts(store, args.dst_eid, &args.receiver))
        .collect();
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts,
            data: layerzero_prover::instruction::QuoteMessage { args }.data(),
        };
        let result = self.send(vec![instruction], &[])?;

        Ok(MessagingFee::try_from_slice(&result.return_data.data).unwrap())
    }
}
```

(Imports to add: `anchor_lang::AnchorDeserialize`, `layerzero_prover::instructions::QuoteMessageArgs`, `layerzero_prover::layerzero::MessagingFee`.)

- [ ] **Step 7: Build and run**

Run: `anchor build && cargo test --test send_layerzero_prover`
Expected: PASS (6 tests).

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo +nightly fmt
git add programs/layerzero-prover integration-tests/tests/common/layerzero_prover_context.rs integration-tests/tests/send_layerzero_prover.rs
git commit -m "feat(layerzero-prover): dispatch committed batches through endpoint send and quote fees"
```

---

### Task 6: Executor discovery — `lz_receive_types_info`, `lz_receive_types_v2`

**Files:**
- Create: `programs/layerzero-prover/src/instructions/lz_receive_types.rs`, `integration-tests/tests/lz_receive_types_layerzero_prover.rs`
- Modify: `instructions/mod.rs`, `lib.rs`, `layerzero_prover_context.rs`

**Interfaces:**
- Consumes: `Store`, `LzReceiveTypesAccount`, mirrored V2 types and PDAs.
- Produces: instructions `lz_receive_types_info(LzReceiveParams) -> LzReceiveTypesInfoResult`, `lz_receive_types_v2(LzReceiveParams) -> LzReceiveTypesV2Result`; `instructions::lz_receive_accounts(&LzReceiveParams, &ProofData) -> Vec<AccountMetaRef>` (the exact list `lz_receive` validates — Task 7 relies on it); const `instructions::CLEAR_ACCOUNTS_LEN = 8`. Context: `receive_params(&Peer, nonce: u64, ProofData) -> LzReceiveParams`, `lz_receive_types_info(&mut self, LzReceiveParams) -> TransactionResult`, `lz_receive_types_v2(&mut self, LzReceiveParams) -> TransactionResult`.

- [ ] **Step 1: Write the failing tests `integration-tests/tests/lz_receive_types_layerzero_prover.rs`**

```rust
use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use layerzero_prover::instructions::{lz_receive_accounts, LayerZeroProverError};
use layerzero_prover::layerzero::{
    LzInstruction, LzReceiveTypesInfoResult, LzReceiveTypesV2Accounts, LzReceiveTypesV2Result,
    EXECUTION_CONTEXT_VERSION_1, LZ_RECEIVE_TYPES_VERSION,
};
use layerzero_prover::state::Store;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{peers, receive_params, BASE_CHAIN_ID};

pub mod common;

fn proof_data() -> ProofData {
    ProofData::new(
        BASE_CHAIN_ID,
        vec![
            IntentHashClaimant::new([1; 32].into(), [2; 32].into()),
            IntentHashClaimant::new([3; 32].into(), [4; 32].into()),
        ],
    )
}

#[test]
fn info_returns_version_two_and_store() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();

    let result = context
        .layerzero_prover()
        .lz_receive_types_info(receive_params(&peers()[0], 1, proof_data()))
        .unwrap();

    assert_eq!(
        LzReceiveTypesInfoResult::try_from_slice(&result.return_data.data).unwrap(),
        LzReceiveTypesInfoResult {
            version: LZ_RECEIVE_TYPES_VERSION,
            accounts: LzReceiveTypesV2Accounts { accounts: vec![Store::pda().0] },
        }
    );
}

#[test]
fn v2_returns_alt_and_exact_lz_receive_accounts() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let params = receive_params(&peers()[0], 1, proof_data());
    let alt = context.account::<Store>(&Store::pda().0).unwrap().alt;

    let result = context.layerzero_prover().lz_receive_types_v2(params.clone()).unwrap();

    let returned = LzReceiveTypesV2Result::try_from_slice(&result.return_data.data).unwrap();
    assert_eq!(
        returned,
        LzReceiveTypesV2Result {
            context_version: EXECUTION_CONTEXT_VERSION_1,
            alts: vec![alt],
            instructions: vec![LzInstruction::LzReceive {
                accounts: lz_receive_accounts(&params, &proof_data()),
            }],
        }
    );
    // 13 fixed accounts + one Proof PDA per pair.
    let LzInstruction::LzReceive { accounts } = &returned.instructions[0] else { panic!() };
    assert_eq!(accounts.len(), 13 + 2);
}

#[test]
fn v2_requires_alt() {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    context.layerzero_prover().init(&authority, peers()).unwrap();

    let result = context
        .layerzero_prover()
        .lz_receive_types_v2(receive_params(&peers()[0], 1, proof_data()));

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::AltNotSet)));
}

#[test]
fn v2_rejects_malformed_message() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let mut params = receive_params(&peers()[0], 1, proof_data());
    params.message.push(0);

    assert!(context.layerzero_prover().lz_receive_types_v2(params).is_err());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test lz_receive_types_layerzero_prover`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `lz_receive_types.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::event_authority_pda;
use eco_svm_std::prover::{Proof, ProofData};

use crate::instructions::LayerZeroProverError;
use crate::layerzero::{
    endpoint_event_authority, endpoint_settings_pda, nonce_pda, oapp_registry_pda, payload_hash_pda,
    AccountMetaRef, AddressLocator, LzInstruction, LzReceiveParams, LzReceiveTypesInfoResult,
    LzReceiveTypesV2Accounts, LzReceiveTypesV2Result, ENDPOINT_ID, EXECUTION_CONTEXT_VERSION_1,
    LZ_RECEIVE_TYPES_VERSION,
};
use crate::state::{pda_payer_pda, LzReceiveTypesAccount, Store};

/// Accounts `endpoint::clear` takes, as the head of `lz_receive`'s remaining
/// accounts: endpoint program, receiver (store), OApp registry, nonce, payload
/// hash (mut), endpoint settings (mut), endpoint event authority, endpoint program.
pub const CLEAR_ACCOUNTS_LEN: usize = 8;

/// Executor entry point 1: names the accounts `lz_receive_types_v2` takes.
#[derive(Accounts)]
pub struct LzReceiveTypesInfo<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    #[account(address = LzReceiveTypesAccount::pda().0 @ LayerZeroProverError::InvalidLzReceiveTypes)]
    pub lz_receive_types: Account<'info, LzReceiveTypesAccount>,
}

pub fn lz_receive_types_info(
    ctx: Context<LzReceiveTypesInfo>,
    _params: LzReceiveParams,
) -> Result<LzReceiveTypesInfoResult> {
    Ok(LzReceiveTypesInfoResult {
        version: LZ_RECEIVE_TYPES_VERSION,
        accounts: LzReceiveTypesV2Accounts {
            accounts: vec![ctx.accounts.store.key()],
        },
    })
}

/// Executor entry point 2 (simulated): the exact `lz_receive` instruction to
/// build for this message. Must agree with what `lz_receive` validates, or the
/// executor halts — both use [`lz_receive_accounts`].
#[derive(Accounts)]
pub struct LzReceiveTypesV2<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
}

pub fn lz_receive_types_v2(
    ctx: Context<LzReceiveTypesV2>,
    params: LzReceiveParams,
) -> Result<LzReceiveTypesV2Result> {
    let alt = ctx.accounts.store.alt;
    require!(alt != Pubkey::default(), LayerZeroProverError::AltNotSet);
    let proof_data = ProofData::from_bytes(&params.message)?;

    Ok(LzReceiveTypesV2Result {
        context_version: EXECUTION_CONTEXT_VERSION_1,
        alts: vec![alt],
        instructions: vec![LzInstruction::LzReceive {
            accounts: lz_receive_accounts(&params, &proof_data),
        }],
    })
}

/// `lz_receive`'s account list in order: the named accounts (store,
/// pda_payer, system program, then event_cpi's event authority and program),
/// the [`CLEAR_ACCOUNTS_LEN`] `clear` accounts, then one `Proof` PDA per pair.
pub fn lz_receive_accounts(params: &LzReceiveParams, proof_data: &ProofData) -> Vec<AccountMetaRef> {
    let store = Store::pda().0;
    let fixed = [
        (store, false),
        (pda_payer_pda().0, true),
        (anchor_lang::system_program::ID, false),
        (event_authority_pda(&crate::ID).0, false),
        (crate::ID, false),
        (ENDPOINT_ID, false),
        (store, false),
        (oapp_registry_pda(&store).0, false),
        (nonce_pda(&store, params.src_eid, &params.sender).0, false),
        (payload_hash_pda(&store, params.src_eid, &params.sender, params.nonce).0, true),
        (endpoint_settings_pda().0, true),
        (endpoint_event_authority().0, false),
        (ENDPOINT_ID, false),
    ];
    let proofs = proof_data
        .intent_hashes_claimants
        .iter()
        .map(|pair| (Proof::pda(&pair.intent_hash, &crate::ID).0, true));

    fixed
        .into_iter()
        .chain(proofs)
        .map(|(pubkey, is_writable)| AccountMetaRef {
            pubkey: AddressLocator::Address(pubkey),
            is_writable,
        })
        .collect()
}
```

- [ ] **Step 4: Wire it**

`instructions/mod.rs`: `mod lz_receive_types; pub use lz_receive_types::*;`. `lib.rs` (add `use layerzero::{LzReceiveParams, LzReceiveTypesInfoResult, LzReceiveTypesV2Result};`):

```rust
    pub fn lz_receive_types_info(
        ctx: Context<LzReceiveTypesInfo>,
        params: LzReceiveParams,
    ) -> Result<LzReceiveTypesInfoResult> {
        instructions::lz_receive_types_info(ctx, params)
    }

    pub fn lz_receive_types_v2(
        ctx: Context<LzReceiveTypesV2>,
        params: LzReceiveParams,
    ) -> Result<LzReceiveTypesV2Result> {
        instructions::lz_receive_types_v2(ctx, params)
    }
```

- [ ] **Step 5: Add the context helpers**

```rust
/// A delivery of `proof_data` from `peer` at `nonce` (guid derived from nonce).
pub fn receive_params(peer: &Peer, nonce: u64, proof_data: ProofData) -> LzReceiveParams {
    LzReceiveParams {
        src_eid: peer.eid,
        sender: peer.address.into(),
        nonce,
        guid: [nonce as u8; 32],
        message: proof_data.to_bytes(),
        extra_data: vec![],
    }
}

impl LayerZeroProver<'_> {
    pub fn lz_receive_types_info(&mut self, params: LzReceiveParams) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::LzReceiveTypesInfo {
                store: Store::pda().0,
                lz_receive_types: LzReceiveTypesAccount::pda().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::LzReceiveTypesInfo { params }.data(),
        };
        self.send(vec![instruction], &[])
    }

    pub fn lz_receive_types_v2(&mut self, params: LzReceiveParams) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::LzReceiveTypesV2 { store: Store::pda().0 }
                .to_account_metas(None),
            data: layerzero_prover::instruction::LzReceiveTypesV2 { params }.data(),
        };
        self.send(vec![instruction], &[])
    }
}
```

(Import `layerzero_prover::layerzero::LzReceiveParams`.)

- [ ] **Step 6: Build and run, lint, commit**

Run: `anchor build && cargo test --test lz_receive_types_layerzero_prover`
Expected: PASS (4 tests).

```bash
cargo clippy --all-targets -- -D warnings && cargo +nightly fmt
git add programs/layerzero-prover integration-tests/tests/common/layerzero_prover_context.rs integration-tests/tests/lz_receive_types_layerzero_prover.rs
git commit -m "feat(layerzero-prover): expose executor V2 receive-types discovery"
```

---

### Task 7: `lz_receive` — inbound proofs, end to end

**Files:**
- Create: `programs/layerzero-prover/src/instructions/lz_receive.rs`, `integration-tests/tests/lz_receive_layerzero_prover.rs`
- Modify: `instructions/mod.rs`, `lib.rs`, `layerzero_prover_context.rs`, `integration-tests/tests/close_proof_layerzero_prover.rs`

**Interfaces:**
- Consumes: `lz_receive_accounts`, `CLEAR_ACCOUNTS_LEN`, mock `mock_verify`, `Nonce`, aggregator context (`install`, `init`, `aggregate`), portal `withdraw_intent`, `refund_intent_with_close_proof`.
- Produces: instruction `lz_receive(LzReceiveParams)`. Context: `verify(&mut self, &LzReceiveParams) -> TransactionResult`, free fn `build_lz_receive_instruction(&LzReceiveParams, Vec<AccountMetaRef>) -> Instruction` (also as method `lz_receive_instruction`), `lz_receive(&mut self, &LzReceiveParams) -> TransactionResult`, `deliver(&mut self, &LzReceiveParams) -> TransactionResult` (verify + lz_receive), `force_nonce(&mut self, src_eid: u32, sender: [u8; 32])`.

- [ ] **Step 1: Pin the `lz_receive` discriminator (add to `close_proof_layerzero_prover.rs`)**

```rust
#[test]
fn lz_receive_discriminator_matches_executor_constant() {
    assert_eq!(
        layerzero_prover::instruction::LzReceive::DISCRIMINATOR,
        layerzero::LZ_RECEIVE_DISCRIMINATOR.as_slice()
    );
    // Endpoint `send` is mocked under a custom name with LayerZero's selector.
    assert_eq!(
        mock_layerzero_endpoint::instruction::SendPacket::DISCRIMINATOR,
        layerzero::SEND_DISCRIMINATOR.as_slice()
    );
}
```

- [ ] **Step 2: Write the failing tests `integration-tests/tests/lz_receive_layerzero_prover.rs`**

```rust
use std::iter;

use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{IntentHashClaimant, IntentProven, Proof, ProofData};
use eco_svm_std::{Bytes32, CANCELLED};
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::layerzero::{self, LzInstruction, LzReceiveTypesV2Result};
use layerzero_prover::state::{pda_payer_pda, ProofAccount, Store};
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use solana_sdk::account::Account;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{evm_peer, peers, receive_params, BASE_CHAIN_ID, BASE_EID, OP_CHAIN_ID};

pub mod common;

fn ready() -> common::Context {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    context
}

fn pairs(hashes: &[(u8, Pubkey)]) -> ProofData {
    ProofData::new(
        BASE_CHAIN_ID,
        hashes
            .iter()
            .map(|(byte, claimant)| IntentHashClaimant::new([*byte; 32].into(), claimant.to_bytes().into()))
            .collect(),
    )
}

fn proof(context: &common::Context, byte: u8) -> Option<Proof> {
    context
        .account::<ProofAccount>(&Proof::pda(&[byte; 32].into(), &layerzero_prover::ID).0)
        .map(|account| account.0)
}

#[test]
fn delivers_and_creates_proofs() {
    let mut context = ready();
    let (alice, bob) = (Pubkey::new_unique(), Pubkey::new_unique());
    let params = receive_params(&peers()[0], 1, pairs(&[(1, alice), (2, bob)]));
    let payload_hash = layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;

    let result = context.layerzero_prover().deliver(&params).unwrap();

    assert_eq!(proof(&context, 1).unwrap().claimant, alice);
    assert_eq!(proof(&context, 2).unwrap().destination, BASE_CHAIN_ID);
    assert!(common::contains_cpi_event(IntentProven::new([1; 32].into(), alice, BASE_CHAIN_ID))(result));
    assert!(context.get_account(&payload_hash).is_none());
}

/// The executor builds `lz_receive` from `lz_receive_types_v2`'s answer.
#[test]
fn delivery_built_from_v2_discovery_succeeds() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let discovered = context.layerzero_prover().lz_receive_types_v2(params.clone()).unwrap();
    let mut decoded = LzReceiveTypesV2Result::try_from_slice(&discovered.return_data.data).unwrap();
    let LzInstruction::LzReceive { accounts } = decoded.instructions.remove(0) else {
        panic!("expected an LzReceive instruction")
    };

    let instruction = context.layerzero_prover().lz_receive_instruction(&params, accounts);
    assert!(context.layerzero_prover().send(vec![instruction], &[]).is_ok());
    assert!(proof(&context, 1).is_some());
}

#[test]
fn replayed_delivery_fails() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().deliver(&params).unwrap();
    context.expire_blockhash();

    assert!(context.layerzero_prover().lz_receive(&params).is_err());
}

#[test]
fn chain_id_mismatch_rejected() {
    let mut context = ready();
    let mut data = pairs(&[(1, Pubkey::new_unique())]);
    data.destination = OP_CHAIN_ID;
    let params = receive_params(&peers()[0], 1, data);

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::ChainIdMismatch)));
    assert!(proof(&context, 1).is_none());
}

#[test]
fn non_peer_sender_rejected_even_if_endpoint_path_exists() {
    let mut context = ready();
    let stranger = evm_peer(BASE_EID, BASE_CHAIN_ID, 0x66);
    context.layerzero_prover().force_nonce(stranger.eid, stranger.address.into());
    let params = receive_params(&stranger, 1, pairs(&[(1, Pubkey::new_unique())]));

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidSender)));
}

#[test]
fn wrong_receiver_in_clear_accounts_rejected() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let mut accounts = layerzero_prover::instructions::lz_receive_accounts(&params, &ProofData::from_bytes(&params.message).unwrap());
    accounts[6].pubkey = layerzero::AddressLocator::Address(Pubkey::new_unique());

    let instruction = context.layerzero_prover().lz_receive_instruction(&params, accounts);
    let result = context.layerzero_prover().send(vec![instruction], &[]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidStore)));
}

#[test]
fn proof_account_mismatch_rejected() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique()), (2, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let mut accounts = layerzero_prover::instructions::lz_receive_accounts(&params, &ProofData::from_bytes(&params.message).unwrap());
    accounts.pop();

    let instruction = context.layerzero_prover().lz_receive_instruction(&params, accounts);
    let result = context.layerzero_prover().send(vec![instruction], &[]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidProof)));
}

#[test]
fn redelivery_is_idempotent_and_conflict_fails() {
    let mut context = ready();
    let alice = Pubkey::new_unique();
    context.layerzero_prover().deliver(&receive_params(&peers()[0], 1, pairs(&[(1, alice)]))).unwrap();

    let same = context.layerzero_prover().deliver(&receive_params(&peers()[0], 2, pairs(&[(1, alice)])));
    assert!(same.is_ok());

    let conflict = context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 3, pairs(&[(1, Pubkey::new_unique())])));
    assert!(conflict.is_err_and(common::is_error(LayerZeroProverError::IntentAlreadyProven)));
    assert_eq!(proof(&context, 1).unwrap().claimant, alice);
}

#[test]
fn duplicate_pair_in_one_message_is_idempotent_and_conflict_fails() {
    let mut context = ready();
    let alice = Pubkey::new_unique();

    let same = context.layerzero_prover().deliver(&receive_params(&peers()[0], 1, pairs(&[(1, alice), (1, alice)])));
    assert!(same.is_ok());

    let conflict = context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 2, pairs(&[(2, alice), (2, Pubkey::new_unique())])));
    assert!(conflict.is_err_and(common::is_error(LayerZeroProverError::IntentAlreadyProven)));
    assert!(proof(&context, 2).is_none());
}

#[test]
fn underfunded_pda_payer_fails_then_retry_succeeds() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    let payload_hash = layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;
    context.layerzero_prover().verify(&params).unwrap();
    context
        .set_account(pda_payer_pda().0, Account { lamports: 0, data: vec![], owner: anchor_lang::system_program::ID, executable: false, rent_epoch: 0 })
        .unwrap();

    assert!(context.layerzero_prover().lz_receive(&params).is_err());
    assert!(context.get_account(&payload_hash).is_some());

    context.airdrop(&pda_payer_pda().0, 1_000_000_000).unwrap();
    assert!(context.layerzero_prover().lz_receive(&params).is_ok());
    assert!(proof(&context, 1).is_some());
}

#[test]
fn cancelled_claimant_passes_through() {
    let mut context = ready();
    let cancelled = Pubkey::new_from_array(CANCELLED.into());

    context.layerzero_prover().deliver(&receive_params(&peers()[0], 1, pairs(&[(1, cancelled)]))).unwrap();

    assert_eq!(proof(&context, 1).unwrap().claimant, cancelled);
}

#[test]
fn delivered_proof_withdraws_and_refunds_rent_to_pda_payer() {
    let mut context = ready();
    let (reward, route_hash, hash) = context.layerzero_prover().funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let claimant = Pubkey::new_unique();
    let data = ProofData::new(BASE_CHAIN_ID, vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())]);
    let pda_payer_before = context.balance(&pda_payer_pda().0);
    context.layerzero_prover().deliver(&receive_params(&peers()[0], 1, data)).unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;

    let result = context.portal().withdraw_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        claimant,
        proof_address,
        WithdrawnMarker::pda(&hash).0,
        proof_closer_pda(&layerzero_prover::ID).0,
        Vec::<AccountMeta>::new(),
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );

    assert!(result.is_ok());
    assert_eq!(context.balance(&claimant), reward.native_amount);
    assert!(context.get_account(&proof_address).is_none());
    assert_eq!(context.balance(&pda_payer_pda().0), pda_payer_before);
}

#[test]
fn delivered_proof_aggregates() {
    let mut context = ready();
    let aggregator_authority = Keypair::new();
    context.aggregator_prover().install(aggregator_authority.pubkey());
    context.aggregator_prover().init(&aggregator_authority, vec![layerzero_prover::ID]).unwrap();
    let claimant = Pubkey::new_unique();
    let hash: Bytes32 = [9; 32].into();
    let data = ProofData::new(BASE_CHAIN_ID, vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())]);
    context.layerzero_prover().deliver(&receive_params(&peers()[0], 1, data)).unwrap();

    let result = context.aggregator_prover().aggregate(hash, layerzero_prover::ID);

    assert!(result.is_ok());
    assert!(context.get_account(&Proof::pda(&hash, &aggregator_prover::ID).0).is_some());
}

#[test]
fn proven_cancellation_refunds_through_close_proof() {
    let mut context = ready();
    let (reward, route_hash, hash) = context.layerzero_prover().funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let data = ProofData::new(BASE_CHAIN_ID, vec![IntentHashClaimant::new(hash, CANCELLED)]);
    context.layerzero_prover().deliver(&receive_params(&peers()[0], 1, data)).unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;

    let result = context.portal().refund_intent_with_close_proof(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        proof_address,
        WithdrawnMarker::pda(&hash).0,
        reward.creator,
        Vec::<AccountMeta>::new(),
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_ok());
    assert_eq!(context.balance(&reward.creator), reward.native_amount);
    assert!(context.get_account(&proof_address).is_none());
}
```

`ProofData` fields are `pub` in `eco-svm-std`, which `chain_id_mismatch_rejected` relies on.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --test lz_receive_layerzero_prover --test close_proof_layerzero_prover`
Expected: FAIL to compile (`instruction::LzReceive`, context `deliver` missing).

- [ ] **Step 4: Implement `lz_receive.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};
use eco_svm_std::Bytes32;

use crate::instructions::{LayerZeroProverError, CLEAR_ACCOUNTS_LEN};
use crate::layerzero::{self, ClearParams, LzReceiveParams, CLEAR_DISCRIMINATOR, ENDPOINT_ID};
use crate::state::{pda_payer_pda, ProofAccount, Store, PDA_PAYER_SEED, STORE_SEED};

/// Permissionless: authenticity comes from `endpoint::clear` (the payload must
/// match a DVN-verified hash, which it then closes) plus our own peer and
/// chain checks. Remaining accounts: the [`CLEAR_ACCOUNTS_LEN`] clear accounts,
/// then one `Proof` PDA per pair (see `lz_receive_accounts`).
#[event_cpi]
#[derive(Accounts)]
pub struct LzReceive<'info> {
    #[account(address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn lz_receive<'info>(ctx: Context<'info, LzReceive<'info>>, params: LzReceiveParams) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() >= CLEAR_ACCOUNTS_LEN,
        LayerZeroProverError::InvalidEndpoint
    );
    let (clear_accounts, proofs) = ctx.remaining_accounts.split_at(CLEAR_ACCOUNTS_LEN);
    let store = ctx.accounts.store.key();
    require_keys_eq!(clear_accounts[1].key(), store, LayerZeroProverError::InvalidStore);

    // Clear first: it burns the nonce before any state changes, as LayerZero requires.
    let (_, bump) = Store::pda();
    layerzero::invoke(
        ENDPOINT_ID,
        CLEAR_DISCRIMINATOR,
        &ClearParams {
            receiver: store,
            src_eid: params.src_eid,
            sender: params.sender,
            nonce: params.nonce,
            guid: params.guid,
            message: params.message.clone(),
        },
        clear_accounts,
        &[store],
        &[&[STORE_SEED, &[bump]]],
    )?;

    // The endpoint does not check the peer; this is our trust boundary.
    let peer = *ctx
        .accounts
        .store
        .peer(params.src_eid)
        .ok_or(LayerZeroProverError::UnknownPeer)?;
    require!(
        peer.address == Bytes32::from(params.sender),
        LayerZeroProverError::InvalidSender
    );
    // The endpoint authenticates `src_eid`, so the self-reported header cannot
    // claim a chain other than the peer's (EVM `_handleCrossChainMessage` rule).
    let proof_data = ProofData::from_bytes(&params.message)?;
    let destination = proof_data.destination;
    require!(destination == peer.chain_id, LayerZeroProverError::ChainIdMismatch);
    require!(
        proofs.len() == proof_data.intent_hashes_claimants.len(),
        LayerZeroProverError::InvalidProof
    );

    proofs
        .iter()
        .zip(proof_data.intent_hashes_claimants)
        .try_for_each(|(proof, pair)| mark_intent_hash_proven(&ctx, proof, destination, pair))
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'info, LzReceive<'info>>,
    proof: &AccountInfo<'info>,
    destination: u64,
    pair: IntentHashClaimant,
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = pair;
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, proof_bump) = prover::Proof::pda(&intent_hash, &crate::ID);
    require_keys_eq!(proof.key(), proof_pda, LayerZeroProverError::InvalidProof);
    let (_, payer_bump) = pda_payer_pda();

    // A delivered payload is immutable, so every redelivery carries the same
    // batch. Reaching the recorded state again is a no-op; only a disagreeing
    // state is an error. The event repeats either way.
    match prover::Proof::try_from_account_info(proof)? {
        Some(recorded) => require!(
            recorded.destination == destination && recorded.claimant == claimant,
            LayerZeroProverError::IntentAlreadyProven
        ),
        None => ProofAccount::from(prover::Proof::new(destination, claimant)).init(
            proof,
            &ctx.accounts.pda_payer,
            &ctx.accounts.system_program,
            &[
                &[PDA_PAYER_SEED, &[payer_bump]],
                &[PROOF_SEED, intent_hash.as_ref(), &[proof_bump]],
            ],
        )?,
    }

    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}
```

- [ ] **Step 5: Wire it**

`instructions/mod.rs`: `mod lz_receive; pub use lz_receive::*;`. `lib.rs`:

```rust
    pub fn lz_receive<'info>(ctx: Context<'info, LzReceive<'info>>, params: LzReceiveParams) -> Result<()> {
        instructions::lz_receive(ctx, params)
    }
```

- [ ] **Step 6: Add the context helpers**

```rust
/// `lz_receive` as the executor would build it from `accounts` (all
/// `AddressLocator::Address`). Free function so the batch-size tests can use it.
pub fn build_lz_receive_instruction(params: &LzReceiveParams, accounts: Vec<AccountMetaRef>) -> Instruction {
    let accounts = accounts
        .into_iter()
        .map(|meta| match meta.pubkey {
            AddressLocator::Address(pubkey) => AccountMeta {
                pubkey,
                is_signer: false,
                is_writable: meta.is_writable,
            },
            other => panic!("unexpected locator {other:?}"),
        })
        .collect();

    Instruction {
        program_id: layerzero_prover::ID,
        accounts,
        data: layerzero_prover::instruction::LzReceive { params: params.clone() }.data(),
    }
}

impl LayerZeroProver<'_> {
    /// Stands in for DVN verification: writes the PayloadHash and advances the
    /// path's inbound nonce on the mock endpoint.
    pub fn verify(&mut self, params: &LzReceiveParams) -> TransactionResult {
        let mut hasher = tiny_keccak::Keccak::v256();
        tiny_keccak::Hasher::update(&mut hasher, &params.guid);
        tiny_keccak::Hasher::update(&mut hasher, &params.message);
        let mut payload_hash = [0u8; 32];
        tiny_keccak::Hasher::finalize(hasher, &mut payload_hash);
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: ENDPOINT_ID,
            accounts: mock_layerzero_endpoint::accounts::MockVerify {
                payer: self.payer.pubkey(),
                nonce: layerzero::nonce_pda(&store, params.src_eid, &params.sender).0,
                payload_hash: layerzero::payload_hash_pda(&store, params.src_eid, &params.sender, params.nonce).0,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mock_layerzero_endpoint::instruction::MockVerify {
                params: mock_layerzero_endpoint::MockVerifyParams {
                    receiver: store,
                    src_eid: params.src_eid,
                    sender: params.sender,
                    nonce: params.nonce,
                    payload_hash,
                },
            }
            .data(),
        };

        self.send(vec![instruction], &[])
    }

    pub fn lz_receive_instruction(&self, params: &LzReceiveParams, accounts: Vec<AccountMetaRef>) -> Instruction {
        build_lz_receive_instruction(params, accounts)
    }

    pub fn lz_receive(&mut self, params: &LzReceiveParams) -> TransactionResult {
        let proof_data = ProofData::from_bytes(&params.message).unwrap();
        let accounts = layerzero_prover::instructions::lz_receive_accounts(params, &proof_data);
        let instruction = self.lz_receive_instruction(params, accounts);
        self.send(vec![instruction], &[])
    }

    pub fn deliver(&mut self, params: &LzReceiveParams) -> TransactionResult {
        self.verify(params)?;
        self.lz_receive(params)
    }

    /// Creates an endpoint Nonce for a path our `init_path` never opened, to
    /// prove our own peer check holds even if the endpoint had one.
    pub fn force_nonce(&mut self, src_eid: u32, sender: [u8; 32]) {
        let store = Store::pda().0;
        let (address, bump) = layerzero::nonce_pda(&store, src_eid, &sender);
        let data = anchor_account_data(&mock_layerzero_endpoint::Nonce {
            bump,
            outbound_nonce: 0,
            inbound_nonce: 0,
        });
        self.set_account(
            address,
            Account {
                lamports: 1_000_000_000,
                data,
                owner: ENDPOINT_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    }
}
```

(Imports: `layerzero_prover::layerzero::{AccountMetaRef, AddressLocator}`.)

- [ ] **Step 7: Build and run**

Run: `anchor build && cargo test --test lz_receive_layerzero_prover --test close_proof_layerzero_prover`
Expected: PASS. If `underfunded_pda_payer_fails_then_retry_succeeds` fails at the retry with `AlreadyProcessed`, add `context.expire_blockhash();` before the retry.

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo +nightly fmt
git add programs/layerzero-prover integration-tests/tests/common/layerzero_prover_context.rs integration-tests/tests/lz_receive_layerzero_prover.rs integration-tests/tests/close_proof_layerzero_prover.rs
git commit -m "feat(layerzero-prover): turn verified LayerZero messages into proofs in lz_receive"
```

---

### Task 8: Batch-size pins

**Files:**
- Create: `integration-tests/tests/layerzero_prover_batch_limits.rs`
- Modify (only if the measured ceilings differ): `programs/layerzero-prover/src/constants.rs`, spec §2.2 / §3.4 numbers, `CLAUDE.md` (Task 10 text)

**Interfaces:**
- Consumes: free builders `build_send_message_instruction`, `send_accounts` (Task 5), `build_lz_receive_instruction` (Task 7), `constants::{MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_MESSAGE}`.
- Produces: tests `outbound_ceiling_matches_max_intents_per_prove`, `inbound_ceiling_matches_max_pairs_per_message`, `max_inbound_batch_executes_under_floor_cu`.

litesvm enforces neither the 1232-byte packet limit nor account locks, so these compile real v0 messages and measure them.

- [ ] **Step 1: Write the tests**

```rust
//! Deliverability ceilings. litesvm does not enforce the 1232-byte packet
//! limit, so these compile the real v0 transactions and measure them.
//!
//! Outbound: `[ComputeBudget, portal::prove, send_message]` with one ALT
//! holding every non-signer, non-invoked account that exists before the
//! transaction (FulfillMarkers, store, dispatcher, LayerZero accounts, four
//! DVN worker quadruples). The `PendingSend` PDA is new per batch → static.
//!
//! Inbound: the executor's delivery transaction modelled as
//! `[ComputeBudget limit, ComputeBudget price, executor pre_execute,
//! lz_receive, executor post_execute]`. Our ALT holds the fixed `lz_receive`
//! accounts and every peer's Nonce; the PayloadHash and each new `Proof` PDA
//! are static. `pre_execute`/`post_execute` are modelled as
//! `[payer (s,w), execution context PDA (w)]` with 16 / 8 data bytes — the
//! devnet E2E confirms the real executor fits the same count.

use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use eco_svm_std::{Bytes32, CHAIN_ID};
use layerzero_prover::constants::{MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_MESSAGE};
use layerzero_prover::layerzero::{self, ENDPOINT_ID};
use layerzero_prover::state::{pda_payer_pda, PendingSend, Store};
use portal::state::FulfillMarker;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_packet::PACKET_DATA_SIZE;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::{v0, AddressLookupTableAccount, VersionedMessage};
use solana_sdk::pubkey::Pubkey;

use crate::common::layerzero_prover_context::{
    build_lz_receive_instruction, build_send_message_instruction, peers, receive_params, send_accounts,
    BASE_CHAIN_ID, BASE_EID,
};

pub mod common;

const DVN_COUNT: usize = 4;
const EXECUTOR_PROGRAM: Pubkey = Pubkey::new_from_array([0xe0; 32]);

fn transaction_len(payer: &Pubkey, instructions: &[Instruction], table: Vec<Pubkey>) -> usize {
    let alt = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: table,
    };
    let message = v0::Message::try_compile(payer, instructions, &[alt], Hash::default()).unwrap();
    // compact-u16 signature count, one signature, then the message.
    1 + 64 + VersionedMessage::V0(message).serialize().len()
}

fn ceiling(len: impl Fn(usize) -> usize, max: usize) -> usize {
    (1..=max).take_while(|&n| len(n) <= PACKET_DATA_SIZE).last().unwrap_or(0)
}

fn outbound_len(n: usize) -> usize {
    let payer = Pubkey::new_unique();
    let receiver = peers()[0].address;
    let hashes: Vec<Bytes32> = (0..n).map(|i| [i as u8 + 1; 32].into()).collect();
    let markers: Vec<Pubkey> = hashes.iter().map(|hash| FulfillMarker::pda(hash).0).collect();
    let payload = ProofData::new(
        CHAIN_ID,
        hashes.iter().map(|hash| IntentHashClaimant::new(*hash, [7; 32].into())).collect(),
    )
    .to_bytes();
    let pending = PendingSend::pda(BASE_EID, &receiver, &payload).0;
    let dispatcher = portal::state::dispatcher_pda(&layerzero_prover::ID).0;

    let prove = Instruction {
        program_id: portal::ID,
        accounts: [
            AccountMeta::new_readonly(layerzero_prover::ID, false),
            AccountMeta::new_readonly(dispatcher, false),
        ]
        .into_iter()
        .chain(markers.iter().map(|marker| AccountMeta::new_readonly(*marker, false)))
        .chain([
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(Store::pda().0, false),
            AccountMeta::new(pending, false),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        ])
        .collect(),
        data: anchor_lang::InstructionData::data(&portal::instruction::Prove {
            args: portal::instructions::ProveArgs {
                prover: layerzero_prover::ID,
                source_chain_domain_id: BASE_EID.into(),
                intent_hashes: hashes,
                data: receiver.to_vec(),
            },
        }),
    };
    let mut send = build_send_message_instruction(pending, payer, payer, BASE_EID, &receiver, 1);
    let workers: Vec<AccountMeta> = (0..4 + 4 * DVN_COUNT)
        .map(|i| AccountMeta::new_readonly(Pubkey::new_from_array([0x40 + i as u8; 32]), false))
        .collect();
    send.accounts.extend(workers.clone());

    let table = markers
        .into_iter()
        .chain([layerzero_prover::ID, dispatcher, Store::pda().0, anchor_lang::system_program::ID])
        .chain(send_accounts(Store::pda().0, payer, BASE_EID, &receiver).into_iter().map(|meta| meta.pubkey).filter(|key| *key != payer))
        .chain(workers.into_iter().map(|meta| meta.pubkey))
        .collect();

    transaction_len(
        &payer,
        &[ComputeBudgetInstruction::set_compute_unit_limit(1_400_000), prove, send],
        table,
    )
}

fn inbound_len(n: usize) -> usize {
    let payer = Pubkey::new_unique();
    let proof_data = ProofData::new(
        BASE_CHAIN_ID,
        (0..n).map(|i| IntentHashClaimant::new([i as u8 + 1; 32].into(), [7; 32].into())).collect(),
    );
    let params = receive_params(&peers()[0], u64::MAX, proof_data.clone());
    let accounts = layerzero_prover::instructions::lz_receive_accounts(&params, &proof_data);
    let lz_receive = build_lz_receive_instruction(&params, accounts);
    let execution_context = Pubkey::new_unique();
    let wrapper = |data_len: usize| Instruction {
        program_id: EXECUTOR_PROGRAM,
        accounts: vec![AccountMeta::new(payer, true), AccountMeta::new(execution_context, false)],
        data: vec![0; data_len],
    };
    let store = Store::pda().0;
    let table = [
        store,
        pda_payer_pda().0,
        anchor_lang::system_program::ID,
        eco_svm_std::event_authority_pda(&layerzero_prover::ID).0,
        layerzero_prover::ID,
        ENDPOINT_ID,
        layerzero::oapp_registry_pda(&store).0,
        layerzero::endpoint_settings_pda().0,
        layerzero::endpoint_event_authority().0,
    ]
    .into_iter()
    .chain(peers().iter().map(|peer| layerzero::nonce_pda(&store, peer.eid, &peer.address).0))
    .collect();

    transaction_len(
        &payer,
        &[
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudgetInstruction::set_compute_unit_price(1),
            wrapper(16),
            lz_receive,
            wrapper(8),
        ],
        table,
    )
}

#[test]
fn outbound_ceiling_matches_max_intents_per_prove() {
    let measured = ceiling(outbound_len, 64);
    assert_eq!(measured, MAX_INTENTS_PER_PROVE, "outbound ceiling measured at {measured}");
}

#[test]
fn inbound_ceiling_matches_max_pairs_per_message() {
    let measured = ceiling(inbound_len, 64);
    assert_eq!(measured, MAX_PAIRS_PER_MESSAGE, "inbound ceiling measured at {measured}");
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

    let floor = layerzero_prover::constants::lz_receive_gas(MAX_PAIRS_PER_MESSAGE) as u64;
    println!("lz_receive of {MAX_PAIRS_PER_MESSAGE} pairs: {} CU (floor {floor})", result.compute_units_consumed);
    assert!(result.compute_units_consumed < floor);
}
```

- [ ] **Step 2: Run and read the measured ceilings**

Run: `anchor build && cargo test --test layerzero_prover_batch_limits -- --nocapture`
Expected: `max_inbound_batch_executes_under_floor_cu` PASS. The two ceiling tests either PASS or FAIL with `… measured at N`.

- [ ] **Step 3: Set the constants to the measured ceilings**

If a ceiling test failed, set `MAX_INTENTS_PER_PROVE` / `MAX_PAIRS_PER_MESSAGE` in `constants.rs` to the printed `N`, and update the matching numbers in the spec (§2.2 "~8 pairs", §3.4 "provisionally 16"). If `N` for the outbound case is below 2, stop and raise it with the human: the outbound design needs prove and send in separate transactions.

Run: `anchor build && cargo test --test layerzero_prover_batch_limits && cargo test --test prove_layerzero_prover`
Expected: PASS.

- [ ] **Step 4: Lint and commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo +nightly fmt
git add programs/layerzero-prover/src/constants.rs integration-tests/tests/layerzero_prover_batch_limits.rs docs/superpowers/specs/2026-10-06-layerzero-prover-design.md
git commit -m "test(layerzero-prover): pin outbound and inbound batch ceilings"
```

---

### Task 9: Real-endpoint wiring test

**Files:**
- Create: `integration-tests/tests/layerzero_prover_real.rs`, `integration-tests/tests/fixtures/layerzero/README.md`

**Interfaces:**
- Consumes: context `install`, `init`, `init_path`, `verify`-equivalent staging, `lz_receive`; mock account types for byte-identical staging.
- Produces: `#[ignore]` test `init_path_and_clear_against_real_endpoint`, run with `LZ_ENDPOINT_SO=<path> cargo test --test layerzero_prover_real -- --ignored`.

Covers every CPI that touches only the endpoint (register_oapp, init_nonce, init_send/receive_library, set_send/receive_library, clear) against LayerZero's deployed binary. `init_config`/`set_config`/`send`/`quote` reach ULN302 and workers; the devnet E2E covers them.

- [ ] **Step 1: Write the fixture README**

`integration-tests/tests/fixtures/layerzero/README.md`:

````markdown
# LayerZero endpoint binary for `layerzero_prover_real`

Not committed. Dump the deployed EndpointV2 and point the test at it:

```bash
solana program dump 76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6 "$TMPDIR/lz_endpoint.so" --url devnet
shasum -a 256 "$TMPDIR/lz_endpoint.so"
LZ_ENDPOINT_SO="$TMPDIR/lz_endpoint.so" cargo test --test layerzero_prover_real -- --ignored --nocapture
```

Record the sha256 and date in the PR description when the test is run for a release.
````

- [ ] **Step 2: Write the test**

```rust
//! Our hand-mirrored endpoint CPIs against LayerZero's real EndpointV2 binary.
//! `#[ignore]`: needs `LZ_ENDPOINT_SO` (see fixtures/layerzero/README.md).

use eco_svm_std::prover::{IntentHashClaimant, Proof, ProofData};
use layerzero_prover::layerzero::{self, ENDPOINT_ID};
use layerzero_prover::state::{ProofAccount, Store};
use mock_layerzero_endpoint::{EndpointSettings, MessageLibInfo, MessageLibType, Nonce, PayloadHash};
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{anchor_account_data, peers, receive_params, BASE_CHAIN_ID};

pub mod common;

fn stage<T: anchor_lang::AccountSerialize>(context: &mut common::Context, address: Pubkey, account: &T) {
    let data = anchor_account_data(account);
    context
        .set_account(address, Account { lamports: 1_000_000_000, data, owner: ENDPOINT_ID, executable: false, rent_epoch: 0 })
        .unwrap();
}

#[test]
#[ignore]
fn init_path_and_clear_against_real_endpoint() {
    let path = std::env::var("LZ_ENDPOINT_SO").expect("set LZ_ENDPOINT_SO");
    let binary = std::fs::read(path).unwrap();
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    context.add_program(ENDPOINT_ID, &binary).unwrap();

    // What LayerZero's admin set up on-chain: endpoint settings and ULN302's library record.
    let (settings, settings_bump) = layerzero::endpoint_settings_pda();
    stage(&mut context, settings, &EndpointSettings { eid: layerzero::DEVNET_SOLANA_EID, bump: settings_bump, admin: Pubkey::new_unique(), lz_token_mint: None });
    let (uln_settings, uln_bump) = layerzero::uln_settings_pda();
    let (info, info_bump) = layerzero::message_lib_info_pda(&uln_settings);
    stage(&mut context, info, &MessageLibInfo { message_lib_type: MessageLibType::SendAndReceive, bump: info_bump, message_lib_bump: uln_bump });

    context.layerzero_prover().init(&authority, peers()).unwrap();
    let peer = peers()[0];
    context.layerzero_prover().init_path(&authority, &peer).unwrap();

    // Stand in for DVN verification of nonce 1.
    let claimant = Pubkey::new_unique();
    let data = ProofData::new(BASE_CHAIN_ID, vec![IntentHashClaimant::new([1; 32].into(), claimant.to_bytes().into())]);
    let params = receive_params(&peer, 1, data);
    let store = Store::pda().0;
    let (nonce, nonce_bump) = layerzero::nonce_pda(&store, peer.eid, &peer.address);
    stage(&mut context, nonce, &Nonce { bump: nonce_bump, outbound_nonce: 0, inbound_nonce: 1 });
    let mut hasher = tiny_keccak::Keccak::v256();
    tiny_keccak::Hasher::update(&mut hasher, &params.guid);
    tiny_keccak::Hasher::update(&mut hasher, &params.message);
    let mut hash = [0u8; 32];
    tiny_keccak::Hasher::finalize(hasher, &mut hash);
    let (payload_hash, payload_bump) = layerzero::payload_hash_pda(&store, peer.eid, &peer.address, 1);
    stage(&mut context, payload_hash, &PayloadHash { hash, bump: payload_bump });

    let result = context.layerzero_prover().lz_receive(&params);

    assert!(result.is_ok(), "{result:?}");
    assert!(context.get_account(&payload_hash).is_none());
    let proof = context
        .account::<ProofAccount>(&Proof::pda(&[1; 32].into(), &layerzero_prover::ID).0)
        .unwrap();
    assert_eq!(proof.0.claimant, claimant);
}
```

- [ ] **Step 3: Run it (requires network once for the dump)**

Run the README commands.
Expected: PASS. A failure inside `init_path` or `clear` means a mirrored account list, seed or discriminator in `layerzero.rs` disagrees with LayerZero's binary — fix `layerzero.rs` (and the mock to match), never the test. Also confirm the default run skips it: `cargo test --test layerzero_prover_real` → `1 ignored`.

- [ ] **Step 4: Lint and commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo +nightly fmt
git add integration-tests/tests/layerzero_prover_real.rs integration-tests/tests/fixtures/layerzero/README.md
git commit -m "test(layerzero-prover): check endpoint CPIs against LayerZero's deployed binary"
```

---

### Task 10: Build/release wiring and documentation

**Files:**
- Modify: `Anchor.toml`, `.github/workflows/release.yml`, `scripts/bump-cargo-versions.sh`, `CLAUDE.md`, `README.md`

**Interfaces:**
- Consumes: everything above.
- Produces: devnet/mainnet builds and IDL releases include `layerzero-prover`; docs describe it.

- [ ] **Step 1: Wire builds and releases**

`Anchor.toml`:
- `[programs.devnet]` and `[programs.mainnet]`: add `layerzero-prover = "<PROGRAM_ID>"`. Do **not** add `mock-layerzero-endpoint`.
- `build-devnet`: append `&& anchor build --program-name layerzero-prover`.
- `build-mainnet`: append `&& anchor build --program-name layerzero-prover -- --features mainnet`.
- `deploy-devnet` / `deploy-mainnet`: append `&& anchor deploy --provider.cluster <cluster> --program-name layerzero-prover` with the matching cluster.

`.github/workflows/release.yml`: in both `for program in ...` loops, append `layerzero_prover` (after `polymer_prover`).

`scripts/bump-cargo-versions.sh`: add `programs/layerzero-prover/Cargo.toml` to its list (keep alphabetical with the neighbours).

- [ ] **Step 2: Verify the enumerations**

Run: `anchor run build-devnet && ls target/deploy/layerzero_prover.so && ! grep -n "mock-layerzero-endpoint\|mock_layerzero_endpoint" .github/workflows/release.yml && ! grep -n "program-name mock-layerzero-endpoint" Anchor.toml`
Expected: build succeeds; both negative greps print nothing. Then `scripts/assert-sbf-rustc.sh` with the CI env values (see `.github/workflows/pr-checks.yml`) passes.

- [ ] **Step 3: Document in `CLAUDE.md`**

- In **Architecture**: "Seven production" → "Eight production"; add `mock-layerzero-endpoint` to the localnet-only test-program list.
- In the build section's PoC/mock paragraph: name `mock-layerzero-endpoint` alongside `mock-polymer-prover` as an Anchor program kept out of devnet/mainnet by the explicit enumerations.
- Add a **layerzero-prover** bullet under **Programs**:

```markdown
- **layerzero-prover** — LayerZero V2-backed prover, both directions; liveness member for the 1-of-N aggregator. **Outbound is two-step**: `prove` (gated to `dispatcher_pda(&layerzero_prover::ID)`) only commits the batch to a content-addressed `PendingSend` PDA, and a permissionless top-level `send_message` CPIs `endpoint::send` — `send` nests endpoint → ULN → worker → pricefeed, which already fills Solana's 5-frame invoke stack, so it cannot run under `portal::prove` (SIMD-0268, raising the limit, was not active when this shipped). Solver: `[ComputeBudget, portal::prove, layerzero_prover::send_message]` in one v0 transaction with ALTs, fee from simulating `quote_message`; batch ≤ `MAX_INTENTS_PER_PROVE`. Options are computed on-chain with the EVM gas floor (`200_000 + 50_000·n`). **Inbound** is LayerZero's executor V2 flow: `lz_receive_types_info` → `lz_receive_types_v2` (both derive from `lz_receive_accounts`, which must stay the single source of the account list) → `lz_receive`, which CPIs `endpoint::clear` first, then checks `sender == peer[src_eid].address` and `ProofData.destination == peer.chain_id`, then creates `Proof` PDAs like `hyper-prover::handle` (identical redelivery no-op, conflict `IntentAlreadyProven`). Proof rent comes from the `pda_payer` reserve (`close_proof` refunds it) because the EVM sender's options carry gas only; an empty reserve makes delivery fail retryably, so monitor its balance. EVM `Inbox.prove` batches toward Solana must stay ≤ `MAX_PAIRS_PER_MESSAGE` — the EVM contract cannot enforce it and an over-cap message can never execute (re-prove smaller). The EVM `LayerZeroProver` must whitelist the **`Store` PDA** (the OApp address LayerZero reports as `origin.sender`), not the program ID, and map Solana's EID (30168 mainnet / 40168 devnet) to `CHAIN_ID`. **Admin model**: `pda_payer` is the LayerZero delegate; `init` / `init_path` / `set_path_config` / `set_alt` are gated on the program's upgrade authority, so finalizing the program is the delegate revocation. Every path must be fully pinned (ULN302 send/receive library, explicit DVNs, confirmations, executor — `set_path_config` rejects LayerZero defaults) and read back before finalizing; an unpinned path after finalization needs a new program ID. `layerzero.rs` hand-mirrors LayerZero-v2@9c741e7f (endpoint/ULN IDs, big-endian seeds, discriminators, V2 executor types); `layerzero_prover_real` checks it against the dumped endpoint binary. Spec: `docs/superpowers/specs/2026-10-06-layerzero-prover-design.md`.
```

- In **The programs ship as one atomic release**: add layerzero-prover to the list ("Portal, local-prover, hyper-prover, flash-fulfiller, polymer-prover, and layerzero-prover must be built from one tree and deployed together").
- In **Integration tests**: add `layerzero_prover_context.rs` to the per-program contexts list.

- [ ] **Step 4: Document in `README.md`**

Add a "LayerZero Prover" subsection next to the Polymer one, covering: purpose (liveness member), the two-step outbound transaction, the inbound executor flow, `pda_payer` funding, batch caps, and the EVM-side configuration (Store PDA whitelist, EID mapping) — same content as the CLAUDE.md bullet, written for integrators.

- [ ] **Step 5: Full verification**

Run:
```bash
anchor build && cargo build-sbf --manifest-path integration-tests/programs/mock-igp/Cargo.toml --sbf-out-dir target/deploy
cargo test --no-fail-fast
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt --all -- --check
cargo sort --workspace --check
```
Expected: all green (the real-endpoint test reports `ignored`).

- [ ] **Step 6: Commit**

```bash
git add Anchor.toml .github/workflows/release.yml scripts/bump-cargo-versions.sh CLAUDE.md README.md
git commit -m "chore(layerzero-prover): wire devnet/mainnet builds and IDL release, document the program"
```
