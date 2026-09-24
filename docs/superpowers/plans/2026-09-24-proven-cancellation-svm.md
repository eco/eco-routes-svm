# Proven Cancellation (SVM) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let anyone cancel an unfulfilled intent on the SVM destination after `route.deadline`, carry that
cancellation to the source as the `CANCELLED` sentinel claimant, and let the SVM source Portal refund
immediately on a proven cancellation while `withdraw` refuses it.

**Architecture:** The destination writes `CANCELLED` into the existing `FulfillMarker` PDA (new `cancel`
instruction), so `fulfill`'s "already fulfilled" check and `prove`'s payload build cover cancellation unchanged.
`close_fulfill_marker` shrinks the marker to a 40-byte `FulfillTombstone` instead of deleting it. On the source,
provers and the `Proof` layout are untouched: a proof whose claimant is `CANCELLED` is a proven cancellation, which
`refund` accepts before `reward.deadline` (and then CPIs the prover's `close_proof`), and `withdraw` rejects.

**Tech Stack:** Anchor 1.1.2, Rust 1.97.1 host / platform-tools rustc 1.89.0-dev on-chain, litesvm 0.16
integration tests, goldie 0.7 snapshots.

**Spec:** `eco-routes` repo, `CLAUDE/specs/2026-09-24-proven-cancellation-design.md` (worktree
`/Users/carlosfebres/dev/eco/eco-routes-proven-cancellation/CLAUDE/specs/2026-09-24-proven-cancellation-design.md`).
This plan implements §5, §7, §9 and the SVM column of §10, under decisions D1–D8. Pointer:
`docs/superpowers/specs/2026-09-24-proven-cancellation-design.md` in this repo.

## Global Constraints

- `CANCELLED = keccak256("eco.portal.intent.cancelled")` =
  `0xa8aa898126679f5f179cb3a4e685056aec77686a83e2a6bdf37c6f71dd2fdb5f`, byte-identical to EVM `Inbox.CANCELLED` (D2).
- Wire format unchanged: `[destination u64 BE] ‖ N × [intent_hash 32 B ‖ claimant 32 B]` (D1). Do not touch
  `eco_svm_std::prover::ProofData` or `Proof`.
- SVM `Proof` layout unchanged; a proven cancellation is `Proof { destination, claimant: CANCELLED }` (D5).
  hyper-prover, local-prover and flash-fulfiller get **no code changes**.
- Conflicts: first recorded wins — keep hyper-prover's existing "disagreeing redelivery reverts" behaviour (D7).
- Minor release (D8): commits are `feat(...)`, `test(...)`, `fix(...)`, `docs(...)`, `refactor(...)` —
  **never** a `!` suffix and **never** a `BREAKING CHANGE` footer.
- Every commit message ends with the line `Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP`.
  No co-author lines. Stage files by name (never `git add -A` / `git add .`).
- `PortalError` codes are positional from 6000 — **append only**, never insert or reorder.
- On-chain code must compile under platform-tools rustc 1.89.0-dev: no stdlib APIs newer than that (e.g. no
  `is_multiple_of`).
- Programs are redeployed under new IDs, never upgraded: **no layout migration or old-layout fallback code**.
- Integration tests embed `target/deploy/*.so` via `include_bytes!`: run `anchor build` before any
  `cargo test` of `integration-tests`, or the binaries are stale.
- Closed accounts are asserted with `get_account(&x).is_none()`, never by inspecting a husk.
- Lint gates: `cargo clippy --all-targets -- -D warnings` (never `--all-features`), `cargo +nightly fmt`,
  `cargo sort --workspace --check`.
- Goldie: new snapshots via `GOLDIE_UPDATE=1`, and review the generated `.golden` before committing it.
- `CLAUDE.md` security rule: this is a feature, not a fix for deployed code, so the normal PR flow applies —
  but do **not** push or open a PR as part of this plan; the human decides.

### Command conventions

All commands use absolute paths. Set once per shell:

```bash
WS=/Users/carlosfebres/dev/eco/eco-routes-svm-proven-cancellation
```

- Host cargo: `cargo +1.97.1 <cmd> --manifest-path "$WS/Cargo.toml" ...` (explicit toolchain, because
  `rust-toolchain.toml` is only picked up from inside the repo).
- Anchor has no manifest flag; run it in a subshell: `(cd "$WS" && anchor build)`.
- Git: `git -C "$WS" ...`.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `packages/eco-svm-std/src/lib.rs` | modify | `CANCELLED` constant + pin tests |
| `packages/eco-svm-std/Cargo.toml` | modify | `tiny-keccak` dev-dependency for the pin test |
| `packages/eco-svm-std/src/testdata/cancelled_deterministic.golden` | create | snapshot of `CANCELLED` |
| `programs/portal/src/instructions/mod.rs` | modify | new errors (appended), `cancel` + `close_proof` modules |
| `programs/portal/src/instructions/cancel.rs` | create | `cancel` instruction |
| `programs/portal/src/instructions/close_proof.rs` | create | shared prover `close_proof` CPI (moved out of `withdraw.rs`) |
| `programs/portal/src/instructions/fulfill.rs` | modify | reject `claimant == CANCELLED` |
| `programs/portal/src/instructions/close_fulfill_marker.rs` | modify | rewrite marker → tombstone |
| `programs/portal/src/instructions/prove.rs` | modify | read claimant from marker **or** tombstone |
| `programs/portal/src/instructions/withdraw.rs` | modify | reject cancelled proofs; use shared `close_proof` |
| `programs/portal/src/instructions/refund.rs` | modify | cancellation fast path + `close_proof` tail |
| `programs/portal/src/state.rs` | modify | `FulfillTombstone`, `fulfillment_claimant` |
| `programs/portal/src/state/testdata/fulfill_tombstone_layout_deterministic.golden` | create | tombstone layout snapshot |
| `programs/portal/src/events.rs` | modify | `IntentCancelled`; `FulfillMarkerClosed` doc |
| `programs/portal/src/lib.rs` | modify | `cancel` entrypoint |
| `integration-tests/tests/common/portal_context.rs` | modify | `cancel_intent`, `refund_intent_with_close_proof` builders |
| `integration-tests/tests/cancel.rs` | create | `cancel` instruction tests |
| `integration-tests/tests/proven_cancellation.rs` | create | cancel → prove → refund / withdraw flows via local-prover |
| `integration-tests/tests/fulfill.rs`, `close_fulfill_marker.rs`, `withdraw.rs`, `refund.rs`, `handle.rs` | modify | per-feature tests |
| `CLAUDE.md`, `README.md` | modify | document `cancel`, tombstone, fast refund |

---

### Task 1: `CANCELLED` sentinel in `eco-svm-std`

**Files:**
- Modify: `packages/eco-svm-std/src/lib.rs`
- Modify: `packages/eco-svm-std/Cargo.toml`
- Create: `packages/eco-svm-std/src/testdata/cancelled_deterministic.golden` (generated)

**Interfaces:**
- Produces: `pub const eco_svm_std::CANCELLED: Bytes32`. Compare with `claimant == CANCELLED` (`Bytes32`) or
  `CANCELLED == pubkey` (`Bytes32: PartialEq<Pubkey>`, already implemented).

- [ ] **Step 1: Add the dev-dependency**

In `packages/eco-svm-std/Cargo.toml`, extend `[dev-dependencies]` (keep it sorted):

```toml
[dev-dependencies]
goldie = { workspace = true }
tiny-keccak = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` in `packages/eco-svm-std/src/lib.rs`:

```rust
    #[test]
    fn cancelled_is_keccak_of_its_domain_tag() {
        use tiny_keccak::{Hasher, Keccak};

        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];
        hasher.update(b"eco.portal.intent.cancelled");
        hasher.finalize(&mut hash);

        assert_eq!(<[u8; 32]>::from(CANCELLED), hash);
    }

    /// The same 32 bytes are `Inbox.CANCELLED` on EVM; a drift here makes one
    /// VM's cancellation look like a real claimant to the other.
    #[test]
    fn cancelled_matches_the_evm_constant() {
        let hex: String = CANCELLED.iter().map(|byte| format!("{byte:02x}")).collect();

        assert_eq!(
            hex,
            "a8aa898126679f5f179cb3a4e685056aec77686a83e2a6bdf37c6f71dd2fdb5f"
        );
    }

    /// Upper 12 bytes non-zero: EVM provers can never mistake it for an address.
    #[test]
    fn cancelled_is_not_an_evm_address() {
        assert!(CANCELLED[..12].iter().any(|byte| *byte != 0));
    }

    #[test]
    fn cancelled_deterministic() {
        goldie::assert_debug!(CANCELLED);
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p eco-svm-std cancelled`
Expected: compile error `cannot find value CANCELLED in this scope`.

- [ ] **Step 4: Add the constant**

In `packages/eco-svm-std/src/lib.rs`, directly after the `Bytes32` struct definition:

```rust
/// Claimant sentinel written by the destination portal's `cancel` instruction.
///
/// `keccak256("eco.portal.intent.cancelled")`, byte-identical to EVM
/// `Inbox.CANCELLED`, so a cancellation travels as an ordinary
/// `(intent_hash, claimant)` pair on every prover. Nobody holds a key for it
/// (it is a hash output), and its upper 12 bytes are non-zero, so EVM provers
/// never read it as an address. On the source, a `Proof` whose claimant is
/// `CANCELLED` is a proven cancellation: `refund` accepts it before
/// `reward.deadline`, and `withdraw` rejects it.
pub const CANCELLED: Bytes32 = Bytes32([
    0xa8, 0xaa, 0x89, 0x81, 0x26, 0x67, 0x9f, 0x5f, 0x17, 0x9c, 0xb3, 0xa4, 0xe6, 0x85, 0x05, 0x6a,
    0xec, 0x77, 0x68, 0x6a, 0x83, 0xe2, 0xa6, 0xbd, 0xf3, 0x7c, 0x6f, 0x71, 0xdd, 0x2f, 0xdb, 0x5f,
]);
```

- [ ] **Step 5: Generate the snapshot and run the tests**

Run: `GOLDIE_UPDATE=1 cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p eco-svm-std cancelled`
then `cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p eco-svm-std`
Expected: all pass; `packages/eco-svm-std/src/testdata/cancelled_deterministic.golden` contains
`Bytes32([168, 170, 137, 129, 38, 103, 159, 95, 23, 156, 179, 164, 230, 133, 5, 106, 236, 119, 104, 106, 131, 226, 166, 189, 243, 124, 111, 113, 221, 47, 219, 95])`.

- [ ] **Step 6: Sort, lint, commit**

```bash
(cd "$WS" && cargo +1.97.1 sort --workspace --check)
git -C "$WS" add packages/eco-svm-std/Cargo.toml packages/eco-svm-std/src/lib.rs \
  packages/eco-svm-std/src/testdata/cancelled_deterministic.golden Cargo.lock
git -C "$WS" commit -m "feat(eco-svm-std): add the CANCELLED claimant sentinel

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```
(Stage `Cargo.lock` only if it changed; check `git -C "$WS" status --short` first.)

---

### Task 2: New portal errors and `fulfill` rejects the sentinel

**Files:**
- Modify: `programs/portal/src/instructions/mod.rs:56-64`
- Modify: `programs/portal/src/instructions/fulfill.rs:13,70-71`
- Test: `integration-tests/tests/fulfill.rs`

**Interfaces:**
- Consumes: `eco_svm_std::CANCELLED` (Task 1).
- Produces: `PortalError::ReservedClaimant`, `PortalError::IntentCancelled` (appended in this order; used by
  Tasks 3, 5).

- [ ] **Step 1: Write the failing test**

Append to `integration-tests/tests/fulfill.rs` and add `use eco_svm_std::CANCELLED;` to its imports:

```rust
/// Only `cancel` may write the sentinel: a fulfill carrying it would record a
/// fill that later proves as a cancellation.
#[test]
fn fulfill_reserved_claimant_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, reward) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    let reward_hash = reward.hash();
    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        CANCELLED,
        state::executor_pda().0,
        fulfill_marker,
        vec![],
        vec![],
    );

    assert!(result.is_err_and(common::is_error(PortalError::ReservedClaimant)));
    assert!(ctx.get_account(&fulfill_marker).is_none());
}
```

- [ ] **Step 2: Build and run to verify it fails**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test fulfill fulfill_reserved_claimant_fail`
Expected: compile error `no variant named ReservedClaimant`.

- [ ] **Step 3: Append the errors**

In `programs/portal/src/instructions/mod.rs`, after `ExecutorAtaCorrupted,` (the last variant):

```rust
    /// `claimant` is the reserved `CANCELLED` sentinel, which only `cancel` may write.
    ReservedClaimant,
    /// The intent's proof records a cancellation, which never pays a claimant.
    IntentCancelled,
```

- [ ] **Step 4: Reject the sentinel in `fulfill`**

In `programs/portal/src/instructions/fulfill.rs` change the import to
`use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};` and add, right after the `RouteExpired` check (line 71):

```rust
    require!(claimant != CANCELLED, PortalError::ReservedClaimant);
```

- [ ] **Step 5: Build and run the fulfill tests**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test fulfill`
Expected: all pass, including `fulfill_reserved_claimant_fail`.

- [ ] **Step 6: Commit**

```bash
git -C "$WS" add programs/portal/src/instructions/mod.rs programs/portal/src/instructions/fulfill.rs \
  integration-tests/tests/fulfill.rs
git -C "$WS" commit -m "feat(portal): reserve the CANCELLED claimant for cancel

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

### Task 3: `cancel` instruction

**Files:**
- Create: `programs/portal/src/instructions/cancel.rs`
- Modify: `programs/portal/src/instructions/mod.rs:3-18`
- Modify: `programs/portal/src/events.rs`
- Modify: `programs/portal/src/lib.rs:41-46`
- Modify: `integration-tests/tests/common/portal_context.rs`
- Test: `integration-tests/tests/cancel.rs` (create)

**Interfaces:**
- Consumes: `CANCELLED` (Task 1); `PortalError::{InvalidPortal, RouteNotExpired, InvalidIntentHash,
  InvalidFulfillMarker, IntentAlreadyFulfilled}` (existing).
- Produces:
  - `portal::instructions::CancelArgs { intent_hash: Bytes32, route: Route, reward_hash: Bytes32 }`
  - accounts `portal::accounts::Cancel { payer, fulfill_marker, system_program }`
  - instruction `portal::instruction::Cancel { args }`
  - event `portal::events::IntentCancelled::new(intent_hash: Bytes32)`
  - test builder `Portal::cancel_intent(&mut self, intent_hash: Bytes32, route: &Route, reward_hash: Bytes32,
    fulfill_marker: Pubkey) -> TransactionResult`

Note for the implementer: `cancel` takes the **canonical route** — the one whose `hash()` the source
committed to (as emitted in `IntentPublished`). Unlike `fulfill`, it executes no calls, so it does not
rewrite `call.data` into `CalldataWithAccounts`; it hashes the route exactly as passed.

- [ ] **Step 1: Add the test builder**

In `integration-tests/tests/common/portal_context.rs`, add inside `impl Portal<'_>`:

```rust
    pub fn cancel_intent(
        &mut self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        fulfill_marker: Pubkey,
    ) -> TransactionResult {
        let args = portal::instructions::CancelArgs {
            intent_hash,
            route: route.clone(),
            reward_hash,
        };
        let instruction = Instruction {
            program_id: portal::ID,
            accounts: portal::accounts::Cancel {
                payer: self.payer.pubkey(),
                fulfill_marker,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: portal::instruction::Cancel { args }.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
```

- [ ] **Step 2: Write the failing tests**

Create `integration-tests/tests/cancel.rs`:

```rust
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};
use local_prover::state::ProofAccount;
use portal::events::{IntentCancelled, IntentProven};
use portal::instructions::PortalError;
use portal::state::{self, FulfillMarker};
use portal::types::{self, Route};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

/// A minimal intent (no route tokens, calls, or native amount) destined for
/// this chain and proven by local-prover, not yet fulfilled.
fn open_intent(ctx: &mut common::Context) -> (Bytes32, Route, Bytes32) {
    let (_, mut route, mut reward) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    reward.prover = local_prover::ID;
    let reward_hash = reward.hash();
    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);

    (intent_hash, route, reward_hash)
}

#[test]
fn cancel_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_ok_and(common::contains_event(IntentCancelled::new(intent_hash))));
    let marker = ctx.account::<FulfillMarker>(&fulfill_marker).unwrap();
    assert_eq!(marker.claimant, CANCELLED);
    assert_eq!(marker.payer, ctx.payer.pubkey());
    assert_eq!(marker.deadline, route.deadline);
}

/// `fulfill` still accepts `now == route.deadline`, so `cancel` must not.
#[test]
fn cancel_at_route_deadline_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::RouteNotExpired)));
    assert!(ctx.get_account(&fulfill_marker).is_none());
}

#[test]
fn cancel_invalid_portal_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, reward_hash) = open_intent(&mut ctx);
    route.portal = random::<[u8; 32]>().into();
    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidPortal)));
}

#[test]
fn cancel_invalid_intent_hash_fail() {
    let mut ctx = common::Context::default();
    let (_, route, reward_hash) = open_intent(&mut ctx);
    let wrong_intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let fulfill_marker = FulfillMarker::pda(&wrong_intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(wrong_intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidIntentHash)));
}

#[test]
fn cancel_invalid_fulfill_marker_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result =
        ctx.portal()
            .cancel_intent(intent_hash, &route, reward_hash, Pubkey::new_unique());

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarker)));
}

#[test]
fn cancel_after_fulfill_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);

    let result = ctx.portal().cancel_intent(
        intent.intent_hash,
        &intent.route,
        intent.reward_hash,
        fulfill_marker,
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentAlreadyFulfilled)));
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker).unwrap().claimant,
        claimant
    );
}

#[test]
fn cancel_twice_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::IntentAlreadyFulfilled)));
}

/// Mutual exclusion from the other side: the windows are disjoint, so a
/// cancelled intent is already past the point where `fulfill` would accept it.
#[test]
fn fulfill_after_cancel_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        Pubkey::new_unique().to_bytes().into(),
        state::executor_pda().0,
        fulfill_marker,
        vec![],
        vec![],
    );

    assert!(result.is_err_and(common::is_error(PortalError::RouteExpired)));
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker).unwrap().claimant,
        CANCELLED
    );
}

/// `prove` needs no change: the sentinel rides the existing payload, and the
/// prover records it verbatim.
#[test]
fn prove_cancelled_intent_via_local_prover_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        CHAIN_ID,
        vec![fulfill_marker],
        state::dispatcher_pda(&local_prover::ID).0,
        vec![proof],
    );

    assert!(result.is_ok_and(common::contains_event(IntentProven::new(
        intent_hash,
        CANCELLED
    ))));
    let proof = ctx.account::<ProofAccount>(&proof).unwrap();
    assert!(CANCELLED == proof.0.claimant);
    assert_eq!(proof.0.destination, CHAIN_ID);
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test cancel`
Expected: compile errors (`CancelArgs`, `accounts::Cancel`, `IntentCancelled` not found).

- [ ] **Step 4: Add the event**

Append to `programs/portal/src/events.rs`:

```rust
#[event]
#[derive(new)]
pub struct IntentCancelled {
    intent_hash: Bytes32,
}
```

- [ ] **Step 5: Implement the instruction**

Create `programs/portal/src/instructions/cancel.rs`:

```rust
use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};

use crate::events::IntentCancelled;
use crate::instructions::{now, PortalError};
use crate::state::{FulfillMarker, FULFILL_MARKER_SEED};
use crate::types::{self, Route};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CancelArgs {
    pub intent_hash: Bytes32,
    pub route: Route,
    pub reward_hash: Bytes32,
}

/// Permanently closes an unfulfilled intent on its destination.
///
/// Writes the `CANCELLED` sentinel into the intent's `FulfillMarker` PDA, the
/// same record `fulfill` writes, so the two are mutually exclusive by
/// construction: whichever lands first owns the PDA and the other fails to
/// create it. Time separates them too — `fulfill` needs
/// `route.deadline >= now`, `cancel` needs `route.deadline < now` — so they
/// never race.
///
/// Permissionless: the caller chooses only *when* after the deadline, never
/// the outcome. `prove` then carries the sentinel to the source unchanged,
/// where it enables `refund` before `reward.deadline`.
///
/// `route` is the canonical route the source committed to; it is hashed as
/// passed (no calls are executed).
#[derive(Accounts)]
#[instruction(args: CancelArgs)]
pub struct Cancel<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub fulfill_marker: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn cancel_intent(ctx: Context<Cancel>, args: CancelArgs) -> Result<()> {
    let CancelArgs {
        intent_hash: expected_intent_hash,
        route,
        reward_hash,
    } = args;

    require!(route.portal == crate::ID, PortalError::InvalidPortal);
    require!(route.deadline < now()?, PortalError::RouteNotExpired);

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    require!(
        intent_hash == expected_intent_hash,
        PortalError::InvalidIntentHash
    );

    let (fulfill_marker, bump) = FulfillMarker::pda(&intent_hash);
    require!(
        ctx.accounts.fulfill_marker.key() == fulfill_marker,
        PortalError::InvalidFulfillMarker
    );
    let signer_seeds = [FULFILL_MARKER_SEED, intent_hash.as_ref(), &[bump]];

    FulfillMarker::new(CANCELLED, ctx.accounts.payer.key(), route.deadline, bump)
        .init(
            &ctx.accounts.fulfill_marker,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &[&signer_seeds],
        )
        .map_err(|_| Error::from(PortalError::IntentAlreadyFulfilled))?;

    emit!(IntentCancelled::new(intent_hash));

    Ok(())
}
```

- [ ] **Step 6: Wire the module and entrypoint**

In `programs/portal/src/instructions/mod.rs` add `mod cancel;` before `mod close_fulfill_marker;` and
`pub use cancel::*;` before `pub use close_fulfill_marker::*;`.

In `programs/portal/src/lib.rs`, inside `pub mod portal`, after `close_fulfill_marker`:

```rust
    pub fn cancel(ctx: Context<Cancel>, args: CancelArgs) -> Result<()> {
        cancel_intent(ctx, args)
    }
```

- [ ] **Step 7: Build and run the tests**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test cancel`
Expected: 9 passed.

- [ ] **Step 8: Commit**

```bash
git -C "$WS" add programs/portal/src/instructions/cancel.rs programs/portal/src/instructions/mod.rs \
  programs/portal/src/events.rs programs/portal/src/lib.rs \
  integration-tests/tests/common/portal_context.rs integration-tests/tests/cancel.rs
git -C "$WS" commit -m "feat(portal): add a permissionless cancel instruction after route.deadline

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

### Task 4: `close_fulfill_marker` leaves a tombstone; `prove` reads it

**Files:**
- Modify: `programs/portal/src/state.rs`
- Create: `programs/portal/src/state/testdata/fulfill_tombstone_layout_deterministic.golden` (generated)
- Modify: `programs/portal/src/instructions/close_fulfill_marker.rs` (whole file)
- Modify: `programs/portal/src/instructions/prove.rs:12,63-93,98-107`
- Modify: `programs/portal/src/events.rs:52-54` (doc comment)
- Test: `integration-tests/tests/close_fulfill_marker.rs`

**Interfaces:**
- Consumes: `cancel_intent` builder (Task 3), `CANCELLED` (Task 1).
- Produces:
  - `portal::state::FulfillTombstone { pub claimant: Bytes32 }` (`#[account]`, 8 + 32 = 40 bytes)
  - `portal::state::fulfillment_claimant(account: &AccountInfo) -> Result<Bytes32>` — claimant of a
    `FulfillMarker` or `FulfillTombstone`, else `PortalError::InvalidFulfillMarker`.
  - `FulfillMarkerClosed.lamports` now means "lamports returned to payer" (marker rent − tombstone rent + any
    surplus), no longer the full marker balance.

Why `UncheckedAccount`: Anchor's `Account<'info, FulfillMarker>` re-serializes a `mut` account on exit, which
would overwrite the tombstone with an 81-byte marker. The instruction therefore validates address, owner,
discriminator and payer by hand.

- [ ] **Step 1: Update the close tests to the tombstone behaviour (failing)**

In `integration-tests/tests/close_fulfill_marker.rs`:

1. Change imports to
   `use eco_svm_std::{prover, CANCELLED, CHAIN_ID};` and `use portal::state::{self, FulfillMarker, FulfillTombstone};`,
   and add `use local_prover::state::ProofAccount;`. Remove `use anchor_lang::error::ErrorCode;` if it becomes unused.
2. Replace `rent_exempt_minimum` with:

```rust
fn marker_rent(ctx: &common::Context) -> u64 {
    ctx.get_sysvar::<Rent>()
        .minimum_balance(8 + FulfillMarker::INIT_SPACE)
}

fn tombstone_rent(ctx: &common::Context) -> u64 {
    ctx.get_sysvar::<Rent>()
        .minimum_balance(8 + FulfillTombstone::INIT_SPACE)
}
```

3. Replace `close_fulfill_marker_success` with:

```rust
#[test]
fn close_fulfill_marker_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let payer = ctx.payer.pubkey();
    let refund = marker_rent(&ctx) - tombstone_rent(&ctx);
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    assert_eq!(ctx.balance(&fulfill_marker), marker_rent(&ctx));
    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    let payer_balance = ctx.balance(&payer);

    let result = ctx
        .portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker);

    assert!(
        result.is_ok_and(common::contains_event(FulfillMarkerClosed::new(
            intent.intent_hash,
            payer,
            claimant,
            refund,
        )))
    );
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_none());
    assert_eq!(
        ctx.account::<FulfillTombstone>(&fulfill_marker).unwrap(),
        FulfillTombstone::new(claimant)
    );
    assert_eq!(
        ctx.get_account(&fulfill_marker).unwrap().data.len(),
        8 + FulfillTombstone::INIT_SPACE
    );
    assert_eq!(ctx.balance(&fulfill_marker), tombstone_rent(&ctx));
    assert_eq!(ctx.balance(&payer), payer_balance + refund - TRANSACTION_FEE);
}
```

4. In `close_fulfill_marker_batch_success`, replace `let rent = rent_exempt_minimum(&ctx);` with
   `let refund = marker_rent(&ctx) - tombstone_rent(&ctx);`, the per-marker assertion body with
   `assert!(ctx.account::<FulfillTombstone>(fulfill_marker).is_some());`, and the balance assertion with
   `payer_balance + refund * markers.len() as u64 - TRANSACTION_FEE`.
5. In `fulfill_after_close_fail`, append `assert!(ctx.account::<FulfillTombstone>(&fulfill_marker).is_some());`.
6. In `close_fulfill_marker_wrong_intent_hash_fail`, change the expected error to
   `PortalError::InvalidFulfillMarker`.
7. In `close_fulfill_marker_twice_fail`, change the expected error to `PortalError::InvalidFulfillMarker`.
8. Replace `prove_after_close_fail` (and its doc comment) with:

```rust
/// The tombstone keeps the claimant, so closing no longer strands an unproven
/// fulfillment: the solver can still prove after reclaiming rent.
#[test]
fn prove_after_close_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let proof = prover::Proof::pda(&intent.intent_hash, &local_prover::ID).0;
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent.intent_hash],
        CHAIN_ID,
        vec![fulfill_marker],
        state::dispatcher_pda(&local_prover::ID).0,
        vec![proof],
    );

    assert!(result.is_ok());
    assert!(claimant == ctx.account::<ProofAccount>(&proof).unwrap().0.claimant);
}

/// A closed marker must not read as "never fulfilled": `cancel` still finds
/// the PDA occupied.
#[test]
fn cancel_after_close_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().cancel_intent(
        intent.intent_hash,
        &intent.route,
        intent.reward_hash,
        fulfill_marker,
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentAlreadyFulfilled)));
    assert!(ctx.account::<FulfillTombstone>(&fulfill_marker).is_some());
}

/// A cancelled marker closes like any other and still proves as a cancellation.
#[test]
fn close_cancelled_marker_keeps_sentinel_success() {
    let mut ctx = common::Context::default();
    let (_, mut route, reward) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    let reward_hash = reward.hash();
    let intent_hash = portal::types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().close_fulfill_marker(intent_hash, fulfill_marker);

    assert!(result.is_ok());
    assert_eq!(
        ctx.account::<FulfillTombstone>(&fulfill_marker).unwrap(),
        FulfillTombstone::new(CANCELLED)
    );
}
```

- [ ] **Step 2: Add the tombstone layout test (failing)**

In `programs/portal/src/state.rs`, inside `mod tests`, add:

```rust
    /// Pins size, discriminator and field order: `prove` reads `claimant` out
    /// of a tombstone, so all three are ABI.
    #[test]
    fn fulfill_tombstone_layout_deterministic() {
        use anchor_lang::Discriminator;

        let tombstone = FulfillTombstone::new([1u8; 32].into());

        goldie::assert_json!((
            8 + FulfillTombstone::INIT_SPACE,
            FulfillTombstone::DISCRIMINATOR,
            borsh::to_vec(&tombstone).unwrap()
        ));
    }
```

- [ ] **Step 3: Run to verify failures**

Run: `cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p portal fulfill_tombstone_layout_deterministic`
Expected: compile error `cannot find type FulfillTombstone`.

- [ ] **Step 4: Add `FulfillTombstone` and `fulfillment_claimant`**

In `programs/portal/src/state.rs`, add `use crate::instructions::PortalError;` to the imports, and after
`impl FulfillMarker { ... }`:

```rust
/// What `close_fulfill_marker` leaves at a [`FulfillMarker`]'s PDA.
///
/// Keeping the PDA occupied is what keeps `cancel` sound: a deleted marker is
/// indistinguishable from "never fulfilled", so `cancel` could otherwise
/// succeed on a fulfilled intent after its solver reclaimed rent. It also
/// keeps `fulfill` failing, and it keeps `claimant`, so the intent stays
/// provable after the close.
#[account]
#[derive(InitSpace, Debug, PartialEq, new)]
pub struct FulfillTombstone {
    pub claimant: Bytes32,
}

/// Claimant recorded for an intent on this chain, read from its live
/// [`FulfillMarker`] or from the [`FulfillTombstone`] it was closed to.
pub fn fulfillment_claimant(account: &AccountInfo) -> Result<Bytes32> {
    let data = account.try_borrow_data()?;

    if let Ok(marker) = FulfillMarker::try_deserialize(&mut &data[..]) {
        return Ok(marker.claimant);
    }

    FulfillTombstone::try_deserialize(&mut &data[..])
        .map(|tombstone| tombstone.claimant)
        .map_err(|_| PortalError::InvalidFulfillMarker.into())
}
```

Update the `FulfillMarker` doc comment's sentence "`prove` deserializes the full struct (`prove.rs`)" to
"`prove` deserializes the full struct (`fulfillment_claimant`)".

- [ ] **Step 5: Generate the golden and check it**

Run: `GOLDIE_UPDATE=1 cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p portal fulfill_tombstone_layout_deterministic`
Expected: `programs/portal/src/state/testdata/fulfill_tombstone_layout_deterministic.golden` holds `40`, an
8-byte discriminator, and 32 bytes of `1`.

- [ ] **Step 6: Rewrite `close_fulfill_marker`**

Replace `programs/portal/src/instructions/close_fulfill_marker.rs` with:

```rust
use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::events::FulfillMarkerClosed;
use crate::instructions::{now, PortalError};
use crate::state::{FulfillMarker, FulfillTombstone};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseFulfillMarkerArgs {
    pub intent_hash: Bytes32,
}

/// Reclaims most of a [`FulfillMarker`]'s rent to the payer that funded it by
/// shrinking the marker in place to a [`FulfillTombstone`].
///
/// # Why a tombstone, not a close
///
/// The PDA must stay occupied. `cancel` treats an empty marker PDA as "never
/// fulfilled", so deleting it would let anyone cancel an intent that was in
/// fact fulfilled — and if that cancellation's proof reached the source before
/// the solver's, the creator would be refunded for a delivered route. The
/// tombstone keeps `fulfill` and `cancel` failing and keeps the claimant, so
/// the intent also stays provable after the close.
///
/// Solana charges rent on a 128-byte account overhead, so shrinking 81 bytes
/// to 40 returns only about a fifth of the marker's rent.
///
/// # Deadline gate
///
/// `route.deadline` (stored in the marker because `fulfill` sees no reward)
/// must have passed: until then the marker is also the double-fulfill guard.
///
/// # The payer must be solver-controlled
///
/// `payer` is the sole authority able to close the marker and the address the
/// rent returns to, so a sponsored or ephemeral fee-payer would strand it.
#[derive(Accounts)]
#[instruction(args: CloseFulfillMarkerArgs)]
pub struct CloseFulfillMarker<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address, owner, discriminator and payer are validated in
    /// `close_fulfill_marker`. Not an `Account<FulfillMarker>`: that wrapper
    /// re-serializes the marker on exit and would overwrite the tombstone.
    #[account(mut)]
    pub fulfill_marker: UncheckedAccount<'info>,
}

pub fn close_fulfill_marker(
    ctx: Context<CloseFulfillMarker>,
    args: CloseFulfillMarkerArgs,
) -> Result<()> {
    let CloseFulfillMarkerArgs { intent_hash } = args;
    let fulfill_marker = &ctx.accounts.fulfill_marker;
    let payer = &ctx.accounts.payer;

    require!(
        fulfill_marker.key() == FulfillMarker::pda(&intent_hash).0,
        PortalError::InvalidFulfillMarker
    );
    require!(
        fulfill_marker.owner == &crate::ID,
        PortalError::InvalidFulfillMarker
    );
    let FulfillMarker {
        claimant,
        payer: marker_payer,
        deadline,
        ..
    } = FulfillMarker::try_deserialize(&mut &fulfill_marker.try_borrow_data()?[..])
        .map_err(|_| Error::from(PortalError::InvalidFulfillMarker))?;

    require!(
        marker_payer == payer.key(),
        PortalError::InvalidFulfillMarkerPayer
    );
    require!(deadline < now()?, PortalError::RouteNotExpired);

    let tombstone_len = 8 + FulfillTombstone::INIT_SPACE;
    fulfill_marker.resize(tombstone_len)?;
    FulfillTombstone::new(claimant)
        .try_serialize(&mut &mut fulfill_marker.try_borrow_mut_data()?[..])?;

    let refunded = fulfill_marker
        .get_lamports()
        .checked_sub(Rent::get()?.minimum_balance(tombstone_len))
        .ok_or(ProgramError::ArithmeticOverflow)?;
    fulfill_marker.sub_lamports(refunded)?;
    payer.add_lamports(refunded)?;

    emit!(FulfillMarkerClosed::new(
        intent_hash,
        payer.key(),
        claimant,
        refunded,
    ));

    Ok(())
}
```

- [ ] **Step 7: Make `prove` accept tombstones**

In `programs/portal/src/instructions/prove.rs`:
- change `use crate::state::{dispatcher_pda, FulfillMarker, DISPATCHER_SEED};` to
  `use crate::state::{dispatcher_pda, fulfillment_claimant, FulfillMarker, DISPATCHER_SEED};`
- replace the `IntentHashAndFulfillMarker` alias and `fulfill_marker_and_prove_accounts` with:

```rust
type IntentHashAndClaimant = (Bytes32, Bytes32);

fn intent_hash_claimants_and_prove_accounts<'info>(
    ctx: &Context<'info, Prove<'info>>,
    intent_hashes: Vec<Bytes32>,
) -> Result<(Vec<IntentHashAndClaimant>, &'info [AccountInfo<'info>])> {
    require!(
        intent_hashes.len() <= ctx.remaining_accounts.len(),
        PortalError::InvalidFulfillMarker
    );
    let (fulfill_markers, prove_accounts) = ctx.remaining_accounts.split_at(intent_hashes.len());

    let intent_hash_claimants = fulfill_markers
        .iter()
        .zip(intent_hashes)
        .map(|(fulfill_marker, intent_hash)| {
            require!(
                fulfill_marker.key() == FulfillMarker::pda(&intent_hash).0,
                PortalError::InvalidFulfillMarker
            );

            Ok((intent_hash, fulfillment_claimant(fulfill_marker)?))
        })
        .try_collect()?;

    Ok((intent_hash_claimants, prove_accounts))
}
```

- in `prove_intent`, rename the call and bindings:

```rust
    let (intent_hash_claimants, prove_accounts) =
        intent_hash_claimants_and_prove_accounts(&ctx, intent_hashes)?;

    intent_hash_claimants
        .iter()
        .for_each(|(intent_hash, claimant)| {
            emit!(IntentProven::new(*intent_hash, *claimant));
        });

    invoke_prover_prove(
        &ctx,
        source_chain_domain_id,
        intent_hash_claimants,
        prove_accounts,
        data,
    )?;
```

- in `invoke_prover_prove`, change the parameter to `intent_hash_claimants: Vec<IntentHashAndClaimant>` and the
  mapping to:

```rust
    let intent_hashes_claimants = intent_hash_claimants
        .into_iter()
        .map(|(intent_hash, claimant)| IntentHashClaimant::new(intent_hash, claimant))
        .collect::<Vec<_>>();
```

- [ ] **Step 8: Update the event doc**

In `programs/portal/src/events.rs`, replace the `FulfillMarkerClosed` doc comment with:

```rust
/// Emitted when a marker is shrunk to its tombstone. `lamports` is what was
/// returned to `payer`; the tombstone keeps `claimant`, recorded here too so
/// the event stream is a complete audit record.
```

- [ ] **Step 9: Build and run**

Run:
```bash
(cd "$WS" && anchor build)
cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p portal
cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test close_fulfill_marker --test prove_local_prover --test prove_hyper_prover --test cancel
```
Expected: all pass.

- [ ] **Step 10: Commit**

```bash
git -C "$WS" add programs/portal/src/state.rs \
  programs/portal/src/state/testdata/fulfill_tombstone_layout_deterministic.golden \
  programs/portal/src/instructions/close_fulfill_marker.rs programs/portal/src/instructions/prove.rs \
  programs/portal/src/events.rs integration-tests/tests/close_fulfill_marker.rs
git -C "$WS" commit -m "feat(portal): leave a FulfillTombstone when closing a fulfill marker

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

### Task 5: `withdraw` rejects a proven cancellation; share `close_proof`

**Files:**
- Create: `programs/portal/src/instructions/close_proof.rs`
- Modify: `programs/portal/src/instructions/mod.rs`
- Modify: `programs/portal/src/instructions/withdraw.rs:9-18,93,103-122,266-308`
- Test: `integration-tests/tests/withdraw.rs`

**Interfaces:**
- Consumes: `CANCELLED` (Task 1), `PortalError::IntentCancelled` (Task 2).
- Produces: `pub(crate) fn crate::instructions::close_proof::close_proof<'info>(prover: &AccountInfo<'info>,
  proof_closer: &AccountInfo<'info>, proof: &AccountInfo<'info>, remaining_accounts: &[AccountInfo<'info>])
  -> Result<()>` (used by Task 6).

- [ ] **Step 1: Write the failing test**

Append to `integration-tests/tests/withdraw.rs`; add `use eco_svm_std::CANCELLED;` and
`use portal::instructions::PortalError;` to its imports:

```rust
/// P0: `withdraw` does not require the claimant to sign, so without this guard
/// anyone could pay a cancelled intent's reward to the unowned `CANCELLED` key.
#[test]
fn withdraw_intent_cancelled_fail() {
    let (mut ctx, (destination, _, reward), route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let cancelled = Pubkey::new_from_array(CANCELLED.into());
    let vault_balance = ctx.balance(&vault);

    ctx.set_proof(proof, Proof::new(destination, cancelled), hyper_prover::ID);

    let result = ctx.portal().withdraw_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        cancelled,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        proof_closer_pda(&reward.prover).0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentCancelled)));
    assert_eq!(ctx.balance(&cancelled), 0);
    assert_eq!(ctx.balance(&vault), vault_balance);
    assert!(ctx.get_account(&proof).is_some());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test withdraw withdraw_intent_cancelled_fail`
Expected: FAIL — `validate_proof` accepts the proof (the passed claimant key equals the recorded one), so the
withdraw proceeds and fails later with `InvalidMint` (no token accounts passed); the `IntentCancelled` assertion
fails. With token accounts supplied it would have paid the `CANCELLED` key — the bug this guard closes.

- [ ] **Step 3: Extract the shared `close_proof` CPI**

Create `programs/portal/src/instructions/close_proof.rs`:

```rust
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::prover::CLOSE_PROOF_DISCRIMINATOR;

use crate::state::{proof_closer_pda, PROOF_CLOSER_SEED};

/// CPIs `prover`'s `close_proof`, signing `proof_closer_pda(prover)`, and
/// forwards `remaining_accounts` (the prover's rent recipient and whatever
/// else its `close_proof` needs) unchanged.
pub(crate) fn close_proof<'info>(
    prover: &AccountInfo<'info>,
    proof_closer: &AccountInfo<'info>,
    proof: &AccountInfo<'info>,
    remaining_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    let prover_key = prover.key();
    let (_, bump) = proof_closer_pda(&prover_key);
    let signer_seeds = [PROOF_CLOSER_SEED, prover_key.as_ref(), &[bump]];

    let remaining_account_metas = remaining_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    });

    let ix = Instruction::new_with_bytes(
        prover_key,
        &CLOSE_PROOF_DISCRIMINATOR,
        vec![
            AccountMeta::new_readonly(proof_closer.key(), true),
            AccountMeta::new(proof.key(), false),
        ]
        .into_iter()
        .chain(remaining_account_metas)
        .collect(),
    );

    invoke_signed(
        &ix,
        [proof_closer.clone(), proof.clone()]
            .into_iter()
            .chain(remaining_accounts.iter().cloned())
            .collect::<Vec<_>>()
            .as_slice(),
        &[&signer_seeds],
    )
    .map_err(Into::into)
}
```

In `programs/portal/src/instructions/mod.rs` add `mod close_proof;` (private; after `mod close_fulfill_marker;`).

- [ ] **Step 4: Use it from `withdraw` and add the guard**

In `programs/portal/src/instructions/withdraw.rs`:
- delete the local `fn close_proof` (lines 266-308);
- imports: `use eco_svm_std::prover::Proof;`, `use eco_svm_std::{Bytes32, CANCELLED};`,
  `use crate::instructions::close_proof::close_proof;`, and
  `use crate::state::{proof_closer_pda, vault_pda, WithdrawnMarker, CLAIMED_MARKER_SEED, VAULT_SEED};`
  (drop `PROOF_CLOSER_SEED`, `CLOSE_PROOF_DISCRIMINATOR`, `AccountMeta`, `Instruction`, `invoke_signed` only if
  now unused — `invoke_signed` is still used by `withdraw_native`);
- replace line 93 `close_proof(&ctx, remaining_accounts)?;` with:

```rust
    close_proof(
        &ctx.accounts.prover,
        &ctx.accounts.proof_closer,
        &ctx.accounts.proof,
        remaining_accounts,
    )?;
```

- in `validate_proof`, make the match:

```rust
    match Proof::try_from_account_info(&ctx.accounts.proof)? {
        // checked first: a cancelled proof's claimant is a real, if unowned,
        // key, and `withdraw` does not require the claimant to sign
        Some(proof) if CANCELLED == proof.claimant => Err(PortalError::IntentCancelled.into()),
        Some(proof)
            if proof.claimant == *ctx.accounts.claimant.key && proof.destination == destination =>
        {
            Ok(())
        }
        _ => Err(PortalError::IntentNotFulfilled.into()),
    }
```

- [ ] **Step 5: Build and run the withdraw suites**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test withdraw --test withdraw_confused_deputy --test close_proof_hyper_prover --test close_proof_local_prover --test flash_fulfill`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git -C "$WS" add programs/portal/src/instructions/close_proof.rs programs/portal/src/instructions/mod.rs \
  programs/portal/src/instructions/withdraw.rs integration-tests/tests/withdraw.rs
git -C "$WS" commit -m "feat(portal): refuse to withdraw a proven cancellation

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

### Task 6: `refund` fast path on a proven cancellation

**Files:**
- Modify: `programs/portal/src/instructions/refund.rs` (args, accounts, status, tail split, close)
- Modify: `integration-tests/tests/common/portal_context.rs:150-201`
- Test: `integration-tests/tests/refund.rs`

**Interfaces:**
- Consumes: `CANCELLED` (Task 1), `close_proof` helper (Task 5).
- Produces:
  - `RefundArgs { destination: u64, route_hash: Bytes32, reward: Reward, close_proof_account_count: u8 }`
  - `accounts::Refund { payer, creator, vault, proof (mut), proof_closer, prover, withdrawn_marker,
    token_program, token_2022_program, system_program }`
  - remaining accounts: `[from, to, mint] × k` token chunks, then the last `close_proof_account_count`
    accounts forwarded to the prover's `close_proof` (hyper-prover: `[pda_payer (w)]`; local-prover:
    `[payer (s, w)]`). The tail is used only on the cancellation path.
  - test builders: `refund_intent(...)` (unchanged signature, empty tail) and
    `refund_intent_with_close_proof(..., close_proof_accounts: Vec<AccountMeta>)`.

Two deliberate choices, both flagged in the report to the spec owner:
1. The tail length is an explicit `u8` arg because refund's token chunk count is caller-chosen (it sweeps any
   vault token account), so the split point cannot be derived from the reward as `withdraw` does.
2. `prover` is address-checked but **not** `executable`-constrained: a timeout refund must still work for an
   intent whose `reward.prover` is not a deployed program. Executability is required only on the cancellation
   path, where a `Proof` owned by that program already exists.

- [ ] **Step 1: Update the builders**

In `integration-tests/tests/common/portal_context.rs`, add `use portal::state::proof_closer_pda;` and replace
`refund_intent` with:

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn refund_intent(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        self.refund_intent_with_close_proof(
            destination,
            reward,
            vault,
            route_hash,
            proof,
            withdrawn_marker,
            creator,
            token_transfer_accounts,
            vec![],
        )
    }

    /// `close_proof_accounts` is the tail forwarded to the prover's
    /// `close_proof` on the cancellation path.
    #[allow(clippy::too_many_arguments)]
    pub fn refund_intent_with_close_proof(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
        close_proof_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let prover = reward.prover;
        let args = portal::instructions::RefundArgs {
            destination,
            route_hash,
            reward,
            close_proof_account_count: close_proof_accounts.len().try_into().unwrap(),
        };
        let instruction = portal::instruction::Refund { args };
        let accounts: Vec<_> = portal::accounts::Refund {
            payer: self.payer.pubkey(),
            creator,
            vault,
            proof,
            proof_closer: proof_closer_pda(&prover).0,
            prover,
            withdrawn_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .chain(close_proof_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
```

- [ ] **Step 2: Parameterize the refund test setup by prover**

In `integration-tests/tests/refund.rs`, rename `fn setup(is_token_2022: bool)` to
`fn setup_with_prover(is_token_2022: bool, prover: Pubkey)`, and right after
`let (destination, _, reward) = ctx.rand_intent();` change the binding to `mut reward` and add
`reward.prover = prover;`. Then add:

```rust
fn setup(is_token_2022: bool) -> (common::Context, u64, Reward, Bytes32) {
    setup_with_prover(is_token_2022, hyper_prover::ID)
}
```

- [ ] **Step 3: Write the failing tests**

Append to `integration-tests/tests/refund.rs`. Imports: add `use eco_svm_std::CANCELLED;`,
`use anchor_lang::Space;` and `use solana_sdk::rent::Rent;`, and widen the existing
`use hyper_prover::state::pda_payer_pda;` to `use hyper_prover::state::{pda_payer_pda, ProofAccount};`:

```rust
fn cancelled_proof(destination: u64) -> Proof {
    Proof::new(destination, Pubkey::new_from_array(CANCELLED.into()))
}

#[test]
fn refund_intent_cancelled_before_deadline_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let pda_payer = pda_payer_pda().0;
    let proof_rent = ctx
        .get_sysvar::<Rent>()
        .minimum_balance(8 + ProofAccount::INIT_SPACE);

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    let pda_payer_balance = ctx.balance(&pda_payer);
    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_intent_with_close_proof(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        reward.creator,
        vec![],
        vec![AccountMeta::new(pda_payer, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_none());
    assert_eq!(ctx.balance(&pda_payer), pda_payer_balance + proof_rent);
}

/// A cancellation proven for another destination is not this intent's
/// cancellation: the fallback deadline still applies.
#[test]
fn refund_intent_cancelled_on_wrong_destination_not_expired_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination + 1), hyper_prover::ID);

    let result = ctx.portal().refund_intent_with_close_proof(
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::RewardNotExpired
    )));
}

/// The cancelled proof must be closed, so a missing `close_proof` tail fails
/// the whole refund rather than leaking the prover's rent.
#[test]
fn refund_intent_cancelled_without_close_proof_accounts_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
    );

    assert!(result.is_err());
    assert_eq!(ctx.balance(&reward.creator), 0);
    assert!(ctx.get_account(&proof).is_some());
}

/// Refund gained `proof_closer`/`prover` accounts; the timeout path must still
/// work when `reward.prover` is not a deployed program.
#[test]
fn refund_intent_expired_with_non_program_prover_success() {
    let (mut ctx, destination, reward, route_hash) = setup_with_prover(false, Pubkey::new_unique());
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        Proof::pda(&intent_hash, &reward.prover).0,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
    );

    assert!(result.is_ok());
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
}
```

- [ ] **Step 4: Run to verify failures**

Run: `cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test refund`
Expected: compile errors (`close_proof_account_count`, `proof_closer`, `prover` not fields of `RefundArgs` /
`accounts::Refund`).

- [ ] **Step 5: Implement the fast path**

In `programs/portal/src/instructions/refund.rs`:

Imports:

```rust
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::token_interface::{close_account, CloseAccount};
use anchor_spl::{token, token_2022};
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED};

use crate::events::IntentRefunded;
use crate::instructions::close_proof::close_proof;
use crate::instructions::{now, PortalError};
use crate::state::{proof_closer_pda, vault_pda, WithdrawnMarker, VAULT_SEED};
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};
```

Args and accounts:

```rust
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RefundArgs {
    pub destination: u64,
    pub route_hash: Bytes32,
    pub reward: Reward,
    /// Number of trailing remaining accounts forwarded to the prover's
    /// `close_proof` when refunding a proven cancellation. The token chunks
    /// before them are caller-chosen, so the split cannot be derived.
    pub close_proof_account_count: u8,
}

#[derive(Accounts)]
#[instruction(args: RefundArgs)]
pub struct Refund<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut, address = args.reward.creator @ PortalError::InvalidCreator)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub proof: UncheckedAccount<'info>,
    /// CHECK: address is validated, scoped to the intent's prover
    #[account(address = proof_closer_pda(&args.reward.prover).0 @ PortalError::InvalidProofCloser)]
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: address is validated. Deliberately not `executable`: a timeout
    /// refund must work for an intent whose `reward.prover` is not a deployed
    /// program; executability is checked on the cancellation path only.
    #[account(address = args.reward.prover @ PortalError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub withdrawn_marker: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub system_program: Program<'info, System>,
}
```

Handler (replace `refund_intent`, `validate_intent_status`, `is_fulfilled`, `refund_tokens`):

```rust
/// Why a refund is allowed. Only `Cancelled` closes the proof.
#[derive(PartialEq, Eq)]
enum RefundPath {
    Withdrawn,
    Cancelled,
    Expired,
}

pub fn refund_intent<'info>(ctx: Context<'info, Refund<'info>>, args: RefundArgs) -> Result<()> {
    let RefundArgs {
        destination,
        route_hash,
        reward,
        close_proof_account_count,
    } = args;
    let intent_hash = types::intent_hash(destination, &route_hash, &reward.hash());
    let (vault_pda, bump) = vault_pda(&intent_hash);
    let signer_seeds = [VAULT_SEED, intent_hash.as_ref(), &[bump]];

    require!(
        ctx.accounts.vault.key() == vault_pda,
        PortalError::InvalidVault
    );
    require!(
        ctx.accounts.proof.key() == Proof::pda(&intent_hash, &reward.prover).0,
        PortalError::InvalidProof
    );
    require!(
        ctx.accounts.withdrawn_marker.key() == WithdrawnMarker::pda(&intent_hash).0,
        PortalError::InvalidWithdrawnMarker
    );

    let refund_path = validate_intent_status(&ctx, &reward, destination)?;
    let (token_transfer_accounts, close_proof_accounts) =
        token_transfer_and_close_proof_accounts(&ctx, close_proof_account_count)?;

    refund_native(&ctx, &signer_seeds)?;
    refund_tokens(&ctx, &signer_seeds, token_transfer_accounts)?;

    if refund_path == RefundPath::Cancelled {
        require!(ctx.accounts.prover.executable, PortalError::InvalidProver);
        close_proof(
            &ctx.accounts.prover,
            &ctx.accounts.proof_closer,
            &ctx.accounts.proof,
            close_proof_accounts,
        )?;
    }

    emit!(IntentRefunded::new(intent_hash, reward.creator));

    Ok(())
}

// TODO: allow early recover if the token specified is not a reward token (before anything)
fn validate_intent_status<'info>(
    ctx: &Context<'info, Refund<'info>>,
    reward: &Reward,
    destination: u64,
) -> Result<RefundPath> {
    if !ctx.accounts.withdrawn_marker.data_is_empty() {
        return Ok(RefundPath::Withdrawn);
    }

    match Proof::try_from_account_info(&ctx.accounts.proof.to_account_info())? {
        // proven cancellation for this destination: refundable immediately
        Some(proof) if proof.destination == destination && CANCELLED == proof.claimant => {
            return Ok(RefundPath::Cancelled);
        }
        // fulfilled but not withdrawn
        Some(proof) if proof.destination == destination => {
            return Err(PortalError::IntentFulfilledAndNotWithdrawn.into());
        }
        // no proof, or a proof for another destination
        _ => {}
    }

    require!(reward.deadline <= now()?, PortalError::RewardNotExpired);

    Ok(RefundPath::Expired)
}

type RefundRemainingAccounts<'info> = (&'info [AccountInfo<'info>], &'info [AccountInfo<'info>]);

fn token_transfer_and_close_proof_accounts<'info>(
    ctx: &Context<'info, Refund<'info>>,
    close_proof_account_count: u8,
) -> Result<RefundRemainingAccounts<'info>> {
    let split_index = ctx
        .remaining_accounts
        .len()
        .checked_sub(close_proof_account_count.into())
        .ok_or(Error::from(PortalError::InvalidTokenTransferAccounts))?;

    Ok(ctx.remaining_accounts.split_at(split_index))
}

fn refund_tokens<'info>(
    ctx: &Context<'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
    token_transfer_accounts: &'info [AccountInfo<'info>],
) -> Result<()> {
    let accounts: VecTokenTransferAccounts<'info> = token_transfer_accounts.try_into()?;

    accounts
        .into_inner()
        .into_iter()
        .try_for_each(|accounts| refund_token(ctx, signer_seeds, accounts))
}
```

`refund_native` and `refund_token` are unchanged.

- [ ] **Step 6: Build and run the refund suite**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test refund`
Expected: all pass (existing tests unchanged, 4 new).

- [ ] **Step 7: Commit**

```bash
git -C "$WS" add programs/portal/src/instructions/refund.rs \
  integration-tests/tests/common/portal_context.rs integration-tests/tests/refund.rs
git -C "$WS" commit -m "feat(portal): refund a proven cancellation before reward.deadline

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

### Task 7: End-to-end flows through real provers

**Files:**
- Create: `integration-tests/tests/proven_cancellation.rs`
- Modify: `integration-tests/tests/handle.rs`

**Interfaces:**
- Consumes: `cancel_intent`, `refund_intent_with_close_proof`, `prove_intent_via_local_prover`,
  `withdraw_intent`, `fund_intent` builders; `CANCELLED`; `PortalError::IntentCancelled`.

- [ ] **Step 1: Write the local-prover flow tests**

Create `integration-tests/tests/proven_cancellation.rs`:

```rust
use anchor_lang::prelude::AccountMeta;
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};
use portal::events::IntentRefunded;
use portal::instructions::PortalError;
use portal::state::{self, proof_closer_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{self, Reward, Route};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

/// A same-chain intent proven by local-prover, funded with native reward only,
/// cancelled on this chain and proven back to it: the full SVM round trip.
fn cancelled_and_proven_intent(ctx: &mut common::Context) -> (Route, Reward, Bytes32) {
    let (_, mut route, mut reward) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    reward.prover = local_prover::ID;
    reward.tokens.clear();
    let route_hash = route.hash();
    let intent_hash = types::intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, reward.native_amount).unwrap();
    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            state::vault_pda(&intent_hash).0,
            route_hash,
            false,
            vec![],
        )
        .unwrap();

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward.hash(), fulfill_marker)
        .unwrap();
    ctx.portal()
        .prove_intent_via_local_prover(
            vec![intent_hash],
            CHAIN_ID,
            vec![fulfill_marker],
            state::dispatcher_pda(&local_prover::ID).0,
            vec![Proof::pda(&intent_hash, &local_prover::ID).0],
        )
        .unwrap();

    (route, reward, intent_hash)
}

#[test]
fn cancel_prove_refund_via_local_prover_before_reward_deadline_success() {
    let mut ctx = common::Context::default();
    let (route, reward, intent_hash) = cancelled_and_proven_intent(&mut ctx);
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let payer = ctx.payer.pubkey();

    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_intent_with_close_proof(
        CHAIN_ID,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route.hash(),
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
        vec![AccountMeta::new(payer, true)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_none());
}

#[test]
fn cancel_prove_withdraw_via_local_prover_fail() {
    let mut ctx = common::Context::default();
    let (route, reward, intent_hash) = cancelled_and_proven_intent(&mut ctx);
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let cancelled = Pubkey::new_from_array(CANCELLED.into());
    let payer = ctx.payer.pubkey();

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route.hash(),
        cancelled,
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        proof_closer_pda(&local_prover::ID).0,
        vec![],
        vec![AccountMeta::new(payer, true)],
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentCancelled)));
    assert_eq!(ctx.balance(&cancelled), 0);
    assert!(ctx.get_account(&proof).is_some());
}
```

- [ ] **Step 2: Write the hyper-prover inbound test**

Append to `integration-tests/tests/handle.rs`; add `use eco_svm_std::CANCELLED;`,
`use portal::events::IntentRefunded;` and `use portal::types::Reward;` to the imports:

```rust
/// A cancellation arriving over Hyperlane is recorded verbatim (no prover
/// change) and unlocks refund before `reward.deadline`, returning the proof's
/// rent to `pda_payer`.
#[test]
fn handle_cancellation_enables_refund_success() {
    let mut ctx = setup();
    let destination: u32 = random();
    let (_, _, mut reward): (u64, _, Reward) = ctx.rand_intent();
    reward.tokens.clear();
    let route_hash: Bytes32 = random::<[u8; 32]>().into();
    let intent_hash = intent_hash(destination.into(), &route_hash, &reward.hash());
    let vault = vault_pda(&intent_hash).0;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, reward.native_amount).unwrap();
    ctx.portal()
        .fund_intent(
            destination.into(),
            reward.clone(),
            vault,
            route_hash,
            false,
            vec![],
        )
        .unwrap();

    let payload = ProofData::new(
        destination.into(),
        vec![IntentHashClaimant::new(intent_hash, CANCELLED)],
    )
    .to_bytes();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );
    let pda_payer = pda_payer_pda().0;
    let pda_payer_balance = ctx.balance(&pda_payer);
    let sender = ctx.sender.pubkey();
    let handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    ctx.hyperlane()
        .inbox_process(message, handle_account_metas)
        .unwrap();

    let proof = Proof::pda(&intent_hash, &hyper_prover::ID).0;
    assert!(CANCELLED == ctx.account::<ProofAccount>(&proof).unwrap().0.claimant);
    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_intent_with_close_proof(
        destination.into(),
        reward.clone(),
        vault,
        route_hash,
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
        vec![AccountMeta::new(pda_payer, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_none());
    assert_eq!(ctx.balance(&pda_payer), pda_payer_balance);
}
```

- [ ] **Step 3: Build and run**

Run: `(cd "$WS" && anchor build) && cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" -p integration-tests --test proven_cancellation --test handle`
Expected: all pass. (These exercise code from Tasks 1–6; if one fails, fix the implementation, not the
assertion.)

- [ ] **Step 4: Commit**

```bash
git -C "$WS" add integration-tests/tests/proven_cancellation.rs integration-tests/tests/handle.rs
git -C "$WS" commit -m "test(portal): cover cancel, prove and refund end to end through both provers

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

### Task 8: Documentation and full verification

**Files:**
- Modify: `CLAUDE.md` (Architecture → portal bullet; cross-cutting "Intent hash" bullet unchanged)
- Modify: `README.md:266-268`

- [ ] **Step 1: Update `CLAUDE.md`**

In the **portal** bullet of "Programs (`programs/*`)":
- the instruction list becomes `publish`, `fund`, `fulfill`, `cancel`, `prove`, `withdraw`, `refund`,
  `close_fulfill_marker`;
- replace the sentence starting "`close_fulfill_marker` reclaims the `FulfillMarker`'s rent…" through
  "…see `programs/portal/src/instructions/close_fulfill_marker.rs`." with:

```markdown
`cancel` (permissionless, strictly after `route.deadline`) writes the `CANCELLED` sentinel
(`eco_svm_std::CANCELLED`, byte-identical to EVM `Inbox.CANCELLED`) into the same `FulfillMarker` PDA that
`fulfill` writes, so the two are mutually exclusive and `prove` carries a cancellation unchanged. On the source, a
`Proof` whose claimant is `CANCELLED` is a proven cancellation: `refund` accepts it before `reward.deadline` and
CPIs the prover's `close_proof` (tail length in `RefundArgs.close_proof_account_count`), and `withdraw` rejects it
with `IntentCancelled` — that guard is load-bearing because `withdraw` does not require the claimant to sign.
`close_fulfill_marker` (stored `payer` only, after `route.deadline`) shrinks the marker in place to a 40-byte
`FulfillTombstone { claimant }` and returns the difference (~20% of the rent). The PDA must stay occupied: a
deleted marker would read as "never fulfilled" and let `cancel` succeed on a fulfilled intent. The tombstone keeps
the claimant, so `prove` still works after a close.
```

- [ ] **Step 2: Update `README.md`**

Replace line 268 (the `close_fulfill_marker` bullet) with:

```markdown
- `cancel` - After `route.deadline`, permanently cancel an unfulfilled intent on its destination. Permissionless. Writes the `CANCELLED` sentinel into the intent's fulfill marker; `prove` then carries it to the source, where `refund` succeeds before `reward.deadline`.
- `close_fulfill_marker` - Reclaim part of a `FulfillMarker`'s rent once `route.deadline` has passed, by shrinking it to a `FulfillTombstone` that keeps the claimant. Signed by the marker's stored `payer`, which is also the refund target. The tombstone keeps the intent provable and blocks both `fulfill` and `cancel`.
```

and change line 266 to `- \`refund\` - Refund intent after \`reward.deadline\`, or immediately once its cancellation is proven`.

- [ ] **Step 3: Full verification**

Run each and confirm it is clean:

```bash
(cd "$WS" && cargo +nightly fmt --check)
(cd "$WS" && cargo +1.97.1 sort --workspace --check)
cargo +1.97.1 clippy --manifest-path "$WS/Cargo.toml" --all-targets -- -D warnings
(cd "$WS" && anchor build)
cargo build-sbf --manifest-path "$WS/integration-tests/programs/mock-igp/Cargo.toml" --sbf-out-dir "$WS/target/deploy"
cargo +1.97.1 test --manifest-path "$WS/Cargo.toml" --no-fail-fast
(cd "$WS" && anchor run build-mainnet)
```
Expected: no fmt diff, sorted manifests, no clippy warnings, every test passes, and the mainnet build succeeds
(proves the on-chain code compiles under platform-tools rustc). If `cargo +nightly fmt --check` reports a diff,
run `(cd "$WS" && cargo +nightly fmt)` and include the formatted files in this task's commit.

- [ ] **Step 4: Commit**

```bash
git -C "$WS" add CLAUDE.md README.md
git -C "$WS" commit -m "docs: document cancel, the fulfill tombstone and the cancellation refund

Claude-Session: https://claude.ai/code/session_01Fs7GU5DoDuKhLo9VDktMXP"
```

---

## Spec coverage (self-review)

| Spec item | Task |
|---|---|
| §5 `cancel` (args, accounts, strict deadline, hash, marker create, `IntentCancelled`) | 3 |
| §5 `fulfill` rejects `CANCELLED` (`ReservedClaimant`) | 2 |
| §5 tombstone: resize in place, 40 B, lamports to payer, PDA occupied, `prove` accepts both, no re-close | 4 |
| §5 errors appended: `ReservedClaimant`, `IntentCancelled` | 2 |
| §7 `eco-svm-std::CANCELLED`, golden-pinned to EVM bytes | 1 |
| §7 `Proof` / provers / flash-fulfiller unchanged | Global Constraints (no task touches them) |
| §7 `withdraw` rejects `CANCELLED` | 5, 7 |
| §7 `refund` fast path + `close_proof` rent return | 6, 7 |
| §9 invariants 1 (mutual exclusion incl. after close), 2, 4, 5, 7 | 3, 4, 5, 6, 1 |
| §10 SVM column (boundaries, exclusion, tombstone, fast refund, withdraw guard, fallback, conflicts, cross-VM, tombstone rent) | 3, 4, 6, 7, 1; conflicts = existing `handle` redelivery tests, unchanged |
| D8 minor release | commit conventions |
