# Proven Cancellation: Client-Visible Changes

An intent whose route was never fulfilled can be cancelled on its destination after `route.deadline`, proven back to its source like a fulfillment, and refunded there before `reward.deadline`. This page lists everything a client, solver or refund service must change to work with it.

## Release Note

The `refund` instruction's account list and arguments change shape, and `close_fulfill_marker` is removed with the `FulfillMarker` shrinking to 41 bytes, which breaks every existing caller. This is acceptable only because each release deploys the programs under new program IDs; do not upgrade a deployed portal in place.

`CANCELLED` is deliberately a valid EVM address (see below), so an old-generation source (an EVM prover, or an
old SVM portal, which stores claimants verbatim) would treat it as a payable claimant, and a permissionless
`withdraw` would burn the reward. Every release therefore needs a new EVM root SALT and new program IDs, and
prover whitelists must never cross generations.

## Destination Chain

### `cancel` (new)

- Accounts: `payer` (signer, writable), `fulfill_marker` (writable, `FulfillMarker::pda(intent_hash)`), `system_program`, then every call's accounts in call order as remaining accounts, passed read-only and unsigned.
- Args: `CancelArgs { intent_hash, route, reward_hash, account_flags: Vec<u8> }`. `route` is in the same compact form `fulfill` takes: each call's `data` is a borsh `Calldata`, whose `account_count` accounts are the next remaining accounts. `account_flags` has one byte per remaining account: bit 0 (`ACCOUNT_FLAG_SIGNER`) is the committed `is_signer`, bit 1 (`ACCOUNT_FLAG_WRITABLE`) the committed `is_writable`; any other bit is rejected. The portal rebuilds each call's canonical `CalldataWithAccounts` from these and hashes the result, so the original call signers are not needed. The flags cost one byte per call-account slot, repeats included, which `fulfill` does not pay, so a route near the transaction size limit can fit `fulfill` but not `cancel`; such an intent refunds from `reward.deadline` instead. `route.portal` must equal the portal ID. A count mismatch between calls, remaining accounts and flags fails with `InvalidCalldata`; a wrong flag or key fails with `InvalidIntentHash`.
- Permissionless, allowed only once `route.deadline < now`. It creates a `FulfillMarker { claimant: CANCELLED }` at the intent's marker PDA, the same account `fulfill` writes, so it fails with `IntentAlreadyFulfilledOrCancelled` if the intent was fulfilled (and `fulfill` fails once it is cancelled). Cancelling an already-cancelled intent is a no-op (no event, no rent), so a front-run `cancel` never fails a `cancel` + `prove` transaction. The payer pays the marker's rent-exempt minimum (1,176,240 lamports at the default rent), and the marker is permanent: nothing can close it, which is what keeps a fulfilled intent from ever being cancelled.
- Emits `IntentCancelled { intent_hash }`.
- `CANCELLED` (`eco_svm_std::CANCELLED`) is the EVM address `0xe685056aEc77686A83E2a6bDf37c6f71dD2fdB5f` (the low
  20 bytes of `keccak256("eco.portal.intent.cancelled")`) left-padded to 32 bytes, byte-identical to EVM
  `Inbox.CANCELLED`. It is deliberately a valid EVM address, hash-derived so no EVM or Solana key controls it.

### `prove`

- Proves a cancelled intent exactly like a fulfilled one, so the prover's `IntentProven` event (and the source `Proof`) can carry `claimant == CANCELLED`. Indexers must treat that claimant as a cancellation, not as a payee.
- The claimant is read from the `FulfillMarker`, and only from an account the portal owns (`InvalidFulfillMarker` otherwise).

### `fulfill`

- Rejects `claimant == CANCELLED` with the new error `ReservedClaimant`.
- `IntentAlreadyFulfilledOrCancelled` means the marker PDA is already occupied. From `fulfill` the intent was fulfilled or cancelled; read the marker's claimant to tell which (`CANCELLED` for a cancellation). From `cancel` it always means fulfilled, since a repeat cancel succeeds. A payer short of SOL does not produce it: the failed System Program CPI aborts the transaction with its own error.

### `close_fulfill_marker` (removed)

- The instruction, its `FulfillMarkerClosed` event and the `InvalidFulfillMarkerPayer` error's only use are gone; the error keeps its slot so later error codes keep their numbers. Fulfill-marker rent is no longer reclaimable.
- Closing a marker freed its PDA, so an intent fulfilled and then closed could later be cancelled, sending the source a cancellation proof that conflicts with the fulfillment. A permanent marker rules that out. Reclaiming rent safely once the reward is settled is future work.

## Source Chain

### `refund`

- Accounts, in order: `payer` (signer, writable), `creator` (writable), `vault` (writable), `proof` (**now writable**), `proof_closer` (**new, optional**, `proof_closer_pda(reward.prover)`), `prover` (**new, optional**, must equal `reward.prover`), `withdrawn_marker` (writable), `token_program`, `token_2022_program`, `system_program`. Then the remaining accounts: the token chunks, followed by the close-proof tail.
- `proof_closer` and `prover` are Anchor optional accounts: pass the portal program ID in their slots to omit them. Only the proven-cancellation path uses them, and it fails with `InvalidProofCloser` or `InvalidProver` when either is omitted; the withdrawn and timeout paths work without them. When passed, each is checked against its address on every path. Omitting both keeps a timeout refund within a legacy transaction: five reward mints, payer ≠ creator and no compute-budget instruction serialize to 1,232 bytes with them omitted and 1,296 bytes with them passed.
- Args: `RefundArgs` gains `close_proof_account_count: u8`, the number of trailing remaining accounts forwarded to the prover's `close_proof`. Pass `0` when no proof is closed.
- Paths, checked in this order:
  1. Withdrawn: refunds leftovers as before.
  2. Proven cancellation (`Proof` for this destination with `claimant == CANCELLED`): refundable at any time. The proof is **always** closed through the prover's `close_proof` (so the close-proof tail is always required and `reward.prover` must be an executable program, `InvalidProver` otherwise), before and after `reward.deadline`. `reward.deadline` decides only which token chunks are required:
     - Before `reward.deadline` (the fast path): the token chunks must sweep every reward mint. Each mint in `reward.tokens` needs a chunk whose source is the vault's ATA for that mint, or the refund fails with `InvalidMint` and nothing moves; extra non-reward mints remain allowed. Every reward mint's vault ATA must therefore exist and be transferable; a client should create any missing one idempotently in the same transaction.
     - From `reward.deadline`: the refund sweeps whatever chunks it is given, like a timeout refund, so a reward mint that cannot be swept (closed or non-transferable mint, frozen or missing vault ATA) may be omitted instead of locking the rest.
     - The fast path is live only while `reward.prover` can close its proofs, which is why production provers are deployed finalized (no upgrade authority): a finalized prover can never be closed, so `close_proof` cannot fail for that reason.
  3. Fulfilled for this destination and not withdrawn: `IntentFulfilledAndNotWithdrawn`, as before.
  4. Otherwise (no proof, or a proof for another destination) the deadline gate applies (`RewardNotExpired` before `reward.deadline`), as before.

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

`FulfillMarker` does not appear in the portal IDL: the IDL only lists account types used as typed `Account<...>` fields, and `fulfill`, `cancel` and `prove` all take the marker PDA as an unchecked account. It is an Anchor account at `FulfillMarker::pda(intent_hash)` (seeds `[b"fulfill_marker", intent_hash]`), owned by the portal, Borsh-encoded after an 8-byte discriminator (`sha256("account:FulfillMarker")[..8]`).

`FulfillMarker` — 41 bytes, written by `fulfill` (the solver's claimant) or by `cancel` (`CANCELLED`):

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 8 | discriminator | `[226, 174, 31, 239, 112, 220, 188, 31]` |
| 8 | 32 | `claimant` | `Bytes32`; `CANCELLED` for a cancelled intent |
| 40 | 1 | `bump` | PDA bump |

`fulfill_marker_layout_deterministic` in `programs/portal/src/state.rs` pins the encoding.
