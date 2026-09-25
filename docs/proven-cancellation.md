# Proven Cancellation: Client-Visible Changes

An intent whose route was never fulfilled can be cancelled on its destination after `route.deadline`, proven back to its source like a fulfillment, and refunded there before `reward.deadline`. This page lists everything a client, solver or refund service must change to work with it.

## Release Note

The `refund` instruction's account list and arguments change shape, which breaks every existing caller. This is acceptable only because each release deploys the programs under new program IDs; do not upgrade a deployed portal in place.

## Destination Chain

### `cancel` (new)

- Accounts: `payer` (signer, writable), `fulfill_marker` (writable, `FulfillMarker::pda(intent_hash)`), `system_program`.
- Args: `CancelArgs { intent_hash, route, reward_hash }`. `route` must be the canonical route the source committed to, with `route.portal` equal to the portal ID.
- Permissionless, allowed only once `route.deadline < now`. It creates the intent's `FulfillMarker` with `claimant = CANCELLED`, so it fails if the intent was fulfilled (and `fulfill` fails once it is cancelled).
- Emits `IntentCancelled { intent_hash }`.
- `CANCELLED` is `keccak256("eco.portal.intent.cancelled")` (`eco_svm_std::CANCELLED`), byte-identical to EVM `Inbox.CANCELLED`.

### `prove`

- Proves a cancelled intent exactly like a fulfilled one, so the prover's `IntentProven` event (and the source `Proof`) can carry `claimant == CANCELLED`. Indexers must treat that claimant as a cancellation, not as a payee.
- The claimant is now read from a `FulfillMarker` or a `FulfillTombstone`, and only from an account the portal owns (`InvalidFulfillMarker` otherwise).

### `fulfill`

- Rejects `claimant == CANCELLED` with the new error `ReservedClaimant`.
- The marker-init failure `IntentAlreadyFulfilled` (also returned by `cancel`) is raised for *any* failure to create the marker, including a payer short of SOL. Services must read the marker account's state to learn whether the intent was really fulfilled or cancelled, rather than trusting the error.

### `close_fulfill_marker`

- A wrong marker PDA, a marker that is not a live `FulfillMarker` (including a second close) and an account the portal does not own now all fail with `InvalidFulfillMarker` (previously `ConstraintSeeds` / `AccountNotInitialized`).
- The marker is no longer deleted: it is shrunk in place to a 40-byte `FulfillTombstone` that keeps the claimant. The intent stays provable and both `fulfill` and `cancel` keep failing on it.
- `FulfillMarkerClosed.lamports` is now the partial refund (the marker's rent minus the tombstone's rent-exempt minimum), not the marker's full balance.

## Source Chain

### `refund`

- Accounts, in order: `payer` (signer, writable), `creator` (writable), `vault` (writable), `proof` (**now writable**), `proof_closer` (**new**, `proof_closer_pda(reward.prover)`), `prover` (**new**, must equal `reward.prover`), `withdrawn_marker` (writable), `token_program`, `token_2022_program`, `system_program`. Then the remaining accounts: the token chunks, followed by the close-proof tail.
- Args: `RefundArgs` gains `close_proof_account_count: u8`, the number of trailing remaining accounts forwarded to the prover's `close_proof`. Pass `0` when no proof is closed.
- Paths, checked in this order:
  1. Withdrawn: refunds leftovers as before.
  2. Proven cancellation (`Proof` for this destination with `claimant == CANCELLED`, and `reward.prover` is an executable program): refunds immediately, at any time, and closes the proof through the prover's `close_proof`. Because this branch comes before the deadline branch, a refund of a cancelled-and-proven intent **still needs the close-proof tail after `reward.deadline`**.
  3. Otherwise the deadline gate applies (`RewardNotExpired` before `reward.deadline`). A proven cancellation whose `reward.prover` is no longer an executable program takes this path: it refunds only after `reward.deadline`, needs no close-proof tail, and leaves the proof open.
- On the cancellation path the token chunks must sweep every reward mint: each mint in `reward.tokens` needs a chunk whose source is the vault's ATA for that mint, or the refund fails with `InvalidMint` and nothing moves. Extra non-reward mints remain allowed. The withdrawn and deadline paths keep sweeping only the chunks they are given.

### `withdraw`

- Fails with the new error `IntentCancelled` when the proof's claimant is `CANCELLED`.

### Close-Proof Tail

- hyper-prover's `close_proof` takes `pda_payer` (writable), which receives the proof's rent.
- local-prover's `close_proof` takes `payer` (writable, **signer**), which receives the proof's rent. On a refund that is whoever supplies the tail, usually the refund caller.
- Because the tail is forwarded as-is, a refund service should forward a signer in the tail only to provers it allowlists; a signer passed to an unknown `reward.prover` is a signer that program can use.

## New Errors and Events

- Errors (appended to `PortalError`): `ReservedClaimant`, `IntentCancelled`.
- Event: `IntentCancelled { intent_hash: Bytes32 }`.

## Account Layouts

Neither account appears in the portal IDL: the IDL only lists account types used as typed `Account<...>` fields, and `fulfill`, `cancel`, `prove` and `close_fulfill_marker` all take the marker PDA as an unchecked account. Both are Anchor accounts at `FulfillMarker::pda(intent_hash)` (seeds `[b"fulfill_marker", intent_hash]`), owned by the portal, Borsh-encoded after an 8-byte discriminator (`sha256("account:<Name>")[..8]`). Tell them apart by discriminator.

`FulfillMarker` — 81 bytes, written by `fulfill` and `cancel`:

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 8 | discriminator | `[226, 174, 31, 239, 112, 220, 188, 31]` |
| 8 | 32 | `claimant` | `Bytes32`; `CANCELLED` for a cancelled intent |
| 40 | 32 | `payer` | `Pubkey` allowed to close the marker, and its rent target |
| 72 | 8 | `deadline` | `u64` little-endian, `route.deadline` |
| 80 | 1 | `bump` | PDA bump |

`FulfillTombstone` — 40 bytes, left by `close_fulfill_marker`:

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 8 | discriminator | `[167, 239, 111, 226, 98, 112, 202, 27]` |
| 8 | 32 | `claimant` | `Bytes32`, copied from the closed marker |

`fulfill_marker_layout_deterministic` and `fulfill_tombstone_layout_deterministic` in `programs/portal/src/state.rs` pin both encodings.
