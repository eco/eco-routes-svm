# Prover validation and cleanup

This release changes Portal, prover and Flash-Fulfiller instruction ABIs. Deploy the programs together under new IDs; regenerate clients from the release IDLs. Proof encoding, destination markers, withdrawal markers and bridge payloads are unchanged.

## Prover interface

| Instruction | Arguments | Result |
| --- | --- | --- |
| `validate_proof` | `ValidateProofArgs { intent_hash, destination, claimant: Option<Pubkey> }` | `bool` |
| `close_proof` | `intent_hash: Bytes32` | Unit |

`Some(claimant)` checks an exact claimant. `None` checks for any valid nonzero claimant, including cancellation. A negative exact query does not establish absence of other claimants.

A concrete validator requires its canonical proof PDA for the hash. Missing/pre-funded or invalid contents, zero claimant and destination/claimant mismatch return false. Substituted/omitted accounts and execution errors remain errors. Validation changes no state and receives no signer or writable privileges through the shared CPI helpers.

`eco_svm_std::prover::cpi` exposes `validate_proof` (payable claimant), `validate_cancelled` (sentinel hidden inside std), `has_proof` (wildcard), `invoke_validate_proof` (adapter query) and `close_proof`. Boolean decoding requires exactly one byte, 0 or 1, with the invoked program as return-data producer. CPI/decoding errors never become false. The aggregator explicitly republishes the member result under its own program ID.

## Account tails

| Operation | Concrete tail | Aggregator tail |
| --- | --- | --- |
| Exact validation | `[proof]` | `[config, selected_member_program, proof]` |
| Wildcard validation | `[proof]` | `[config, member_program_0, proof_0, ...]` |
| Cleanup | `[proof, rent_recipient]` | `[config, selected_member_program, proof, rent_recipient]` |

Wildcard queries require every configured member exactly once in configuration order. The aggregator checks the complete framing before dispatch and returns false only after every member returns false. Exact queries and cleanup require configured executable membership. The aggregator owns only immutable config; it has no aggregate proof or proof event. Members are concrete programs with the single-proof-account validation ABI; nested aggregation is unsupported.

For cancellation cleanup the same tail is validated and closed. Validation ignores the cleanup-only rent recipient. Concrete proof cleanup accepts the Portal closer signer first, then the concrete tail; the aggregator accepts the closer signer first, then the aggregator tail.

## Portal accounts

- `withdraw`: payer (writable signer), claimant (writable), vault (writable), prover (executable, equals `reward.prover`), withdrawn marker (writable), Token, Token-2022, System. Remaining: one token triple per unique reward mint, then validation tail. Arguments remain `WithdrawArgs { destination, route_hash, reward }`.
- `refund`: payer (writable signer), creator (writable), vault (writable), optional prover, withdrawn marker (writable), Token, Token-2022, System. Remaining: caller-selected token triples, then validation tail. Arguments: `RefundArgs { destination, route_hash, reward, kind, prover_account_count }`. Kinds are `Expired`, `Cancelled`, `Withdrawn`. The prover is required for the first two and may be omitted only for `Withdrawn`; any supplied prover must equal `reward.prover`.
- `close_proof`: withdrawn marker (read-only, may be absent), prover (executable, equals `reward.prover`), proof closer (read-only). Remaining: cleanup tail. Arguments: `CloseProofArgs { destination, route_hash, reward }`.

Token triples remain `[from, to, mint]`. Validation account tails are read-only and unsigned. Cleanup marks the proof and recipient writable; Local/Polymer require a signing recipient, Hyperlane requires its PDA payer. Flash-Fulfiller drops its closer account, retains prove/withdraw/fulfill/sweep order and the 256-KiB heap requirement, and leaves its proof for a later Portal cleanup instruction.

## Cleanup authority and lifecycle

Withdrawal creates the permanent `WithdrawnMarker`; refunds create no state. Neither closes a proof. Independent cleanup requires either an authentic withdrawal marker or a validated cancellation at/after the reward deadline. Timeout, an empty vault or a refund event alone never authorizes closure of fulfillment evidence.

Portal recomputes the intent hash and signs `[b"proof_closer", intent_hash, bump]`. Every leaf requires both that Portal signer and its own canonical proof PDA for the same hash. The intent hash already commits to `reward.prover`; forwarding does not need another prover argument or whitelist.

Close each member independently. Late delivery or redelivery remains cleanable under the same rules. Closing a missing proof errors without changing settlement state. A cleanup failure in a later transaction cannot roll back settlement; when bundled in one transaction, any failure rolls back the bundle.
