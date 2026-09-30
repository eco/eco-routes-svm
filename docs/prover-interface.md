# Proof queries and cleanup

This release changes Portal, prover and Flash-Fulfiller instruction ABIs. Deploy the programs together under new IDs; regenerate clients from the release IDLs. Proof encoding, destination markers, withdrawal markers and bridge payloads are unchanged.

## Prover interface

| Instruction | Arguments | Result |
| --- | --- | --- |
| `get_proof` | `GetProofArgs { intent_hash, destination, data: Vec<u8> }` | `Option<Proof>` |
| `close_proof` | `CloseProofArgs { intent_hash, data: Vec<u8> }` | Unit |

`data` belongs to the invoked prover. Local, Hyper and Polymer need no query data. They require their canonical proof PDA for the hash and return `None` for missing/pre-funded accounts, invalid contents, zero claimants or a destination mismatch. Substituted/omitted accounts and execution errors remain errors.

`Proof::is_cancelled()` identifies cancellation; `Proof::is_payable_to(&claimant)` checks a nonzero, non-cancellation payout recipient. Portal applies settlement policy to the returned proof.

`eco_svm_std::prover::cpi::get_proof` passes all accounts read-only and unsigned. It requires the invoked program as return-data producer and an exact Borsh `Option<Proof>` encoding, with no trailing bytes. A returned proof must match the requested destination and have a nonzero claimant. CPI/decoding errors never become `None`. The aggregator republishes the result under its own program ID.

## Aggregator framing

The aggregator's `data` is Borsh-encoded `Vec<MemberQuery>`. Each entry has `account_count: u8` (excluding the member program) and opaque `data: Vec<u8>` forwarded to that member.

Accounts are `[config, member_program_0, member_accounts_0..., member_program_1, member_accounts_1..., ...]`. Counts delimit the slices; members may need different numbers of accounts and different data. The aggregator validates membership, uniqueness and complete account consumption before dispatch.

`get_proof` requires every configured member exactly once, in caller-supplied order. It returns the first `Some(proof)`; `None` requires every member to return `None`. A member execution error reached before a proof is found propagates. Where members disagree, caller order determines which proof is returned, retaining selected-member settlement. There is no aggregate proof PDA or proof event.

`close_proof` forwards only the first member group's accounts and data, with the inherited Portal signer. Following groups are permitted so cancellation cleanup can use the identical tail for querying and closing. With a withdrawal marker, only the selected member group is needed.

Account framing does not impose a single-account restriction on members. Solana CPI depth and reentrancy restrictions still apply; nesting the same deployed aggregator program is not supported.

## Portal accounts

- `withdraw`: payer (writable signer), claimant (writable), vault (writable), prover (executable, equals `reward.prover`), withdrawn marker (writable), Token, Token-2022, System. Remaining: one token triple per unique reward mint, then query tail. Arguments: `WithdrawArgs { destination, route_hash, reward, prover_data }`.
- `refund`: payer (writable signer), creator (writable), vault (writable), optional prover, withdrawn marker (writable), Token, Token-2022, System. Remaining: caller-selected token triples, then query tail. Arguments: `RefundArgs { destination, route_hash, reward, prover_data, prover_account_count }`. The count separates the tails. The prover may be omitted after withdrawal; any supplied prover must equal `reward.prover`.
- `close_proof`: withdrawn marker (read-only, may be absent), prover (executable, equals `reward.prover`), proof closer (read-only). Remaining: cleanup tail. Arguments: `CloseProofArgs { destination, route_hash, reward, prover_data }`.

Portal forwards `prover_data` unchanged. Concrete query tails are `[proof]`; concrete cleanup tails are `[proof, rent_recipient]`. For cancellation cleanup the same tail must support both `get_proof` and `close_proof`; concrete getters ignore the cleanup recipient. Put the selected member first for aggregate cleanup. A payable proof from that member blocks cancellation cleanup even if another member holds cancellation evidence.

Token triples remain `[from, to, mint]`. Cleanup marks the proof and recipient writable; Local/Polymer require a signing recipient, Hyperlane requires its PDA payer. Every prover's close instruction takes the Portal closer signer before its tail. Flash-Fulfiller retains prove/withdraw/fulfill/sweep order and the 256-KiB heap requirement, and leaves its proof for later cleanup.

## Refund and cleanup policy

Refund chooses its path from state: an authentic withdrawal marker permits sweeping without querying a prover; a cancellation proof permits refund immediately; any other proof blocks refund; `None` requires the reward deadline. A supplied non-executable root permits timeout refund, but an omitted root or failed CPI cannot establish absence. Before the deadline cancellation refunds must sweep every reward mint; at/after it partial sweeps are allowed.

Withdrawal creates the permanent `WithdrawnMarker`; refunds create no state. Neither closes a proof. Independent cleanup requires either that marker or cancellation evidence at/after the reward deadline. Timeout, an empty vault or a refund event alone never authorizes closure of fulfillment evidence.

Portal recomputes the intent hash and signs `[b"proof_closer", intent_hash, bump]`. Every leaf requires both that Portal signer and its own canonical proof PDA for the same hash. The hash already commits to `reward.prover`.

Close each member independently. Late delivery or redelivery remains cleanable under the same rules. Closing a missing proof errors without changing settlement state. A cleanup failure in a later transaction cannot roll back settlement; when bundled in one transaction, any failure rolls back the bundle.
