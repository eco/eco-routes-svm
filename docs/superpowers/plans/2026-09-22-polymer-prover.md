# Polymer Prover (Solana) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `polymer-prover` Anchor program that validates Polymer proofs of the EVM `IntentFulfilledFromSource` event into Portal-compatible `Proof` PDAs, and emits Polymer-provable logs for the reverse direction.

**Architecture:** A sixth production program shaped like hyper-prover. `validate` CPIs Polymer's deployed `polymer_prover` program (`validate_event`), reads the result account it writes, mirrors the Solidity `PolymerProver.validate` checks, and creates `Proof` PDAs idempotently. `prove` is gated to Portal's prover-scoped dispatcher and writes one `Prove: program: …` log line per intent. The Polymer CPI and result layout are hand-mirrored in `polymer.rs` (no dependency on Polymer's crate). A localnet-only `mock-polymer-prover` program with identical account layouts drives the litesvm tests.

**Tech Stack:** Anchor 1.1.2, Rust 1.97.1 host / 1.89.0-dev on-chain, litesvm 0.16, goldie 0.7.

**Spec:** `docs/superpowers/specs/2026-09-22-polymer-prover-design.md`. The eco-routes half of the spec (section 5) has its own plan in the eco-routes repo.

## Global Constraints

- Anchor `1.1.2`; `Context<'info, T<'info>>` takes two lifetimes in this version (see `programs/hyper-prover/src/instructions/handle.rs`).
- On-chain rustc is `1.89.0-dev`: no `is_multiple_of`, no stdlib newer than 1.89 in program code. `div_ceil` (1.73) and `split_at_checked` (1.80) are fine.
- Lint: `cargo clippy --all-targets -- -D warnings` (never `--all-features`), `cargo +nightly fmt`, `cargo sort --workspace --check`.
- `anchor build` must run before `cargo test`; integration tests `include_bytes!` from `target/deploy/`.
- goldie 0.7 writes snapshots to `<module>/testdata/` next to the source file (e.g. `programs/polymer-prover/src/state/testdata/`). Create with `GOLDIE_UPDATE=1 cargo test -p polymer-prover` and review the diff.
- Never replace the explicit `--program-name` enumerations in `Anchor.toml` scripts or `release.yml` with a bare `anchor build`; localnet-only programs must stay out of devnet/mainnet artifacts.
- Prover-scoped authorities: `prove` accepts only `portal::state::dispatcher_pda(&crate::ID)`, `close_proof` only `portal::state::proof_closer_pda(&crate::ID)`.
- `mainnet` feature: `["eco-svm-std/mainnet", "portal/mainnet"]`; flips `CHAIN_ID` (1399811149 mainnet / 1399811150 otherwise) and `POLYMER_PROVER_ID`.
- Polymer program IDs: mainnet `CdvSq48QUukYuMczgZAVNZrwcHNshBdtqrjW26sQiGPs`, devnet `FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3`.
- Commit messages: conventional prefix, end with `Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8`. Only commit files you changed. Do not push or open a PR unless asked.
- Program IDs are keypair-backed `Eco…` vanity keys; deploy keypairs live in the git-ignored `keys/` directory.

## File Structure

```
programs/polymer-prover/
  Cargo.toml
  src/lib.rs                       declare_id!, #[program] dispatch
  src/polymer.rs                   Polymer program mirror: IDs, PDAs, discriminators, ValidationResult, validate_event CPI
  src/event.rs                     IntentFulfilledFromSource parsing (selector, topics, ABI bytes)
  src/state.rs                     Config, ProofAccount
  src/instructions/mod.rs          re-exports + PolymerProverError
  src/instructions/init.rs
  src/instructions/validate.rs
  src/instructions/prove.rs
  src/instructions/close_proof.rs
programs/mock-polymer-prover/      localnet-only Anchor stand-in for Polymer's program (same account layouts and instruction names)
  Cargo.toml
  src/lib.rs
integration-tests/tests/common/polymer_prover_context.rs   builders for polymer-prover and the mock
integration-tests/tests/init_polymer_prover.rs
integration-tests/tests/validate_polymer_prover.rs
integration-tests/tests/prove_polymer_prover.rs
integration-tests/tests/close_proof_polymer_prover.rs
integration-tests/tests/validate_polymer_prover_real.rs    #[ignore] smoke test against Polymer's real .so
integration-tests/tests/fixtures/polymer/op-proof-v2.hex   Polymer's published fixture proof (Apache-2.0)
```

Modified: `Anchor.toml`, `Cargo.toml` (workspace deps), `integration-tests/Cargo.toml`, `integration-tests/tests/common/mod.rs`, `integration-tests/tests/withdraw_confused_deputy.rs`, `.github/workflows/release.yml`, `CLAUDE.md`, `README.md`, the spec.

Note on the mock's location: the spec says `integration-tests/programs/`. It goes in `programs/` instead, beside `dummy-ism` and the `malicious-*` programs, because that is the directory `anchor build` compiles. Task 10 amends the spec.

---

### Task 1: Scaffold the program with `init`, `Config`, and workspace wiring

**Files:**
- Create: `programs/polymer-prover/Cargo.toml`
- Create: `programs/polymer-prover/src/lib.rs`
- Create: `programs/polymer-prover/src/state.rs`
- Create: `programs/polymer-prover/src/instructions/mod.rs`
- Create: `programs/polymer-prover/src/instructions/init.rs`
- Modify: `Anchor.toml` (all three `[programs.*]` sections and the four scripts)
- Modify: `Cargo.toml` (`[workspace.dependencies]`)
- Modify: `.github/workflows/release.yml:75,89`

**Interfaces:**
- Produces: `polymer_prover::ID`, `polymer_prover::state::{Config, ProofAccount, CONFIG_SEED}`, `Config::pda() -> (Pubkey, u8)`, `Config::new(Vec<Bytes32>) -> Result<Config>`, `Config::is_whitelisted(&Bytes32) -> bool`, `polymer_prover::instructions::{InitArgs, PolymerProverError}`.

- [ ] **Step 1: Grind the program keypair**

```bash
mkdir -p keys
solana-keygen grind --starts-with Eco:1 --num-threads 8
mv Eco*.json keys/polymer_prover-keypair.json
solana-keygen pubkey keys/polymer_prover-keypair.json
```

Record the printed pubkey; it is `<POLYMER_PROVER_PROGRAM_ID>` below. `keys/` is git-ignored (verify with `git check-ignore keys/polymer_prover-keypair.json`). Also copy it to `target/deploy/polymer_prover-keypair.json` so `anchor build` does not generate a mismatching one.

- [ ] **Step 2: Create `programs/polymer-prover/Cargo.toml`**

```toml
[package]
description = "Polymer-backed prover for eco-routes-svm"
edition = "2021"
name = "polymer-prover"
version = "0.1.0"

[lib]
crate-type = ["cdylib", "lib"]
name = "polymer_prover"

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

# The k256 → getrandom path (via anchor's solana-secp256k1-recover) needs a
# getrandom backend for the SBF target; enable the custom backend there only.
[target.'cfg(target_os = "solana")'.dependencies]
getrandom = { workspace = true }

[dev-dependencies]
goldie = { workspace = true }
```

- [ ] **Step 3: Write the failing state unit tests**

Create `programs/polymer-prover/src/state.rs`:

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::instructions::PolymerProverError;

pub const CONFIG_SEED: &[u8] = b"config";
const MAX_WHITELIST_LEN: usize = 20;

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub eco_svm_std::prover::Proof);

impl AccountExt for ProofAccount {}

impl From<eco_svm_std::prover::Proof> for ProofAccount {
    fn from(proof: eco_svm_std::prover::Proof) -> Self {
        Self(proof)
    }
}

/// Whitelist of EVM `PolymerProver` contracts whose `IntentFulfilledFromSource`
/// events this program accepts. Each entry is a 20-byte EVM address left-padded
/// to 32 bytes.
#[account]
#[derive(InitSpace)]
pub struct Config {
    #[max_len(MAX_WHITELIST_LEN)]
    pub whitelisted_emitters: Vec<Bytes32>,
}

impl Config {
    pub fn new(whitelisted_emitters: Vec<Bytes32>) -> Result<Self> {
        if whitelisted_emitters.len() > MAX_WHITELIST_LEN {
            return Err(PolymerProverError::TooManyWhitelistedEmitters.into());
        }

        Ok(Self {
            whitelisted_emitters,
        })
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }

    pub fn is_whitelisted(&self, emitter: &Bytes32) -> bool {
        self.whitelisted_emitters.contains(emitter)
    }
}

impl AccountExt for Config {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_pda_deterministic() {
        goldie::assert_json!(Config::pda());
    }

    /// The portal authority `prove` accepts, scoped to this program's ID. A seed
    /// change here silently locks out portal — pin the address, not just the
    /// derivation.
    #[test]
    fn accepted_prove_caller_authority_deterministic() {
        goldie::assert_json!(portal::state::dispatcher_pda(&crate::ID));
    }

    /// The portal authority `close_proof` accepts, scoped to this program's ID.
    #[test]
    fn accepted_proof_closer_authority_deterministic() {
        goldie::assert_json!(portal::state::proof_closer_pda(&crate::ID));
    }

    #[test]
    fn config_new_success() {
        let emitters = vec![[1u8; 32].into(), [2u8; 32].into()];
        let config = Config::new(emitters.clone()).unwrap();

        assert_eq!(config.whitelisted_emitters, emitters);
    }

    #[test]
    fn config_new_too_many_emitters() {
        let emitters = vec![[0u8; 32].into(); MAX_WHITELIST_LEN + 1];

        assert!(Config::new(emitters).is_err());
    }

    #[test]
    fn config_is_whitelisted() {
        let emitter1: Bytes32 = [1u8; 32].into();
        let emitter2: Bytes32 = [2u8; 32].into();
        let emitter3: Bytes32 = [3u8; 32].into();

        let config = Config::new(vec![emitter1, emitter2]).unwrap();

        assert!(config.is_whitelisted(&emitter1));
        assert!(config.is_whitelisted(&emitter2));
        assert!(!config.is_whitelisted(&emitter3));
    }
}
```

- [ ] **Step 4: Create `instructions/mod.rs` with the full error enum**

```rust
use anchor_lang::prelude::*;

mod close_proof;
mod init;
mod prove;
mod validate;

pub use close_proof::*;
pub use init::*;
pub use prove::*;
pub use validate::*;

#[error_code]
pub enum PolymerProverError {
    InvalidPortalDispatcher,
    InvalidPortalProofCloser,
    InvalidConfig,
    TooManyWhitelistedEmitters,
    InvalidPolymerProver,
    InvalidCacheAccount,
    InvalidResultAccount,
    InvalidInternalAccount,
    PolymerProofInvalid,
    InvalidEmittingContract,
    InvalidTopicsLength,
    InvalidEventSignature,
    InvalidSourceChain,
    InvalidDestinationChain,
    InvalidEventData,
    InvalidProof,
    IntentAlreadyProven,
    InvalidDestination,
    EmptyProofData,
    TooManyIntents,
}
```

For this task only, comment out the `mod close_proof; mod prove; mod validate;` lines and their `pub use` lines; later tasks uncomment them as the files appear.

- [ ] **Step 5: Create `instructions/init.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::instructions::PolymerProverError;
use crate::state::{Config, CONFIG_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitArgs {
    pub whitelisted_emitters: Vec<Bytes32>,
}

#[derive(Accounts)]
#[instruction(args: InitArgs)]
pub struct Init<'info> {
    /// CHECK: address is validated
    #[account(mut)]
    pub config: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
    let (config_pda, bump) = Config::pda();
    require!(
        ctx.accounts.config.key() == config_pda,
        PolymerProverError::InvalidConfig
    );
    let signer_seeds = [CONFIG_SEED, &[bump]];

    Config::new(args.whitelisted_emitters)?.init(
        &ctx.accounts.config,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&signer_seeds],
    )
}
```

- [ ] **Step 6: Create `src/lib.rs`**

```rust
use anchor_lang::prelude::*;

declare_id!("<POLYMER_PROVER_PROGRAM_ID>");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod polymer_prover {
    use super::*;

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }
}
```

- [ ] **Step 7: Wire the workspace and Anchor.toml**

`Cargo.toml` `[workspace.dependencies]`, keep alphabetical (cargo sort):

```toml
polymer-prover = { path = "programs/polymer-prover" }
```

`Anchor.toml`: add `polymer-prover = "<POLYMER_PROVER_PROGRAM_ID>"` to `[programs.localnet]`, `[programs.devnet]` and `[programs.mainnet]`. Append to each script, keeping the pattern of the existing entries:

```toml
build-devnet = "... && anchor build --program-name polymer-prover"
build-mainnet = "... && anchor build --program-name polymer-prover -- --features mainnet"
deploy-devnet = "... && anchor deploy --provider.cluster devnet --program-name polymer-prover"
deploy-mainnet = "... && anchor deploy --provider.cluster mainnet --program-name polymer-prover"
```

`.github/workflows/release.yml`: both loops become

```bash
for program in portal hyper_prover local_prover flash_fulfiller proof_helper polymer_prover; do
```

- [ ] **Step 8: Build and run the unit tests**

```bash
anchor build
GOLDIE_UPDATE=1 cargo test -p polymer-prover
cargo test -p polymer-prover
```

Expected: build succeeds, three `.golden` files appear under `programs/polymer-prover/src/state/testdata/`, all six tests pass. Open the goldens and confirm the `accepted_*` ones are PDAs under `portal::ID`.

- [ ] **Step 9: Lint and commit**

```bash
cargo +nightly fmt && cargo sort --workspace && cargo clippy --all-targets -- -D warnings
git add programs/polymer-prover Anchor.toml Cargo.toml Cargo.lock .github/workflows/release.yml
git commit -m "feat(polymer-prover): scaffold program with init and Config

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 2: Mirror Polymer's program in `polymer.rs`

**Files:**
- Create: `programs/polymer-prover/src/polymer.rs`
- Modify: `programs/polymer-prover/src/lib.rs` (add `pub mod polymer;`)

**Interfaces:**
- Produces: `POLYMER_PROVER_ID: Pubkey`, `cache_pda(&Pubkey) -> (Pubkey, u8)`, `result_pda(&Pubkey) -> (Pubkey, u8)`, `internal_pda() -> (Pubkey, u8)`, `VALIDATE_EVENT_DISCRIMINATOR`, `CREATE_ACCOUNTS_DISCRIMINATOR`, `LOAD_PROOF_DISCRIMINATOR`, `VALIDATION_RESULT_DISCRIMINATOR`, `INTERNAL_ACCOUNT_DISCRIMINATOR` (all `[u8; 8]`), `struct ValidationResult { is_valid: bool, error_message: String, chain_id: u32, emitting_contract: [u8; 20], topics: Vec<u8>, unindexed_data: Vec<u8> }`, `ValidationResult::try_from_account_info(&AccountInfo) -> Result<ValidationResult>`, `validate_event(program, authority, cache, result, internal) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Create `programs/polymer-prover/src/polymer.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use anchor_lang::solana_program::hash::hash;

    use super::*;

    fn anchor_discriminator(name: &str) -> [u8; 8] {
        hash(name.as_bytes()).to_bytes()[..8].try_into().unwrap()
    }

    #[test]
    fn instruction_discriminators_match_anchor_derivation() {
        assert_eq!(
            VALIDATE_EVENT_DISCRIMINATOR,
            anchor_discriminator("global:validate_event")
        );
        assert_eq!(
            CREATE_ACCOUNTS_DISCRIMINATOR,
            anchor_discriminator("global:create_accounts")
        );
        assert_eq!(
            LOAD_PROOF_DISCRIMINATOR,
            anchor_discriminator("global:load_proof")
        );
    }

    #[test]
    fn account_discriminators_match_anchor_derivation() {
        assert_eq!(
            VALIDATION_RESULT_DISCRIMINATOR,
            anchor_discriminator("account:ValidationResultAccount")
        );
        assert_eq!(
            INTERNAL_ACCOUNT_DISCRIMINATOR,
            anchor_discriminator("account:InternalAccount")
        );
    }

    #[test]
    fn pdas_deterministic() {
        let authority = Pubkey::new_from_array([7u8; 32]);
        goldie::assert_json!((
            cache_pda(&authority),
            result_pda(&authority),
            internal_pda()
        ));
    }

    #[test]
    fn validation_result_roundtrip_ignores_trailing_padding() {
        let expected = ValidationResult {
            is_valid: true,
            error_message: String::new(),
            chain_id: 8453,
            emitting_contract: [0xab; 20],
            topics: vec![1u8; 64],
            unindexed_data: vec![2u8; 96],
        };
        let mut body = expected.try_to_vec().unwrap();
        // Polymer allocates the account at max size, so the serialized body is
        // followed by zero padding that a strict decoder would reject.
        body.extend(std::iter::repeat_n(0u8, 512));

        let decoded = ValidationResult::from_body(&body).unwrap();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn validation_result_rejects_truncated_body() {
        let body = vec![1u8; 5];
        assert!(ValidationResult::from_body(&body).is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p polymer-prover polymer::
```

Expected: compile error, `VALIDATE_EVENT_DISCRIMINATOR` etc. not found.

- [ ] **Step 3: Write the implementation above the tests**

```rust
//! Hand-rolled mirror of the pieces of Polymer's deployed `polymer_prover`
//! program (polymerdao/solana-prover-contracts v1.0.4) that this program
//! consumes. Kept as constants rather than a crate dependency so Polymer's
//! crypto dependencies and Anchor pin stay out of our build graph.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;

use crate::instructions::PolymerProverError;

#[cfg(feature = "mainnet")]
pub const POLYMER_PROVER_ID: Pubkey = pubkey!("CdvSq48QUukYuMczgZAVNZrwcHNshBdtqrjW26sQiGPs");
#[cfg(not(feature = "mainnet"))]
pub const POLYMER_PROVER_ID: Pubkey = pubkey!("FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3");

pub const CACHE_SEED: &[u8] = b"cache";
pub const RESULT_SEED: &[u8] = b"result";
pub const INTERNAL_SEED: &[u8] = b"internal";

/// `sha256("global:validate_event")[..8]`
pub const VALIDATE_EVENT_DISCRIMINATOR: [u8; 8] = [0x42, 0xf9, 0xcf, 0xdd, 0x1e, 0x57, 0x1b, 0x81];
/// `sha256("global:create_accounts")[..8]`
pub const CREATE_ACCOUNTS_DISCRIMINATOR: [u8; 8] = [0x5e, 0xaf, 0x6d, 0xaa, 0xad, 0x0b, 0x19, 0xb0];
/// `sha256("global:load_proof")[..8]`
pub const LOAD_PROOF_DISCRIMINATOR: [u8; 8] = [0x22, 0x91, 0x55, 0x09, 0x48, 0x62, 0x11, 0x5c];
/// `sha256("account:ValidationResultAccount")[..8]`
pub const VALIDATION_RESULT_DISCRIMINATOR: [u8; 8] =
    [0xa0, 0x95, 0x47, 0x4c, 0x94, 0x30, 0x55, 0xe5];
/// `sha256("account:InternalAccount")[..8]`
pub const INTERNAL_ACCOUNT_DISCRIMINATOR: [u8; 8] =
    [0x97, 0x95, 0xe9, 0x4e, 0x8f, 0x0c, 0x11, 0xfe];

pub fn cache_pda(authority: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CACHE_SEED, authority.as_ref()], &POLYMER_PROVER_ID)
}

pub fn result_pda(authority: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RESULT_SEED, authority.as_ref()], &POLYMER_PROVER_ID)
}

pub fn internal_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[INTERNAL_SEED], &POLYMER_PROVER_ID)
}

/// Body of Polymer's `ValidationResultAccount`, field order and types verbatim.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub error_message: String,
    pub chain_id: u32,
    pub emitting_contract: [u8; 20],
    pub topics: Vec<u8>,
    pub unindexed_data: Vec<u8>,
}

impl ValidationResult {
    /// Decodes the account Polymer wrote in `validate_event`. Requires the
    /// account to be owned by Polymer's program and to carry its discriminator.
    pub fn try_from_account_info(account: &AccountInfo<'_>) -> Result<Self> {
        require!(
            *account.owner == POLYMER_PROVER_ID,
            PolymerProverError::InvalidResultAccount
        );
        let data = account.try_borrow_data()?;
        let (discriminator, body) = data
            .split_at_checked(8)
            .ok_or(PolymerProverError::InvalidResultAccount)?;
        require!(
            discriminator == VALIDATION_RESULT_DISCRIMINATOR,
            PolymerProverError::InvalidResultAccount
        );

        Self::from_body(body)
    }

    /// Borsh-decodes a body that may be followed by zero padding: Polymer
    /// allocates the account at `INIT_SPACE`, so `try_from_slice` (which
    /// insists on consuming every byte) would reject a valid account.
    pub fn from_body(mut body: &[u8]) -> Result<Self> {
        AnchorDeserialize::deserialize(&mut body)
            .map_err(|_| PolymerProverError::InvalidResultAccount.into())
    }
}

/// CPIs Polymer's `validate_event`. `authority` must be a signer of the outer
/// transaction; its signature passes through a plain `invoke`.
pub fn validate_event<'info>(
    polymer_prover_program: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    cache_account: &AccountInfo<'info>,
    result_account: &AccountInfo<'info>,
    internal: &AccountInfo<'info>,
) -> Result<()> {
    let ix = Instruction {
        program_id: polymer_prover_program.key(),
        accounts: vec![
            AccountMeta::new(authority.key(), true),
            AccountMeta::new(cache_account.key(), false),
            AccountMeta::new(result_account.key(), false),
            AccountMeta::new_readonly(internal.key(), false),
        ],
        data: VALIDATE_EVENT_DISCRIMINATOR.to_vec(),
    };

    invoke(
        &ix,
        &[
            authority.to_account_info(),
            cache_account.to_account_info(),
            result_account.to_account_info(),
            internal.to_account_info(),
        ],
    )
    .map_err(Into::into)
}
```

Add `pub mod polymer;` to `lib.rs`.

- [ ] **Step 4: Run the tests**

```bash
GOLDIE_UPDATE=1 cargo test -p polymer-prover polymer::
cargo test -p polymer-prover polymer::
```

Expected: 5 tests pass; `programs/polymer-prover/src/polymer/testdata/pdas_deterministic.golden` created. The three PDAs must be under the devnet Polymer ID (tests build without `mainnet`).

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add programs/polymer-prover
git commit -m "feat(polymer-prover): mirror Polymer program IDs, PDAs and result layout

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 3: Parse `IntentFulfilledFromSource` in `event.rs`

**Files:**
- Create: `programs/polymer-prover/src/event.rs`
- Modify: `programs/polymer-prover/src/lib.rs` (add `pub mod event;`)

**Interfaces:**
- Produces: `INTENT_FULFILLED_FROM_SOURCE_SELECTOR: [u8; 32]`, `TOPICS_LEN: usize = 64`, `struct IntentFulfilledFromSource { source: u64, encoded_proofs: Vec<u8> }`, `IntentFulfilledFromSource::parse(topics: &[u8], unindexed_data: &[u8]) -> Result<Self>`, `evm_address_to_bytes32([u8; 20]) -> Bytes32`, `abi_encode_bytes(&[u8]) -> Vec<u8>` (test-only helper under `#[cfg(test)]` is not enough: the integration tests need it too, so make it `pub`).

- [ ] **Step 1: Write the failing tests**

Create `programs/polymer-prover/src/event.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use anchor_lang::solana_program::keccak;

    use super::*;

    fn topics(selector: [u8; 32], source: u64) -> Vec<u8> {
        let mut topics = selector.to_vec();
        topics.extend_from_slice(&[0u8; 24]);
        topics.extend_from_slice(&source.to_be_bytes());
        topics
    }

    #[test]
    fn selector_matches_keccak_of_signature() {
        let expected = keccak::hash(b"IntentFulfilledFromSource(uint64,bytes)").to_bytes();
        assert_eq!(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, expected);
    }

    #[test]
    fn parse_success() {
        let payload = vec![0x11u8; 72];
        let event = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1399811150),
            &abi_encode_bytes(&payload),
        )
        .unwrap();

        assert_eq!(event.source, 1399811150);
        assert_eq!(event.encoded_proofs, payload);
    }

    #[test]
    fn abi_encode_bytes_pads_to_word() {
        goldie::assert_debug!((
            abi_encode_bytes(&[]),
            abi_encode_bytes(&[1, 2, 3]),
            abi_encode_bytes(&[9u8; 32]),
        ));
    }

    #[test]
    fn parse_rejects_wrong_topics_length() {
        let mut t = topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1);
        t.extend_from_slice(&[0u8; 32]);
        let err = IntentFulfilledFromSource::parse(&t, &abi_encode_bytes(&[1u8; 8])).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidTopicsLength.into());
    }

    #[test]
    fn parse_rejects_wrong_selector() {
        let err = IntentFulfilledFromSource::parse(
            &topics([0u8; 32], 1),
            &abi_encode_bytes(&[1u8; 8]),
        )
        .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventSignature.into());
    }

    #[test]
    fn parse_rejects_source_wider_than_u64() {
        let mut t = topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1);
        t[32] = 1; // high byte of the second topic word
        let err = IntentFulfilledFromSource::parse(&t, &abi_encode_bytes(&[1u8; 8])).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidSourceChain.into());
    }

    #[test]
    fn parse_rejects_bad_abi_offset() {
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[31] = 64;
        let err = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1),
            &data,
        )
        .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_short_buffer() {
        let data = abi_encode_bytes(&[1u8; 40]);
        let truncated = &data[..data.len() - 1];
        let err = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1),
            truncated,
        )
        .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_length_beyond_buffer() {
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[63] = 200;
        let err = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1),
            &data,
        )
        .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn evm_address_left_pads() {
        let address = [0xaau8; 20];
        let bytes: [u8; 32] = evm_address_to_bytes32(address).into();
        assert_eq!(bytes[..12], [0u8; 12]);
        assert_eq!(bytes[12..], address);
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p polymer-prover event::
```

Expected: compile error.

- [ ] **Step 3: Write the implementation**

```rust
//! Decoding of the EVM `IntentFulfilledFromSource(uint64 indexed source, bytes
//! encodedProofs)` event as Polymer returns it: flat 32-byte topics and the
//! ABI-encoded non-indexed data.

use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::instructions::PolymerProverError;

/// `keccak256("IntentFulfilledFromSource(uint64,bytes)")`
pub const INTENT_FULFILLED_FROM_SOURCE_SELECTOR: [u8; 32] = [
    0xd4, 0x93, 0xdd, 0xe4, 0xde, 0x24, 0x06, 0x6d, 0xb2, 0x9a, 0x75, 0x4b, 0x1e, 0x9d, 0xc5, 0xde,
    0xdf, 0x70, 0x84, 0x85, 0x0c, 0x4a, 0xbf, 0x76, 0xc2, 0x30, 0x46, 0xba, 0x9d, 0xad, 0x6b, 0x1d,
];
/// Two topics: the selector and the indexed `uint64 source`.
pub const TOPICS_LEN: usize = 64;
const WORD: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub struct IntentFulfilledFromSource {
    /// Chain the intent was published on, i.e. this chain's `CHAIN_ID`.
    pub source: u64,
    /// `ProofData` bytes: 8-byte destination followed by (intent hash, claimant) pairs.
    pub encoded_proofs: Vec<u8>,
}

impl IntentFulfilledFromSource {
    pub fn parse(topics: &[u8], unindexed_data: &[u8]) -> Result<Self> {
        require!(
            topics.len() == TOPICS_LEN,
            PolymerProverError::InvalidTopicsLength
        );
        require!(
            topics[..WORD] == INTENT_FULFILLED_FROM_SOURCE_SELECTOR,
            PolymerProverError::InvalidEventSignature
        );
        let source =
            uint64_word(&topics[WORD..TOPICS_LEN]).ok_or(PolymerProverError::InvalidSourceChain)?;
        let encoded_proofs =
            abi_decode_bytes(unindexed_data).ok_or(PolymerProverError::InvalidEventData)?;

        Ok(Self {
            source,
            encoded_proofs,
        })
    }
}

/// Left-pads a 20-byte EVM address to the 32-byte form used by `Config`.
pub fn evm_address_to_bytes32(address: [u8; 20]) -> Bytes32 {
    let mut bytes = [0u8; 32];
    bytes[12..].copy_from_slice(&address);
    bytes.into()
}

/// ABI-encodes a single dynamic `bytes` value (offset word, length word, data
/// padded to a 32-byte boundary). Used by tests to build event data.
pub fn abi_encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * WORD + data.len().div_ceil(WORD) * WORD);
    out.extend_from_slice(&[0u8; WORD - 8]);
    out.extend_from_slice(&(WORD as u64).to_be_bytes());
    out.extend_from_slice(&[0u8; WORD - 8]);
    out.extend_from_slice(&(data.len() as u64).to_be_bytes());
    out.extend_from_slice(data);
    out.resize(out.len() + (data.len().div_ceil(WORD) * WORD - data.len()), 0);
    out
}

/// Decodes a 32-byte ABI word whose value fits in a `u64` (top 24 bytes zero).
fn uint64_word(word: &[u8]) -> Option<u64> {
    if word.len() != WORD || word[..WORD - 8].iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(u64::from_be_bytes(word[WORD - 8..].try_into().ok()?))
}

/// Inverse of [`abi_encode_bytes`]: returns the payload, or `None` if the
/// offset is not 32, or the buffer is shorter than the padded payload.
fn abi_decode_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let offset = uint64_word(data.get(..WORD)?)?;
    if offset != WORD as u64 {
        return None;
    }
    let len = usize::try_from(uint64_word(data.get(WORD..2 * WORD)?)?).ok()?;
    let padded_end = 2 * WORD + len.div_ceil(WORD) * WORD;
    if data.len() < padded_end {
        return None;
    }
    Some(data[2 * WORD..2 * WORD + len].to_vec())
}
```

Add `pub mod event;` to `lib.rs`.

- [ ] **Step 4: Run the tests**

```bash
GOLDIE_UPDATE=1 cargo test -p polymer-prover event::
cargo test -p polymer-prover event::
```

Expected: 10 tests pass. Inspect `programs/polymer-prover/src/event/testdata/abi_encode_bytes_pads_to_word.golden`: the empty case is 64 bytes, the 3-byte case is 96 bytes ending in 29 zeros, the 32-byte case is 96 bytes with no padding.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add programs/polymer-prover
git commit -m "feat(polymer-prover): decode IntentFulfilledFromSource topics and ABI data

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 4: Localnet mock of Polymer's program and test-context builders

**Files:**
- Create: `programs/mock-polymer-prover/Cargo.toml`
- Create: `programs/mock-polymer-prover/src/lib.rs`
- Create: `integration-tests/tests/common/polymer_prover_context.rs`
- Modify: `Anchor.toml` (`[programs.localnet]` only)
- Modify: `Cargo.toml` (`[workspace.dependencies]`)
- Modify: `integration-tests/Cargo.toml`
- Modify: `integration-tests/tests/common/mod.rs:33-45,70-85`

**Interfaces:**
- Produces (mock program): instructions `create_accounts`, `load_proof(proof_chunk: Vec<u8>)`, `validate_event`, `close_accounts` with Polymer's account layouts; `mock_polymer_prover::ValidationResultAccount` (pub, `AnchorSerialize`).
- Produces (test context): `Context::polymer_prover() -> PolymerProver<'_>` with `polymer_create_accounts(&Keypair)`, `polymer_load_result(&Keypair, &ValidationResultAccount)`, and the helper `intent_fulfilled_result(emitter: [u8; 20], source: u64, chain_id: u32, encoded_proofs: Vec<u8>) -> ValidationResultAccount`. Later tasks add `init`, `validate`, `prove`, `close_proof` to the same `impl`.

- [ ] **Step 1: Create the mock program**

`programs/mock-polymer-prover/Cargo.toml`:

```toml
[package]
description = "Localnet stand-in for Polymer's polymer_prover program"
edition = "2021"
name = "mock-polymer-prover"
version = "0.1.0"

[lib]
crate-type = ["cdylib", "lib"]
name = "mock_polymer_prover"

[features]
cpi = ["no-entrypoint"]
default = []
idl-build = ["anchor-lang/idl-build"]
no-entrypoint = []
no-idl = []
no-log-ix-name = []

[dependencies]
anchor-lang = { workspace = true }
```

`programs/mock-polymer-prover/src/lib.rs`:

```rust
//! Test-only stand-in for Polymer's deployed `polymer_prover` program
//! (localnet only; excluded from devnet/mainnet builds). Instruction names and
//! account layouts match polymerdao/solana-prover-contracts v1.0.4 so the Anchor
//! discriminators and PDA seeds polymer-prover mirrors resolve here unchanged.
//!
//! Instead of verifying a proof, `validate_event` Borsh-decodes the bytes
//! accumulated in the cache as a `ValidationResultAccount` body and stores it,
//! so a test decides exactly which EVM event "was proven".

use anchor_lang::prelude::*;

// Polymer's devnet program ID: what polymer-prover targets in non-mainnet builds.
declare_id!("FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3");

const DISCRIMINATOR_SIZE: usize = 8;

#[account]
#[derive(InitSpace)]
pub struct ProofCacheAccount {
    #[max_len(3000)]
    pub cache: Vec<u8>,
}

#[account]
#[derive(InitSpace, Default)]
pub struct ValidationResultAccount {
    pub is_valid: bool,
    #[max_len(64)]
    pub error_message: String,
    pub chain_id: u32,
    pub emitting_contract: [u8; 20],
    #[max_len(32 * 4)]
    pub topics: Vec<u8>,
    #[max_len(3000)]
    pub unindexed_data: Vec<u8>,
}

#[derive(Accounts)]
pub struct CreateAccounts<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        seeds = [b"cache", authority.key().as_ref()],
        bump,
        payer = authority,
        space = DISCRIMINATOR_SIZE + ProofCacheAccount::INIT_SPACE,
    )]
    pub cache_account: Account<'info, ProofCacheAccount>,
    #[account(
        init,
        seeds = [b"result", authority.key().as_ref()],
        bump,
        payer = authority,
        space = DISCRIMINATOR_SIZE + ValidationResultAccount::INIT_SPACE,
    )]
    pub result_account: Account<'info, ValidationResultAccount>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseAccounts<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, close = authority, seeds = [b"cache", authority.key().as_ref()], bump)]
    pub cache_account: Account<'info, ProofCacheAccount>,
    #[account(mut, close = authority, seeds = [b"result", authority.key().as_ref()], bump)]
    pub result_account: Account<'info, ValidationResultAccount>,
}

#[derive(Accounts)]
pub struct LoadProof<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, seeds = [b"cache", authority.key().as_ref()], bump)]
    pub cache_account: Account<'info, ProofCacheAccount>,
}

#[derive(Accounts)]
pub struct ValidateEvent<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, seeds = [b"cache", authority.key().as_ref()], bump)]
    pub cache_account: Account<'info, ProofCacheAccount>,
    #[account(mut, seeds = [b"result", authority.key().as_ref()], bump)]
    pub result_account: Account<'info, ValidationResultAccount>,
    /// CHECK: seeds only; the real program reads sequencer config from here,
    /// the mock needs nothing.
    #[account(seeds = [b"internal"], bump)]
    pub internal: UncheckedAccount<'info>,
}

#[program]
pub mod mock_polymer_prover {
    use super::*;

    pub fn create_accounts(_ctx: Context<CreateAccounts>) -> Result<()> {
        Ok(())
    }

    pub fn close_accounts(_ctx: Context<CloseAccounts>) -> Result<()> {
        Ok(())
    }

    pub fn load_proof(ctx: Context<LoadProof>, proof_chunk: Vec<u8>) -> Result<()> {
        ctx.accounts.cache_account.cache.extend(proof_chunk);
        Ok(())
    }

    pub fn validate_event(ctx: Context<ValidateEvent>) -> Result<()> {
        let mut body = ctx.accounts.cache_account.cache.as_slice();
        let result: ValidationResultAccount = AnchorDeserialize::deserialize(&mut body)?;
        ctx.accounts.result_account.set_inner(result);
        ctx.accounts.cache_account.cache.clear();
        Ok(())
    }
}
```

- [ ] **Step 2: Wire it into the workspace, Anchor.toml and the test crate**

`Cargo.toml` workspace deps (alphabetical): `mock-polymer-prover = { path = "programs/mock-polymer-prover" }`.

`Anchor.toml` `[programs.localnet]` only: `mock-polymer-prover = "FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3"`. Do **not** add it to devnet/mainnet sections or scripts.

`integration-tests/Cargo.toml` dependencies (alphabetical):

```toml
mock-polymer-prover = { workspace = true, features = ["no-entrypoint"] }
polymer-prover = { workspace = true, features = ["no-entrypoint"] }
```

`integration-tests/tests/common/mod.rs`: add after the other `mod` lines

```rust
pub mod polymer_prover_context;
```

add to the `include_bytes!` block

```rust
const POLYMER_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/polymer_prover.so");
const MOCK_POLYMER_PROVER_BIN: &[u8] =
    include_bytes!("../../../target/deploy/mock_polymer_prover.so");
```

and in `Context::default()` after the `malicious_proof_closer` line

```rust
        svm.add_program(polymer_prover::ID, POLYMER_PROVER_BIN).unwrap();
        // The mock declares Polymer's devnet ID, which is what non-mainnet
        // polymer-prover builds CPI into.
        svm.add_program(
            polymer_prover::polymer::POLYMER_PROVER_ID,
            MOCK_POLYMER_PROVER_BIN,
        )
        .unwrap();
```

- [ ] **Step 3: Write the test context**

Create `integration-tests/tests/common/polymer_prover_context.rs`:

```rust
use anchor_lang::{AnchorSerialize, InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::{abi_encode_bytes, INTENT_FULFILLED_FROM_SOURCE_SELECTOR};
use polymer_prover::polymer;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult};

/// Polymer's `load_proof` chunk size recommendation.
const LOAD_PROOF_CHUNK: usize = 800;

#[derive(Deref, DerefMut)]
pub struct PolymerProver<'a>(&'a mut Context);

impl Context {
    pub fn polymer_prover(&mut self) -> PolymerProver<'_> {
        PolymerProver(self)
    }
}

/// Builds the result Polymer would write for an `IntentFulfilledFromSource`
/// event emitted by `emitter` on EVM chain `chain_id`, with `source` in the
/// indexed topic and `encoded_proofs` ABI-encoded as the `bytes` argument.
pub fn intent_fulfilled_result(
    emitter: [u8; 20],
    source: u64,
    chain_id: u32,
    encoded_proofs: Vec<u8>,
) -> ValidationResultAccount {
    let mut topics = INTENT_FULFILLED_FROM_SOURCE_SELECTOR.to_vec();
    topics.extend_from_slice(&[0u8; 24]);
    topics.extend_from_slice(&source.to_be_bytes());

    ValidationResultAccount {
        is_valid: true,
        error_message: String::new(),
        chain_id,
        emitting_contract: emitter,
        topics,
        unindexed_data: abi_encode_bytes(&encoded_proofs),
    }
}

impl PolymerProver<'_> {
    /// Polymer's `create_accounts` for `authority`; funds the authority first.
    pub fn polymer_create_accounts(&mut self, authority: &Keypair) -> TransactionResult {
        if self.balance(&authority.pubkey()) == 0 {
            self.airdrop(&authority.pubkey(), super::sol_amount(5.0))
                .unwrap();
        }
        let instruction = Instruction {
            program_id: polymer::POLYMER_PROVER_ID,
            accounts: mock_polymer_prover::accounts::CreateAccounts {
                authority: authority.pubkey(),
                cache_account: polymer::cache_pda(&authority.pubkey()).0,
                result_account: polymer::result_pda(&authority.pubkey()).0,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: mock_polymer_prover::instruction::CreateAccounts {}.data(),
        };
        let transaction = Transaction::new(
            &[authority],
            Message::new(&[instruction], Some(&authority.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// Loads `result` into the mock's cache in 800-byte chunks, one
    /// transaction each, exactly as a relayer loads a real proof.
    pub fn polymer_load_result(
        &mut self,
        authority: &Keypair,
        result: &ValidationResultAccount,
    ) -> TransactionResult {
        let body = result.try_to_vec().unwrap();
        let mut last = None;
        for chunk in body.chunks(LOAD_PROOF_CHUNK) {
            let instruction = Instruction {
                program_id: polymer::POLYMER_PROVER_ID,
                accounts: mock_polymer_prover::accounts::LoadProof {
                    authority: authority.pubkey(),
                    cache_account: polymer::cache_pda(&authority.pubkey()).0,
                }
                .to_account_metas(None),
                data: mock_polymer_prover::instruction::LoadProof {
                    proof_chunk: chunk.to_vec(),
                }
                .data(),
            };
            let transaction = Transaction::new(
                &[authority],
                Message::new(&[instruction], Some(&authority.pubkey())),
                self.latest_blockhash(),
            );
            last = Some(self.send_transaction(transaction)?);
        }

        Ok(last.expect("result body is never empty"))
    }
}
```

- [ ] **Step 4: Write a smoke test for the mock and run it**

Create `integration-tests/tests/validate_polymer_prover.rs` with only this test for now (Task 5 adds the rest):

```rust
use anchor_lang::AccountDeserialize;
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::polymer;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::polymer_prover_context::intent_fulfilled_result;

pub mod common;

#[test]
fn mock_polymer_prover_is_at_the_id_polymer_prover_targets() {
    assert_eq!(mock_polymer_prover::ID, polymer::POLYMER_PROVER_ID);
}

#[test]
fn mock_polymer_load_result_roundtrip() {
    let mut ctx = common::Context::default();
    let authority = Keypair::new();
    ctx.polymer_prover()
        .polymer_create_accounts(&authority)
        .unwrap();

    let result = intent_fulfilled_result([0xab; 20], 7, 8453, vec![1u8; 72]);
    ctx.polymer_prover()
        .polymer_load_result(&authority, &result)
        .unwrap();

    let cache = ctx
        .get_account(&polymer::cache_pda(&authority.pubkey()).0)
        .unwrap();
    let cache = mock_polymer_prover::ProofCacheAccount::try_deserialize(&mut cache.data.as_slice())
        .unwrap();
    assert_eq!(cache.cache, anchor_lang::AnchorSerialize::try_to_vec(&result).unwrap());
    let _: ValidationResultAccount = ctx
        .account(&polymer::result_pda(&authority.pubkey()).0)
        .unwrap();
}
```

```bash
anchor build
cargo test --test validate_polymer_prover
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt && cargo sort --workspace && cargo clippy --all-targets -- -D warnings
git add programs/mock-polymer-prover Anchor.toml Cargo.toml Cargo.lock integration-tests
git commit -m "test(polymer-prover): add localnet mock of Polymer's program and test context

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 5: `validate` instruction with happy-path and idempotency tests

**Files:**
- Create: `programs/polymer-prover/src/instructions/validate.rs`
- Modify: `programs/polymer-prover/src/instructions/mod.rs` (uncomment `validate`)
- Modify: `programs/polymer-prover/src/lib.rs`
- Modify: `integration-tests/tests/common/polymer_prover_context.rs`
- Modify: `integration-tests/tests/validate_polymer_prover.rs`
- Create: `integration-tests/tests/init_polymer_prover.rs`

**Interfaces:**
- Consumes: `polymer::{validate_event, ValidationResult, cache_pda, result_pda, internal_pda, POLYMER_PROVER_ID}`, `event::{IntentFulfilledFromSource, evm_address_to_bytes32}`, `Config`, `ProofAccount`.
- Produces: instruction `validate` with accounts `Validate { authority, config, cache_account, result_account, internal, polymer_prover_program, system_program, event_authority, program }` plus Proof PDAs in `remaining_accounts`; context builders `init(Vec<Bytes32>, Pubkey)` and `validate(&Keypair, Vec<AccountMeta>)`.

- [ ] **Step 1: Add `init` and `validate` builders to the context**

Append to the `impl PolymerProver<'_>` in `polymer_prover_context.rs` (add the imports `use anchor_lang::prelude::AccountMeta; use eco_svm_std::{event_authority_pda, Bytes32}; use solana_compute_budget_interface::ComputeBudgetInstruction; use solana_sdk::pubkey::Pubkey;`):

```rust
    /// Compute limit for `validate`: Polymer's real `validate_event` needs
    /// close to the 1.4M transaction maximum.
    pub const VALIDATE_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

    pub fn init(&mut self, whitelisted_emitters: Vec<Bytes32>, config: Pubkey) -> TransactionResult {
        let instruction = Instruction {
            program_id: polymer_prover::ID,
            accounts: polymer_prover::accounts::Init {
                config,
                payer: self.payer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: polymer_prover::instruction::Init {
                args: polymer_prover::instructions::InitArgs {
                    whitelisted_emitters,
                },
            }
            .data(),
        };
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// `validate` for the proof loaded under `authority`; `proof_accounts` are
    /// the Proof PDAs in payload order.
    pub fn validate(
        &mut self,
        authority: &Keypair,
        proof_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let accounts = polymer_prover::accounts::Validate {
            authority: authority.pubkey(),
            config: polymer_prover::state::Config::pda().0,
            cache_account: polymer::cache_pda(&authority.pubkey()).0,
            result_account: polymer::result_pda(&authority.pubkey()).0,
            internal: polymer::internal_pda().0,
            polymer_prover_program: polymer::POLYMER_PROVER_ID,
            system_program: anchor_lang::system_program::ID,
            event_authority: event_authority_pda(&polymer_prover::ID).0,
            program: polymer_prover::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(proof_accounts)
        .collect();
        let instruction = Instruction {
            program_id: polymer_prover::ID,
            accounts,
            data: polymer_prover::instruction::Validate {}.data(),
        };
        let transaction = Transaction::new(
            &[authority],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(
                        Self::VALIDATE_COMPUTE_UNIT_LIMIT,
                    ),
                    instruction,
                ],
                Some(&authority.pubkey()),
            ),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
```

- [ ] **Step 2: Write the failing init tests**

Create `integration-tests/tests/init_polymer_prover.rs`:

```rust
use anchor_lang::error::ErrorCode;
use eco_svm_std::Bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::state::Config;
use solana_sdk::pubkey::Pubkey;

pub mod common;

fn emitter(byte: u8) -> Bytes32 {
    polymer_prover::event::evm_address_to_bytes32([byte; 20])
}

#[test]
fn init_polymer_prover_success() {
    let mut ctx = common::Context::default();
    let emitters = vec![emitter(1), emitter(2)];

    let result = ctx.polymer_prover().init(emitters.clone(), Config::pda().0);
    assert!(result.is_ok());

    let config: Config = ctx.account(&Config::pda().0).unwrap();
    assert_eq!(config.whitelisted_emitters, emitters);
    assert!(config.is_whitelisted(&emitter(1)));
    assert!(!config.is_whitelisted(&emitter(3)));
}

#[test]
fn init_polymer_prover_invalid_config_fail() {
    let mut ctx = common::Context::default();

    let result = ctx
        .polymer_prover()
        .init(vec![emitter(1)], Pubkey::new_unique());
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidConfig)));
}

#[test]
fn init_polymer_prover_already_initialized_fail() {
    let mut ctx = common::Context::default();
    ctx.polymer_prover()
        .init(vec![emitter(1)], Config::pda().0)
        .unwrap();

    let result = ctx.polymer_prover().init(vec![emitter(1)], Config::pda().0);
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}
```

- [ ] **Step 3: Write the failing validate tests**

Replace `integration-tests/tests/validate_polymer_prover.rs` with (keep the two mock tests from Task 4 at the bottom):

```rust
use anchor_lang::prelude::AccountMeta;
use anchor_lang::AccountDeserialize;
use eco_svm_std::prover::{self, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::{Bytes32, CHAIN_ID};
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::evm_address_to_bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::polymer;
use polymer_prover::state::{Config, ProofAccount};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::polymer_prover_context::intent_fulfilled_result;

pub mod common;

/// Whitelisted EVM PolymerProver address used by every test.
const EMITTER: [u8; 20] = [0xec; 20];
/// EVM chain the fulfilments happened on.
const EVM_CHAIN_ID: u32 = 8453;

struct Fixture {
    ctx: common::Context,
    authority: Keypair,
}

fn setup() -> Fixture {
    let mut ctx = common::Context::default();
    ctx.polymer_prover()
        .init(vec![evm_address_to_bytes32(EMITTER)], Config::pda().0)
        .unwrap();
    let authority = Keypair::new();
    ctx.polymer_prover()
        .polymer_create_accounts(&authority)
        .unwrap();

    Fixture { ctx, authority }
}

fn rand_pairs(count: usize) -> Vec<IntentHashClaimant> {
    (0..count)
        .map(|_| {
            IntentHashClaimant::new(
                random::<[u8; 32]>().into(),
                Pubkey::new_unique().to_bytes().into(),
            )
        })
        .collect()
}

fn proof_metas(pairs: &[IntentHashClaimant]) -> Vec<AccountMeta> {
    pairs
        .iter()
        .map(|pair| AccountMeta::new(Proof::pda(&pair.intent_hash, &polymer_prover::ID).0, false))
        .collect()
}

/// A well-formed event for `pairs`, source = this chain, destination = EVM chain.
fn event_for(pairs: &[IntentHashClaimant]) -> ValidationResultAccount {
    intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), pairs.to_vec()).to_bytes(),
    )
}

fn load_and_validate(
    fixture: &mut Fixture,
    event: &ValidationResultAccount,
    proof_accounts: Vec<AccountMeta>,
) -> common::TransactionResult {
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, event)
        .unwrap();
    fixture
        .ctx
        .polymer_prover()
        .validate(&fixture.authority, proof_accounts)
}

#[test]
fn validate_success() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let authority_balance_before = fixture.ctx.balance(&fixture.authority.pubkey());

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));

    let claimant = Pubkey::new_from_array(pairs[0].claimant.into());
    assert!(result.is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
        pairs[0].intent_hash,
        claimant,
        EVM_CHAIN_ID.into(),
    ))));
    let proof: ProofAccount = fixture
        .ctx
        .account(&Proof::pda(&pairs[0].intent_hash, &polymer_prover::ID).0)
        .unwrap();
    assert_eq!(proof.0.destination, u64::from(EVM_CHAIN_ID));
    assert_eq!(proof.0.claimant, claimant);
    // The relayer paid the Proof rent.
    assert!(fixture.ctx.balance(&fixture.authority.pubkey()) < authority_balance_before);
    // Polymer cleared the cache.
    let cache = fixture
        .ctx
        .get_account(&polymer::cache_pda(&fixture.authority.pubkey()).0)
        .unwrap();
    let cache =
        mock_polymer_prover::ProofCacheAccount::try_deserialize(&mut cache.data.as_slice()).unwrap();
    assert!(cache.cache.is_empty());
}

#[test]
fn validate_multiple_success() {
    let mut fixture = setup();
    let pairs = rand_pairs(3);

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));
    assert!(result.is_ok());

    for pair in &pairs {
        let proof: ProofAccount = fixture
            .ctx
            .account(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
            .unwrap();
        assert_eq!(proof.0.claimant, Pubkey::new_from_array(pair.claimant.into()));
        assert!(result.clone().is_ok_and(common::contains_cpi_event(
            prover::IntentProven::new(
                pair.intent_hash,
                Pubkey::new_from_array(pair.claimant.into()),
                EVM_CHAIN_ID.into(),
            )
        )));
    }
}

#[test]
fn validate_revalidation_is_idempotent() {
    let mut fixture = setup();
    let pairs = rand_pairs(2);
    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();

    // Same proof again: no-op for every pair, event re-emitted.
    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));
    assert!(result.clone().is_ok());
    assert!(result.is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
        pairs[1].intent_hash,
        Pubkey::new_from_array(pairs[1].claimant.into()),
        EVM_CHAIN_ID.into(),
    ))));
}

#[test]
fn validate_disagreeing_claimant_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();

    let conflicting = vec![IntentHashClaimant::new(
        pairs[0].intent_hash,
        Pubkey::new_unique().to_bytes().into(),
    )];
    let result = load_and_validate(&mut fixture, &event_for(&conflicting), proof_metas(&conflicting));
    assert!(result.is_err_and(common::is_error(PolymerProverError::IntentAlreadyProven)));
}

#[test]
fn validate_batch_with_one_already_proven_succeeds() {
    let mut fixture = setup();
    let first = rand_pairs(1);
    load_and_validate(&mut fixture, &event_for(&first), proof_metas(&first)).unwrap();

    let mut batch = first.clone();
    batch.extend(rand_pairs(2));
    let result = load_and_validate(&mut fixture, &event_for(&batch), proof_metas(&batch));
    assert!(result.is_ok());
    for pair in &batch {
        assert!(fixture
            .ctx
            .account::<ProofAccount>(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
            .is_some());
    }
}
```

Keep `mock_polymer_prover_is_at_the_id_polymer_prover_targets` and `mock_polymer_load_result_roundtrip` from Task 4 below these (they need `Bytes32`? no; drop unused imports if clippy complains).

- [ ] **Step 4: Run to verify failure**

```bash
anchor build && cargo test --test init_polymer_prover --test validate_polymer_prover
```

Expected: compile error, `polymer_prover::accounts::Validate` not found.

- [ ] **Step 5: Write `instructions/validate.rs`**

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{self, IntentHashClaimant, IntentProven, ProofData, PROOF_SEED};
use eco_svm_std::CHAIN_ID;

use crate::event::{evm_address_to_bytes32, IntentFulfilledFromSource};
use crate::instructions::PolymerProverError;
use crate::polymer::{self, ValidationResult};
use crate::state::{Config, ProofAccount};

#[event_cpi]
#[derive(Accounts)]
pub struct Validate<'info> {
    /// The relayer. Polymer's cache/result PDAs are derived from this key and
    /// it pays the Proof rent.
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(address = Config::pda().0 @ PolymerProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// CHECK: address is validated; Polymer owns it and checks its seeds in the CPI
    #[account(mut, address = polymer::cache_pda(&authority.key()).0 @ PolymerProverError::InvalidCacheAccount)]
    pub cache_account: UncheckedAccount<'info>,
    /// CHECK: address is validated; owner and discriminator are validated after the CPI
    #[account(mut, address = polymer::result_pda(&authority.key()).0 @ PolymerProverError::InvalidResultAccount)]
    pub result_account: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(address = polymer::internal_pda().0 @ PolymerProverError::InvalidInternalAccount)]
    pub internal: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(executable, address = polymer::POLYMER_PROVER_ID @ PolymerProverError::InvalidPolymerProver)]
    pub polymer_prover_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn validate<'info>(ctx: Context<'info, Validate<'info>>) -> Result<()> {
    polymer::validate_event(
        &ctx.accounts.polymer_prover_program,
        &ctx.accounts.authority,
        &ctx.accounts.cache_account,
        &ctx.accounts.result_account,
        &ctx.accounts.internal,
    )?;

    // Read in the same instruction as the CPI: Polymer overwrites the result on
    // every `validate_event`, so this is this proof's outcome and no other.
    let result = ValidationResult::try_from_account_info(&ctx.accounts.result_account)?;
    if !result.is_valid {
        msg!("polymer: {}", result.error_message);
        return Err(PolymerProverError::PolymerProofInvalid.into());
    }

    require!(
        ctx.accounts
            .config
            .is_whitelisted(&evm_address_to_bytes32(result.emitting_contract)),
        PolymerProverError::InvalidEmittingContract
    );

    let event = IntentFulfilledFromSource::parse(&result.topics, &result.unindexed_data)?;
    require!(event.source == CHAIN_ID, PolymerProverError::InvalidSourceChain);

    let proof_data = ProofData::from_bytes(&event.encoded_proofs)
        .map_err(|_| PolymerProverError::InvalidEventData)?;
    require!(
        proof_data.destination == u64::from(result.chain_id),
        PolymerProverError::InvalidDestinationChain
    );
    require!(
        !proof_data.intent_hashes_claimants.is_empty(),
        PolymerProverError::EmptyProofData
    );

    mark_intent_hashes_proven(&ctx, proof_data)
}

fn mark_intent_hashes_proven<'info>(
    ctx: &Context<'info, Validate<'info>>,
    proof_data: ProofData,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() == proof_data.intent_hashes_claimants.len(),
        PolymerProverError::InvalidProof
    );

    ctx.remaining_accounts
        .iter()
        .zip(proof_data.intent_hashes_claimants)
        .try_for_each(|(proof, intent_hash_claimant)| {
            mark_intent_hash_proven(ctx, proof, proof_data.destination, intent_hash_claimant)
        })
}

fn mark_intent_hash_proven<'info>(
    ctx: &Context<'info, Validate<'info>>,
    proof: &AccountInfo<'info>,
    destination: u64,
    intent_hash_claimant: IntentHashClaimant,
) -> Result<()> {
    let IntentHashClaimant {
        intent_hash,
        claimant,
    } = intent_hash_claimant;
    let claimant = Pubkey::new_from_array(claimant.into());

    let (proof_pda, bump) = prover::Proof::pda(&intent_hash, &crate::ID);
    require!(proof.key() == proof_pda, PolymerProverError::InvalidProof);
    let proof_signer_seeds = [PROOF_SEED, intent_hash.as_ref(), &[bump]];

    // A Polymer proof can be re-validated any number of times and a later EVM
    // `prove()` may re-include an already-proven hash, so reaching the recorded
    // state again is a no-op; only a state that disagrees is an error. The
    // event repeats either way: it asserts the recorded state, not a transition.
    match prover::Proof::try_from_account_info(proof)? {
        Some(recorded) => require!(
            recorded.destination == destination && recorded.claimant == claimant,
            PolymerProverError::IntentAlreadyProven
        ),
        None => ProofAccount::from(prover::Proof::new(destination, claimant)).init(
            proof,
            &ctx.accounts.authority,
            &ctx.accounts.system_program,
            &[&proof_signer_seeds],
        )?,
    }

    emit_cpi!(IntentProven::new(intent_hash, claimant, destination));

    Ok(())
}
```

Note on `Proof::try_from_account_info`: it returns `Ok(None)` for an empty account (no data) and `Some` when data is present. A freshly created but not-yet-owned account has no data, so this branch is correct. If the account has data but is not owned by us, `require!(proof.key() == proof_pda)` has already bound it to our PDA, which only we can create.

`instructions/mod.rs`: uncomment `mod validate;` and `pub use validate::*;`.

`lib.rs`: add

```rust
    pub fn validate<'info>(ctx: Context<'info, Validate<'info>>) -> Result<()> {
        instructions::validate(ctx)
    }
```

- [ ] **Step 6: Build and run**

```bash
anchor build && cargo test --test init_polymer_prover --test validate_polymer_prover
```

Expected: all tests pass (3 init, 5 validate, 2 mock).

- [ ] **Step 7: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add programs/polymer-prover integration-tests
git commit -m "feat(polymer-prover): validate Polymer proofs of IntentFulfilledFromSource into Proof PDAs

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 6: `validate` rejection tests

**Files:**
- Modify: `integration-tests/tests/validate_polymer_prover.rs`

**Interfaces:**
- Consumes: everything from Task 5. Produces nothing new; each test pins one error path.

- [ ] **Step 1: Add the tests**

Append to `validate_polymer_prover.rs`:

```rust
#[test]
fn validate_polymer_invalid_result_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.is_valid = false;
    event.error_message = "invalid membership proof: can't read path".into();

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::PolymerProofInvalid)));
}

#[test]
fn validate_non_whitelisted_emitter_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.emitting_contract = [0x11; 20];

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEmittingContract)));
}

#[test]
fn validate_wrong_topics_length_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.topics.extend_from_slice(&[0u8; 32]);

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidTopicsLength)));
}

#[test]
fn validate_wrong_event_signature_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.topics[0] ^= 0xff;

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEventSignature)));
}

#[test]
fn validate_wrong_source_chain_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID + 1,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), pairs.clone()).to_bytes(),
    );

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidSourceChain)));
}

#[test]
fn validate_destination_mismatch_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    // Payload claims destination 10, Polymer says the event came from 8453.
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        EVM_CHAIN_ID,
        ProofData::new(10, pairs.clone()).to_bytes(),
    );

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidDestinationChain)));
}

#[test]
fn validate_malformed_abi_data_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.unindexed_data.truncate(40);

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEventData)));
}

#[test]
fn validate_unaligned_pairs_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut encoded = ProofData::new(EVM_CHAIN_ID.into(), pairs.clone()).to_bytes();
    encoded.push(0);
    let event = intent_fulfilled_result(EMITTER, CHAIN_ID, EVM_CHAIN_ID, encoded);

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEventData)));
}

#[test]
fn validate_empty_pairs_fail() {
    let mut fixture = setup();
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), vec![]).to_bytes(),
    );

    let result = load_and_validate(&mut fixture, &event, vec![]);
    assert!(result.is_err_and(common::is_error(PolymerProverError::EmptyProofData)));
}

#[test]
fn validate_proof_account_count_mismatch_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(2);

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs[..1]));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidProof)));
}

#[test]
fn validate_wrong_proof_pda_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let wrong = vec![AccountMeta::new(Pubkey::new_unique(), false)];

    let result = load_and_validate(&mut fixture, &event_for(&pairs), wrong);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidProof)));
}

#[test]
fn validate_wrong_polymer_program_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, &event_for(&pairs))
        .unwrap();

    // Hand-build the instruction with local-prover in the Polymer program slot.
    let authority = &fixture.authority;
    let mut accounts = polymer_prover::accounts::Validate {
        authority: authority.pubkey(),
        config: Config::pda().0,
        cache_account: polymer::cache_pda(&authority.pubkey()).0,
        result_account: polymer::result_pda(&authority.pubkey()).0,
        internal: polymer::internal_pda().0,
        polymer_prover_program: local_prover::ID,
        system_program: anchor_lang::system_program::ID,
        event_authority: eco_svm_std::event_authority_pda(&polymer_prover::ID).0,
        program: polymer_prover::ID,
    }
    .to_account_metas(None);
    accounts.extend(proof_metas(&pairs));
    let instruction = solana_sdk::instruction::Instruction {
        program_id: polymer_prover::ID,
        accounts,
        data: polymer_prover::instruction::Validate {}.data(),
    };
    let transaction = solana_sdk::transaction::Transaction::new(
        &[authority],
        solana_sdk::message::Message::new(&[instruction], Some(&authority.pubkey())),
        fixture.ctx.latest_blockhash(),
    );

    let result = fixture.ctx.send_transaction(transaction);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidPolymerProver)));
}

#[test]
fn validate_result_account_for_other_authority_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let other = Keypair::new();
    fixture
        .ctx
        .polymer_prover()
        .polymer_create_accounts(&other)
        .unwrap();
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&other, &event_for(&pairs))
        .unwrap();

    // Authority signs, but points at `other`'s result account.
    let authority = &fixture.authority;
    let mut accounts = polymer_prover::accounts::Validate {
        authority: authority.pubkey(),
        config: Config::pda().0,
        cache_account: polymer::cache_pda(&authority.pubkey()).0,
        result_account: polymer::result_pda(&other.pubkey()).0,
        internal: polymer::internal_pda().0,
        polymer_prover_program: polymer::POLYMER_PROVER_ID,
        system_program: anchor_lang::system_program::ID,
        event_authority: eco_svm_std::event_authority_pda(&polymer_prover::ID).0,
        program: polymer_prover::ID,
    }
    .to_account_metas(None);
    accounts.extend(proof_metas(&pairs));
    let instruction = solana_sdk::instruction::Instruction {
        program_id: polymer_prover::ID,
        accounts,
        data: polymer_prover::instruction::Validate {}.data(),
    };
    let transaction = solana_sdk::transaction::Transaction::new(
        &[authority],
        solana_sdk::message::Message::new(&[instruction], Some(&authority.pubkey())),
        fixture.ctx.latest_blockhash(),
    );

    let result = fixture.ctx.send_transaction(transaction);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidResultAccount)));
}
```

Add `use anchor_lang::{InstructionData, ToAccountMetas};` to the imports.

- [ ] **Step 2: Run**

```bash
cargo test --test validate_polymer_prover
```

Expected: every test passes. If one fails on the error variant, check the order of checks in `validate.rs` against spec section 3.4 and fix the implementation, not the test.

- [ ] **Step 3: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add integration-tests
git commit -m "test(polymer-prover): cover every validate rejection path

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 7: `prove` instruction emitting Polymer-provable logs

**Files:**
- Create: `programs/polymer-prover/src/instructions/prove.rs`
- Modify: `programs/polymer-prover/src/instructions/mod.rs`, `programs/polymer-prover/src/lib.rs`
- Modify: `integration-tests/tests/common/polymer_prover_context.rs`
- Create: `integration-tests/tests/prove_polymer_prover.rs`

**Interfaces:**
- Consumes: `eco_svm_std::prover::{ProveArgs, ProofData, IntentHashClaimant}`, `portal::state::dispatcher_pda`.
- Produces: `MAX_INTENTS_PER_PROVE: usize = 32`, `PROVE_LOG_PAYLOAD_LEN: usize = 160`, `prove_log_payload(source: u64, destination: u64, pair: &IntentHashClaimant) -> [u8; 160]`, `check_prove_args(&ProofData) -> Result<()>`, `prove_log_line(program_id: &Pubkey, payload: &[u8; 160]) -> String` (test helper, `pub`).

- [ ] **Step 1: Write the failing unit tests**

Create `instructions/prove.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> IntentHashClaimant {
        IntentHashClaimant::new([0x11; 32].into(), [0x22; 32].into())
    }

    #[test]
    fn prove_log_payload_layout() {
        let payload = prove_log_payload(8453, CHAIN_ID, &pair());
        goldie::assert_debug!(core::str::from_utf8(&payload).unwrap());
    }

    #[test]
    fn prove_log_line_format() {
        let payload = prove_log_payload(8453, CHAIN_ID, &pair());
        let line = prove_log_line(&crate::ID, &payload);
        assert!(line.starts_with(&format!("Prove: program: {}, ", crate::ID)));
        assert!(line.len() < 500);
        goldie::assert_debug!(line);
    }

    #[test]
    fn check_prove_args_rejects_wrong_destination() {
        let proof_data = ProofData::new(CHAIN_ID + 1, vec![pair()]);
        assert_eq!(
            check_prove_args(&proof_data).unwrap_err(),
            PolymerProverError::InvalidDestination.into()
        );
    }

    #[test]
    fn check_prove_args_rejects_empty() {
        let proof_data = ProofData::new(CHAIN_ID, vec![]);
        assert_eq!(
            check_prove_args(&proof_data).unwrap_err(),
            PolymerProverError::EmptyProofData.into()
        );
    }

    #[test]
    fn check_prove_args_rejects_too_many() {
        let proof_data = ProofData::new(CHAIN_ID, vec![pair(); MAX_INTENTS_PER_PROVE + 1]);
        assert_eq!(
            check_prove_args(&proof_data).unwrap_err(),
            PolymerProverError::TooManyIntents.into()
        );
    }

    #[test]
    fn check_prove_args_accepts_max() {
        let proof_data = ProofData::new(CHAIN_ID, vec![pair(); MAX_INTENTS_PER_PROVE]);
        assert!(check_prove_args(&proof_data).is_ok());
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p polymer-prover prove::
```

Expected: compile error (module not declared / functions missing).

- [ ] **Step 3: Write the implementation**

Above the tests in `instructions/prove.rs`:

```rust
use anchor_lang::prelude::*;
use eco_svm_std::prover::{IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::CHAIN_ID;

use crate::instructions::PolymerProverError;

/// Solana truncates a transaction's log buffer at 10 KB and a truncated log can
/// never be proven; 32 lines of ~222 bytes leave room for Portal's own logs.
pub const MAX_INTENTS_PER_PROVE: usize = 32;
/// hex(source u64 ‖ destination u64 ‖ intent_hash ‖ claimant) = 2 * 80.
pub const PROVE_LOG_PAYLOAD_LEN: usize = 160;
const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Accounts)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda(&crate::ID).0 @ PolymerProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
}

/// Emits one Polymer-provable log line per intent:
/// `Prove: program: <base58 program id>, <160 hex chars>`. The EVM
/// `PolymerProver.validateSolana` parses these back; the layout is a shared ABI.
pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        domain_id,
        proof_data,
        ..
    } = args;

    check_prove_args(&proof_data)?;

    for pair in &proof_data.intent_hashes_claimants {
        let payload = prove_log_payload(domain_id, CHAIN_ID, pair);
        msg!(
            "Prove: program: {}, {}",
            ctx.program_id,
            core::str::from_utf8(&payload).expect("hex is ascii")
        );
    }

    Ok(())
}

pub fn check_prove_args(proof_data: &ProofData) -> Result<()> {
    require!(
        proof_data.destination == CHAIN_ID,
        PolymerProverError::InvalidDestination
    );
    require!(
        !proof_data.intent_hashes_claimants.is_empty(),
        PolymerProverError::EmptyProofData
    );
    require!(
        proof_data.intent_hashes_claimants.len() <= MAX_INTENTS_PER_PROVE,
        PolymerProverError::TooManyIntents
    );
    Ok(())
}

/// Lowercase hex of `source ‖ destination ‖ intent_hash ‖ claimant` (80 bytes),
/// encoded into a stack buffer.
pub fn prove_log_payload(
    source: u64,
    destination: u64,
    pair: &IntentHashClaimant,
) -> [u8; PROVE_LOG_PAYLOAD_LEN] {
    let mut out = [0u8; PROVE_LOG_PAYLOAD_LEN];
    let bytes = source
        .to_be_bytes()
        .into_iter()
        .chain(destination.to_be_bytes())
        .chain(pair.intent_hash)
        .chain(pair.claimant);
    for (i, byte) in bytes.enumerate() {
        out[2 * i] = HEX[usize::from(byte >> 4)];
        out[2 * i + 1] = HEX[usize::from(byte & 0x0f)];
    }
    out
}

/// The exact line `prove_intent` logs (without the `Program log: ` runtime prefix).
pub fn prove_log_line(program_id: &Pubkey, payload: &[u8; PROVE_LOG_PAYLOAD_LEN]) -> String {
    format!(
        "Prove: program: {}, {}",
        program_id,
        core::str::from_utf8(payload).expect("hex is ascii")
    )
}
```

`instructions/mod.rs`: uncomment `mod prove;` / `pub use prove::*;`. `lib.rs`:

```rust
    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }
```

(add `use eco_svm_std::prover;` at the top of `lib.rs`).

- [ ] **Step 4: Run the unit tests**

```bash
GOLDIE_UPDATE=1 cargo test -p polymer-prover prove::
cargo test -p polymer-prover prove::
```

Expected: 6 pass. Open `prove_log_payload_layout.golden`: 160 chars, starting `0000000000002105` (8453) then `00000000536f7ede` (1399811150, non-mainnet CHAIN_ID) then 64 `1`s then 64 `2`s.

- [ ] **Step 5: Add a `prove` builder and write the integration tests**

Append to `impl PolymerProver<'_>` in `polymer_prover_context.rs` (import `eco_svm_std::prover::{ProofData, ProveArgs}`):

```rust
    /// Direct call to `prove` (bypassing Portal) with an arbitrary signer in the
    /// dispatcher slot; used to pin the dispatcher gate.
    pub fn prove(
        &mut self,
        portal_dispatcher: &Keypair,
        domain_id: u64,
        proof_data: ProofData,
    ) -> TransactionResult {
        let instruction = Instruction {
            program_id: polymer_prover::ID,
            accounts: polymer_prover::accounts::Prove {
                portal_dispatcher: portal_dispatcher.pubkey(),
            }
            .to_account_metas(None),
            data: polymer_prover::instruction::Prove {
                args: ProveArgs {
                    domain_id,
                    proof_data,
                    data: vec![],
                },
            }
            .data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, portal_dispatcher],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
```

Create `integration-tests/tests/prove_polymer_prover.rs`:

```rust
use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use eco_svm_std::CHAIN_ID;
use polymer_prover::instructions::{prove_log_line, prove_log_payload, PolymerProverError};
use portal::state;
use solana_sdk::signature::Keypair;

pub mod common;

const EVM_SOURCE_CHAIN: u64 = 8453;

#[test]
fn prove_via_portal_emits_one_polymer_log_per_intent() {
    let mut ctx = common::Context::default();
    let intents = ctx.fulfill_rand_intents(3, polymer_prover::ID);
    let intent_hashes: Vec<_> = intents.iter().map(|intent| intent.intent_hash).collect();
    let fulfill_markers: Vec<_> = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect();
    let claimants: Vec<_> = fulfill_markers
        .iter()
        .map(|marker| ctx.account::<state::FulfillMarker>(marker).unwrap().claimant)
        .collect();

    let result = ctx
        .portal()
        .prove_intent_via_program(
            polymer_prover::ID,
            intent_hashes.clone(),
            EVM_SOURCE_CHAIN,
            fulfill_markers,
            state::dispatcher_pda(&polymer_prover::ID).0,
            vec![],
            vec![],
        )
        .unwrap();

    for (intent_hash, claimant) in intent_hashes.into_iter().zip(claimants) {
        let payload = prove_log_payload(
            EVM_SOURCE_CHAIN,
            CHAIN_ID,
            &IntentHashClaimant::new(intent_hash, claimant),
        );
        let expected = format!("Program log: {}", prove_log_line(&polymer_prover::ID, &payload));
        assert!(
            result.logs.iter().any(|log| *log == expected),
            "missing log line {expected}\nlogs: {:#?}",
            result.logs
        );
    }
}

#[test]
fn prove_invalid_portal_dispatcher_fail() {
    let mut ctx = common::Context::default();
    let fake_dispatcher = Keypair::new();
    let proof_data = ProofData::new(
        CHAIN_ID,
        vec![IntentHashClaimant::new([1u8; 32].into(), [2u8; 32].into())],
    );

    let result = ctx
        .polymer_prover()
        .prove(&fake_dispatcher, EVM_SOURCE_CHAIN, proof_data);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidPortalDispatcher)));
}
```

- [ ] **Step 6: Build and run**

```bash
anchor build && cargo test --test prove_polymer_prover
```

Expected: both pass. If the first fails, print `result.logs` and compare the exact line; the runtime prefixes program logs with `Program log: `.

- [ ] **Step 7: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add programs/polymer-prover integration-tests
git commit -m "feat(polymer-prover): emit Polymer-provable logs from prove

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 8: `close_proof`, withdraw end to end, confused-deputy arm

**Files:**
- Create: `programs/polymer-prover/src/instructions/close_proof.rs`
- Modify: `programs/polymer-prover/src/instructions/mod.rs`, `programs/polymer-prover/src/lib.rs`
- Modify: `integration-tests/tests/common/polymer_prover_context.rs`
- Create: `integration-tests/tests/close_proof_polymer_prover.rs`
- Modify: `integration-tests/tests/withdraw_confused_deputy.rs`

**Interfaces:**
- Consumes: `portal::state::proof_closer_pda`, `ProofAccount`.
- Produces: instruction `close_proof` with accounts `CloseProof { portal_proof_closer, proof, payer }` (the same order local-prover uses, which is what Portal's `withdraw` and `malicious-proof-closer` build).

- [ ] **Step 1: Write `close_proof.rs`**

```rust
use anchor_lang::prelude::*;

use crate::instructions::PolymerProverError;
use crate::state::ProofAccount;

#[derive(Accounts)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&crate::ID).0 @ PolymerProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut)]
    pub proof: Account<'info, ProofAccount>,
    #[account(mut)]
    pub payer: Signer<'info>,
}

/// Closes the proof to `payer`, the account paying for Portal's `withdraw`.
pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.payer.to_account_info())
}
```

Uncomment `mod close_proof;` / `pub use close_proof::*;` in `mod.rs`; in `lib.rs`:

```rust
    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
```

- [ ] **Step 2: Add the context builder**

Append to `impl PolymerProver<'_>`:

```rust
    pub fn close_proof(
        &mut self,
        portal_proof_closer: &Keypair,
        proof: Pubkey,
    ) -> TransactionResult {
        let instruction = Instruction {
            program_id: polymer_prover::ID,
            accounts: polymer_prover::accounts::CloseProof {
                portal_proof_closer: portal_proof_closer.pubkey(),
                proof,
                payer: self.payer.pubkey(),
            }
            .to_account_metas(None),
            data: polymer_prover::instruction::CloseProof {}.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, portal_proof_closer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
```

- [ ] **Step 3: Write the tests**

Create `integration-tests/tests/close_proof_polymer_prover.rs`:

```rust
use std::iter;

use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CHAIN_ID};
use polymer_prover::instructions::PolymerProverError;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn close_proof_invalid_portal_proof_closer_fail() {
    let mut ctx = common::Context::default();
    let invalid_proof_closer = ctx.payer.insecure_clone();
    let intent_hash = [1u8; 32].into();
    let proof_pda = Proof::pda(&intent_hash, &polymer_prover::ID).0;
    ctx.set_proof(
        proof_pda,
        Proof::new(CHAIN_ID, ctx.payer.pubkey()),
        polymer_prover::ID,
    );

    let result = ctx
        .polymer_prover()
        .close_proof(&invalid_proof_closer, proof_pda);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidPortalProofCloser)));
}

/// Portal `withdraw` → polymer-prover `close_proof`: the proof is gone and its
/// rent came back to the withdraw payer.
#[test]
fn withdraw_closes_polymer_proof_and_refunds_payer() {
    let mut ctx = common::Context::default();
    let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let reward = Reward {
        deadline: ctx.now() + 3600,
        creator: ctx.creator.pubkey(),
        prover: polymer_prover::ID,
        native_amount: 0,
        tokens: vec![],
    };
    let hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&hash).0;
    ctx.airdrop(&vault, 1_000_000_000).unwrap();
    let claimant = Pubkey::new_unique();
    let proof = Proof::pda(&hash, &polymer_prover::ID).0;
    ctx.set_proof(proof, Proof::new(CHAIN_ID, claimant), polymer_prover::ID);
    let proof_rent = ctx.balance(&proof);
    let payer = ctx.payer.pubkey();
    let payer_before = ctx.balance(&payer);

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward,
        vault,
        route_hash,
        claimant,
        proof,
        WithdrawnMarker::pda(&hash).0,
        proof_closer_pda(&polymer_prover::ID).0,
        vec![],
        iter::once(AccountMeta::new(payer, true)),
    );
    assert!(result.is_ok(), "{result:?}");

    assert!(ctx.get_account(&proof).is_none());
    // Payer received the proof rent, net of the fee and the withdrawn-marker rent
    // it paid; the balance must therefore have dropped by less than the proof rent.
    assert!(payer_before - ctx.balance(&payer) < proof_rent);
}
```

Append to `integration-tests/tests/withdraw_confused_deputy.rs` (the malicious closer's account order is `[proof_closer, proof, payer]`, identical to polymer-prover's, so it targets the new prover unchanged):

```rust
/// Same property against polymer-prover: its `close_proof` accepts only
/// `proof_closer_pda(&polymer_prover::ID)`.
#[test]
fn malicious_proof_closer_cannot_close_polymer_proof_via_proof_closer() {
    let mut ctx = common::Context::default();

    let victim_intent_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let victim_proof = Proof::pda(&victim_intent_hash, &polymer_prover::ID).0;
    ctx.set_proof(
        victim_proof,
        Proof::new(CHAIN_ID, Pubkey::new_unique()),
        polymer_prover::ID,
    );

    let attacker = ctx.payer.pubkey();
    let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let reward = Reward {
        deadline: ctx.now() + 3600,
        creator: attacker,
        prover: malicious_proof_closer::ID,
        native_amount: 0,
        tokens: vec![],
    };
    let attacker_intent_hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&attacker_intent_hash).0;
    ctx.airdrop(&vault, 1_000_000_000).unwrap();
    let attacker_proof = Proof::pda(&attacker_intent_hash, &malicious_proof_closer::ID).0;
    ctx.set_proof(
        attacker_proof,
        Proof::new(CHAIN_ID, attacker),
        malicious_proof_closer::ID,
    );

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward,
        vault,
        route_hash,
        attacker,
        attacker_proof,
        WithdrawnMarker::pda(&attacker_intent_hash).0,
        proof_closer_pda(&malicious_proof_closer::ID).0,
        vec![],
        vec![
            AccountMeta::new_readonly(polymer_prover::ID, false),
            AccountMeta::new(victim_proof, false),
            AccountMeta::new(attacker, true),
        ],
    );

    assert!(result
        .clone()
        .is_err_and(common::reached_program(polymer_prover::ID)));
    assert!(result.is_err_and(common::is_program_error(
        polymer_prover::ID,
        polymer_prover::instructions::PolymerProverError::InvalidPortalProofCloser
    )));
    assert!(ctx.get_account(&victim_proof).is_some());
}
```

- [ ] **Step 4: Build and run**

```bash
anchor build && cargo test --test close_proof_polymer_prover --test withdraw_confused_deputy
```

Expected: all pass.

- [ ] **Step 5: Run the whole suite once**

```bash
cargo test --no-fail-fast
```

Expected: green. Existing tests must be unaffected by the two new programs in `Context`.

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add programs/polymer-prover integration-tests
git commit -m "feat(polymer-prover): close_proof scoped to Portal's per-prover closer

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 9: Ignored smoke test against Polymer's real program

**Files:**
- Create: `integration-tests/tests/fixtures/polymer/op-proof-v2.hex`
- Create: `integration-tests/tests/fixtures/polymer/README.md`
- Create: `integration-tests/tests/validate_polymer_prover_real.rs`

**Interfaces:**
- Consumes: `polymer::{CREATE_ACCOUNTS_DISCRIMINATOR, LOAD_PROOF_DISCRIMINATOR, INTERNAL_ACCOUNT_DISCRIMINATOR, cache_pda, result_pda, internal_pda, POLYMER_PROVER_ID}`, the `validate` builder.

- [ ] **Step 1: Vendor the fixture**

```bash
mkdir -p integration-tests/tests/fixtures/polymer
curl -sL https://raw.githubusercontent.com/polymerdao/solana-prover-contracts/main/programs/polymer-prover/src/instructions/test-data/op-proof-v2.hex \
  -o integration-tests/tests/fixtures/polymer/op-proof-v2.hex
wc -c integration-tests/tests/fixtures/polymer/op-proof-v2.hex   # ~2184 bytes of hex text
```

`integration-tests/tests/fixtures/polymer/README.md`:

```markdown
# Polymer fixtures

`op-proof-v2.hex` is Polymer's own test proof from
polymerdao/solana-prover-contracts (`programs/polymer-prover/src/instructions/test-data/`),
licensed Apache-2.0. It proves an OP Sepolia event (chain 11155420) emitted by
`0xf221750e52aa080835d2957f2eed0d5d7ddd8c38` with four topics, signed by Polymer's
test sequencer `0x8D3921B96A3815F403Fb3a4c7fF525969d16f9E0` for client type `proof_api`
on peptide chain 901.

Used only by the `#[ignore]` test `validate_polymer_prover_real.rs`, which needs the real
program binary:

    solana program dump FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3 /tmp/polymer_prover.so --url devnet
    POLYMER_PROVER_SO=/tmp/polymer_prover.so cargo test --test validate_polymer_prover_real -- --ignored
```

- [ ] **Step 2: Write the test**

`integration-tests/tests/validate_polymer_prover_real.rs`:

```rust
//! Wiring check against Polymer's real program: loads their published fixture
//! proof through the real `create_accounts` / `load_proof`, then runs our
//! `validate`, which must get past Polymer's verification and fail on our own
//! topic-count check (the fixture event has four topics, ours has two).
//!
//! Needs `POLYMER_PROVER_SO=<path to dumped .so>`; run with `-- --ignored`.

use anchor_lang::prelude::AccountMeta;
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
const FIXTURE_PEPTIDE_CHAIN_ID: u64 = 901;
const FIXTURE_CLIENT_TYPE: &str = "proof_api";

fn fixture_proof() -> Vec<u8> {
    let hex = include_str!("fixtures/polymer/op-proof-v2.hex");
    let hex = hex.trim().trim_start_matches("0x");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
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

#[test]
#[ignore = "needs POLYMER_PROVER_SO pointing at a dumped polymer_prover.so"]
fn real_polymer_program_validates_fixture_proof_and_our_checks_run() {
    let so_path = std::env::var("POLYMER_PROVER_SO").expect("POLYMER_PROVER_SO not set");
    let program = std::fs::read(&so_path).expect("read POLYMER_PROVER_SO");

    let mut ctx = common::Context::default();
    // Replace the mock with the real binary at the same ID.
    ctx.add_program(polymer::POLYMER_PROVER_ID, &program).unwrap();

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
    ctx.set_account(
        polymer::internal_pda().0,
        Account {
            lamports: ctx.get_sysvar::<Rent>().minimum_balance(internal_data.len()),
            data: internal_data,
            owner: polymer::POLYMER_PROVER_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    ctx.polymer_prover()
        .init(vec![evm_address_to_bytes32(FIXTURE_EMITTER)], Config::pda().0)
        .unwrap();

    let authority = Keypair::new();
    ctx.airdrop(&authority.pubkey(), common::sol_amount(5.0)).unwrap();

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
    assert!(
        result.clone().is_err_and(common::is_error(PolymerProverError::InvalidTopicsLength)),
        "{result:?}"
    );
}
```

- [ ] **Step 3: Run it for real**

```bash
solana program dump FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3 /tmp/polymer_prover.so --url devnet
anchor build
POLYMER_PROVER_SO=/tmp/polymer_prover.so cargo test --test validate_polymer_prover_real -- --ignored
```

Expected: pass. If it fails with `PolymerProofInvalid`, print the logs: Polymer's `msg!("{}", result)` line names the failing stage (signature, membership, state root). A signer or peptide mismatch means the internal-account seed is wrong; re-check the fixture README values against Polymer's `tests/test.ts`.

Also confirm the normal run skips it:

```bash
cargo test --test validate_polymer_prover_real
```

Expected: `1 ignored`.

- [ ] **Step 4: Commit**

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add integration-tests
git commit -m "test(polymer-prover): ignored wiring test against Polymer's real program

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 10: Documentation

**Files:**
- Modify: `CLAUDE.md` (Architecture: programs list, conventions, integration tests)
- Modify: `README.md` (program list, test list, `mainnet` feature list)
- Modify: `docs/superpowers/specs/2026-09-22-polymer-prover-design.md` (mock location)

- [ ] **Step 1: CLAUDE.md**

In "Architecture", change "Five production on-chain programs" to "Six" and add the mock to the localnet-only list. Add a bullet under "Programs (`programs/*`)" after hyper-prover:

```markdown
- **polymer-prover** — Polymer-backed prover, pull-based unlike hyper-prover: nobody calls us, a relayer loads a Polymer proof into Polymer's program (`create_accounts` → `load_proof`) and then calls our permissionless `validate`, which CPIs Polymer's `validate_event` and reads the `["result", authority]` account it writes **in the same instruction** (freshness), then mirrors the Solidity `PolymerProver.validate` checks (whitelisted emitter, `IntentFulfilledFromSource` selector, topic 1 == `CHAIN_ID`, payload destination == Polymer's `chain_id`) and creates `Proof` PDAs idempotently (same proof no-op, disagreeing proof `IntentAlreadyProven`). `prove` is gated to `dispatcher_pda(&polymer_prover::ID)` and emits one `Prove: program: <id>, <hex>` log per intent (hex = source u64 ‖ destination u64 ‖ intent hash ‖ claimant) for the EVM `PolymerProver.validateSolana` to parse — that string is a shared ABI. `polymer.rs` hand-mirrors Polymer's IDs (mainnet `CdvSq48Q…`, devnet `FtdxWoZX…`, feature-gated), PDAs, discriminators and result layout the way `hyperlane.rs` mirrors the mailbox; Polymer's public docs describe an older interface, their repo source (v1.0.4) is authoritative. Callers set a 1.4M CU limit on `validate`.
- **mock-polymer-prover** — localnet-only stand-in at Polymer's devnet ID with identical account layouts; its `validate_event` decodes the loaded cache as a `ValidationResultAccount` so tests inject any EVM event.
```

Add `polymer-prover` to the atomic-release sentence in "Cross-cutting conventions" and `polymer_prover_context.rs` to the per-program contexts list. In the "mock-igp" paragraph about localnet-only programs, add `mock-polymer-prover` to the list of Anchor programs kept out by the explicit enumeration.

- [ ] **Step 2: README.md**

Add a `#### **Polymer-Prover Program** (`programs/polymer-prover/`)` entry beside hyper-prover's, list the four new test files in the test index, and add `polymer-prover` to the `mainnet` feature sentence and the Anchor.toml example.

- [ ] **Step 3: Spec amendment**

In the spec, section 6 "Integration (litesvm)", change `integration-tests/programs/mock-polymer-prover` to `programs/mock-polymer-prover` and add: "It lives beside `dummy-ism` because `anchor build` compiles `programs/*`; `integration-tests/programs/` is only for non-Anchor programs built with `cargo build-sbf`."

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md docs/superpowers/specs/2026-09-22-polymer-prover-design.md
git commit -m "docs: document polymer-prover and its localnet mock

Claude-Session: https://claude.ai/code/session_01NHPxMueoj9Tu91K7asAKq8"
```

---

### Task 11: Devnet verification of Polymer log attribution (manual)

This task cannot be test-driven; it answers the open risk in spec section 6 and gates the eco-routes plan's log parser on a real proof.

- [ ] **Step 1: Deploy polymer-prover to devnet**

```bash
anchor run build-devnet
anchor deploy --provider.cluster devnet --program-name polymer-prover --program-keypair keys/polymer_prover-keypair.json
```

Portal on devnet needs no change: `dispatcher_pda(args.prover)` is derived per caller-chosen prover.

- [ ] **Step 2: Initialize**

Run `init` with the devnet EVM `PolymerProver` address(es) left-padded to 32 bytes, using the Solana CLI keypair as payer (see `solana config get`). Use the IDL in `target/idl/polymer_prover.json` with `anchor` TS or a small Rust bin; do not put a private key in any script.

- [ ] **Step 3: Produce a `Prove:` log through Portal**

Fulfill a devnet intent whose `reward.prover` is the new program (routes-cli), then call Portal `prove` with `prover = polymer_prover::ID`, `source_chain_domain_id = <EVM testnet chain id>`, the fulfill marker, and no tail accounts. Confirm the transaction logs show `Program log: Prove: program: <id>, <160 hex>` nested under the polymer-prover invoke.

- [ ] **Step 4: Request a proof from Polymer**

```bash
curl -s -X POST https://proof.testnet.polymer.zone -H "Authorization: Bearer $POLYMER_API_KEY" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"polymer_requestProof","params":[{"srcChainId":2,"txSignature":"<sig>","programID":"<polymer_prover id>"}]}'
# then poll
curl -s -X POST https://proof.testnet.polymer.zone -H "Authorization: Bearer $POLYMER_API_KEY" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"polymer_queryProof","params":["<jobId>"]}'
```

Record the exact `srcChainId` Polymer expects for Solana devnet and mainnet (the spec assumes 2); that value goes into the eco-routes constructor.

- [ ] **Step 5: Decide**

- Proof returned with our log in it: attribution through Portal's CPI works. Note the result in the spec's "Devnet" section and hand the proof bytes to the eco-routes plan as a Foundry fixture.
- Proof request rejected or returns no logs: implement the fallback from spec section 6 (permissionless top-level `prove_from_markers` that reads Portal-owned `FulfillMarker` PDAs) as a new task in this plan before continuing.

---

## Self-review against the spec

- 3.1 layout, features, IDs: Task 1, 2. Mock location deviates from spec (Task 10 amends).
- 3.2 state: Task 1. 3.3 init: Task 1, tests Task 5.
- 3.4 validate, every check and the idempotency rule: Task 5 (implementation, happy path, idempotency), Task 6 (each rejection incl. wrong Polymer program and foreign result account).
- 3.5 prove, cap, log format: Task 7 (unit goldens for payload and line, Portal-driven integration test, dispatcher gate).
- 3.6 close_proof: Task 8 (direct gate test, withdraw end to end, confused-deputy arm).
- 4 security: freshness by construction in Task 5; owner/discriminator in Task 2; prover-scoped authorities pinned by goldens in Task 1.
- 6 tests: unit (Tasks 1–3, 7), integration (4–8), real-program smoke (9), devnet (11). EVM tests belong to the eco-routes plan.
- 7 rollout: build/deploy/release wiring in Task 1 and 4; CLAUDE.md in Task 10; devnet init in Task 11; mainnet `init` and the eco-routes redeploy are release steps outside this plan.

Type consistency: `Config::whitelisted_emitters`, `PolymerProver::{init, validate, prove, close_proof, polymer_create_accounts, polymer_load_result}`, `intent_fulfilled_result(emitter, source, chain_id, encoded_proofs)`, `prove_log_payload(source, destination, pair)`, `prove_log_line(program_id, payload)`, `ValidationResult::{try_from_account_info, from_body}` are used with the same names and argument orders in every task.
