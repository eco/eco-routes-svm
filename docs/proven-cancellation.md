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

The account ABI now delegates validation to the configured prover; see [Prover validation and cleanup](prover-interface.md) for complete account ordering.

### `refund`

- Pass `RefundArgs { destination, route_hash, reward, kind, prover_account_count }`. The final count separates prover accounts from caller-selected token triples.
- `Expired`: require the reward deadline and a wildcard negative from every configured member. A supplied non-executable root still permits timeout refund; an omitted root or failed CPI does not.
- `Cancelled`: require `cpi::validate_cancelled` to return true. Before the reward deadline every reward mint must be swept; at/after it partial sweeps are allowed. Proofs remain intact, so subsequent refunds can reuse them.
- `Withdrawn`: require the authentic withdrawal marker and an empty prover tail; refund the supplied assets without executing a prover.
- Refund creates no marker and performs no proof cleanup. Validation accounts are read-only and unsigned. Larger requests may need a versioned transaction with a lookup table; the five-mint timeout test covers this path.

### `withdraw`

Uses `cpi::validate_proof` for the payout claimant and requires true, otherwise `IntentNotFulfilled`. The std helper rejects zero and cancellation claimants. Withdrawal creates the existing permanent marker and leaves proofs intact. `IntentCancelled` keeps its existing error slot but is no longer returned by withdrawal.

### `close_proof`

Separate Portal instruction taking the intent preimage: `CloseProofArgs { destination, route_hash, reward }`. With an authentic withdrawal marker, any member proof for that intent can be closed. Otherwise require `now >= reward.deadline` and `cpi::validate_cancelled == true` for the same selected proof. A prior refund is not required. Before the deadline the cancellation evidence remains available for early refunds; after cleanup use remaining cancellation evidence or the timeout path if all members have no applicable proof.

Hyperlane rent goes to its PDA payer; Local/Polymer rent goes to the supplied writable signing payer. Validation does not forward signer privileges. Cleanup preserves its rent-recipient privileges, so clients should only supply signers to trusted provers. Cleanup can be submitted later, once per member, including for late-arriving proofs. Bundling it with settlement shares the transaction rollback boundary.

## Errors and Events

- `ReservedClaimant`: destination fulfillment attempted to use the cancellation claimant.
- `IntentNotCancelled`: cancellation refund or cleanup received a negative cancellation query.
- `RewardNotExpired`: timeout refund, or cancellation cleanup without withdrawal, preceded the reward deadline.
- `IntentCancelled { intent_hash: Bytes32 }`: destination cancellation event, unchanged.

## Account Layouts

`FulfillMarker` does not appear in the portal IDL: the IDL only lists account types used as typed `Account<...>` fields, and `fulfill`, `cancel` and `prove` all take the marker PDA as an unchecked account. It is an Anchor account at `FulfillMarker::pda(intent_hash)` (seeds `[b"fulfill_marker", intent_hash]`), owned by the portal, Borsh-encoded after an 8-byte discriminator (`sha256("account:FulfillMarker")[..8]`).

`FulfillMarker` — 41 bytes, written by `fulfill` (the solver's claimant) or by `cancel` (`CANCELLED`):

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 8 | discriminator | `[226, 174, 31, 239, 112, 220, 188, 31]` |
| 8 | 32 | `claimant` | `Bytes32`; `CANCELLED` for a cancelled intent |
| 40 | 1 | `bump` | PDA bump |

`fulfill_marker_layout_deterministic` in `programs/portal/src/state.rs` pins the encoding.
