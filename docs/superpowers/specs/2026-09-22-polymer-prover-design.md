# Polymer prover for Solana — design

Date: 2026-09-22
Status: approved design, awaiting implementation plan
Ticket: PAR-670
Counterpart: `contracts/prover/PolymerProver.sol` in eco-routes

## 1. Goal

Add a Polymer-backed prover to the Solana intent protocol so that intents can be
proven in both directions between Solana and EVM chains using Polymer's Prove API,
with no messaging fee and no relayer trust beyond Polymer's sequencer signature.

Two directions, one Solana program:

| Direction | Intent published on | Fulfilled on | Proof consumed by |
|---|---|---|---|
| Inbound | Solana | EVM | Solana `polymer-prover.validate` |
| Outbound | EVM | Solana | EVM `PolymerProver.validateSolana` (new) |

The eco-routes `PolymerProver` is extended in the same effort (section 5). It works
as-is for the inbound direction and needs a new validation path for the outbound one.

## 2. Background: how Polymer proves things

### 2.1 EVM event proofs consumed on Solana

Polymer runs a deployed Solana program (`polymer_prover`, Anchor 0.31.1, source at
`polymerdao/solana-prover-contracts`, v1.0.4):

| Cluster | Program ID |
|---|---|
| mainnet-beta | `CdvSq48QUukYuMczgZAVNZrwcHNshBdtqrjW26sQiGPs` |
| devnet | `FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3` |

A relayer proves an EVM event in three steps, all signed by one `authority` key:

1. `create_accounts` — once per authority. Creates `["cache", authority]` (a 3000-byte
   proof buffer) and `["result", authority]` (the decoded event) under Polymer's program.
2. `load_proof(chunk)` — appends proof bytes to the cache, called repeatedly with
   about 800 bytes per transaction.
3. `validate_event` — accounts `[authority (signer, mut), cache (mut), result (mut),
   internal]`. Verifies the secp256k1 sequencer signature over the Polymer state root and
   an IAVL membership proof, then **overwrites** the result account and **clears** the
   cache. It does not return data. The result layout (Borsh, after the 8-byte Anchor
   account discriminator):

   ```
   is_valid: bool
   error_message: String            (max 64)
   chain_id: u32                    (EVM chain that emitted the event)
   emitting_contract: [u8; 20]
   topics: Vec<u8>                  (flat, 32 bytes per topic, max 4 topics)
   unindexed_data: Vec<u8>          (ABI-encoded non-indexed params, max 3000)
   ```

   `["internal"]` holds Polymer's client type, sequencer address and peptide chain ID and
   is pre-initialized by Polymer on both clusters.

Polymer's public docs describe an older interface (`init_if_needed` cache, return data,
`[authority]` seeds). The deployed program source is authoritative and is what this
design targets.

### 2.2 Solana logs proven on EVM

Polymer proves `msg!` log lines. Requirements from Polymer:

- The line starts with `Prove: program: <base58 program id>, ` and is emitted by the
  program being proven. Polymer strips the `Prove: ` prefix in what it returns. That is
  documented but unverified against the deployed indexer, so the EVM parser accepts the
  line with or without the prefix (and with or without the Solana runtime's
  `Program log: ` prefix); the tolerance is confined to the head of the line.
- Fields are comma-delimited. Keep each line under roughly 500 bytes.
- Several `Prove:` lines in one transaction are all proven together.

On EVM, `CrossL2ProverV2.validateSolLogs(bytes proof)` returns
`(uint32 chainId, bytes32 programID, string[] logMessages)`. `chainId` is Polymer's own
identifier for Solana (documented as `2`), `programID` is the raw 32-byte program key,
and the membership proof commits to `keccak(programID ‖ log₁ ‖ log₂ …)`. The mainnet
CrossL2ProverV2 already used by eco-routes on Base (`0x95ccEAE7…`) exposes this function.

## 3. Solana program: `programs/polymer-prover`

### 3.1 Layout

Modelled on hyper-prover. Crate `polymer-prover`, lib `polymer_prover`.

```
programs/polymer-prover/
  Cargo.toml            features: cpi, default, idl-build, mainnet, no-entrypoint, no-idl, no-log-ix-name
  src/lib.rs            declare_id!(<new keypair-backed Eco… ID>), instruction dispatch
  src/polymer.rs        POLYMER_PROVER_ID (feature-gated), PDA helpers, validate_event
                        discriminator, ValidationResult mirror + account decoder
  src/event.rs          IntentFulfilledFromSource selector, topic checks, ABI `bytes` unwrap
  src/state.rs          Config, ProofAccount
  src/instructions/     init.rs, prove.rs, validate.rs, close_proof.rs, mod.rs (error enum)
```

- `mainnet = ["eco-svm-std/mainnet", "portal/mainnet"]`, and `polymer.rs` selects the
  mainnet Polymer program ID under that feature and the devnet ID otherwise. This mirrors
  how `hyperlane.rs` selects `MAILBOX_ID`.
- The Polymer CPI is hand-rolled. `polymer.rs` mirrors only what we consume: the
  `validate_event` discriminator (`sha256("global:validate_event")[..8]`), the four-account
  layout, the `ValidationResultAccount` discriminator
  (`sha256("account:ValidationResultAccount")[..8]`) and its Borsh layout. Unit tests pin
  every constant. We take no dependency on Polymer's crate.
- No dispatcher or `pda_payer` PDA. `prove` performs no CPI, and the relayer pays Proof
  rent directly.
- Program ID: a fresh keypair-backed `Eco…` vanity key, ground before implementation, added
  to `[programs.localnet|devnet|mainnet]` in `Anchor.toml`.

### 3.2 State

- `Config` at `["config"]`: `whitelisted_emitters: Vec<Bytes32>`, max 20. Each entry is
  an EVM `PolymerProver` address left-padded to 32 bytes. Same shape and `AccountExt`
  usage as hyper-prover's `Config`.
- `ProofAccount(eco_svm_std::prover::Proof)` at `["proof", intent_hash]`, the shared
  layout Portal's `withdraw` reads.

### 3.3 `init(InitArgs { whitelisted_emitters })`

Accounts: `config` (mut, address-checked), `payer` (signer, mut), `system_program`.
Creates `Config` via `AccountExt::init`. Runs once per deployment; a second call fails
because the PDA exists. There is no update instruction, matching hyper-prover and the
repo's redeploy-not-upgrade policy.

### 3.4 `validate` (inbound)

Permissionless. Precondition: the caller has run Polymer's `create_accounts` and
`load_proof` under its own key.

Accounts:

| Account | Constraints |
|---|---|
| `authority` | signer, mut. Polymer's cache/result PDAs derive from it; it pays Proof rent. |
| `config` | `Config::pda()` |
| `cache_account` | mut, address `= ["cache", authority]` under Polymer |
| `result_account` | mut, address `= ["result", authority]` under Polymer |
| `internal` | address `= ["internal"]` under Polymer |
| `polymer_prover_program` | executable, address `= POLYMER_PROVER_ID` |
| `system_program` | |
| event-CPI accounts | `#[event_cpi]` |
| `remaining_accounts` | one Proof PDA per hash/claimant pair, in payload order |

Flow:

1. CPI Polymer `validate_event` with `[authority, cache, result, internal]` and the bare
   discriminator. The authority's signature passes through from the outer transaction.
2. Decode `result_account`: owner must be `POLYMER_PROVER_ID`, discriminator must match,
   Borsh-decode `ValidationResult`. Require `is_valid`, else `PolymerProofInvalid` and
   `msg!` Polymer's `error_message`.
3. Apply the same gates as the Solidity `validate` (ordered topics-first here; the
   Solidity checks payload shape first — diagnostics differ, accepted outcomes do not):
   - `emitting_contract` left-padded to `Bytes32` is in `config.whitelisted_emitters`,
     else `InvalidEmittingContract`.
   - `topics.len() == 64`, else `InvalidTopicsLength`.
   - `topics[0..32] == keccak256("IntentFulfilledFromSource(uint64,bytes)")`, else
     `InvalidEventSignature`.
   - `topics[32..64]` is a big-endian `uint64` (first 24 bytes zero) equal to `CHAIN_ID`,
     else `InvalidSourceChain`.
4. Unwrap `unindexed_data` as one ABI-encoded `bytes`: word 0 must be `32`, word 1 is the
   length `L`, the payload is the next `L` bytes, and the buffer must be at least
   `64 + ceil32(L)` long. Any violation is `InvalidEventData`. Then
   `ProofData::from_bytes(payload)` from `eco-svm-std` (8-byte big-endian destination
   followed by 64-byte pairs). Require `proof_data.destination == chain_id as u64`, else
   `InvalidDestinationChain`. Require at least one pair, else `EmptyProofData`.
5. Require `remaining_accounts.len() == pairs.len()`, else `InvalidProof`. For each pair,
   the account must be `Proof::pda(intent_hash, &crate::ID)`, else `InvalidProof`. Then
   apply hyper-prover's idempotency rule: if a `Proof` is already recorded there, it must
   equal `(destination, claimant)`, else `IntentAlreadyProven`; otherwise create it with
   `authority` as payer. This matters for Polymer because a proof can be re-validated any
   number of times and a later EVM `prove()` may legitimately re-include an already-proven
   hash. The 32-byte claimant is recorded as an opaque pubkey by construction: supplying a
   real Solana pubkey when the source chain is Solana is a solver obligation enforced
   off-chain before it calls EVM `fulfill` (any other 32 bytes still record a Proof, which
   blocks `refund`, for an address nobody can spend from). PolymerProver.sol's
   `claimantBytes >> 160 != 0` skip is bytes32-to-address narrowing for its own leg, not a
   validation this side lacks; hyper-prover and local-prover behave the same way.
   There is no partial-batch or resume mode, so the relayer must drain the whole event in
   one transaction. That bounds pairs per event; the limits, in the order they bind
   (`validate.rs::mark_intent_hashes_proven`'s doc comment owns the arithmetic, and
   `validate_polymer_prover.rs` pins each one):

   - The 64-entry instruction trace (`MAX_INSTRUCTION_TRACE_LENGTH`) is the real ceiling:
     3 fixed entries (ComputeBudget, `validate`, the `validate_event` CPI) plus 2 per fresh
     pair (the system `create_account` inside `AccountExt::init`, and `emit_cpi!`), so 30
     fresh pairs. A pre-funded Proof PDA takes `create_account`'s griefing-resistant
     `transfer + allocate + assign` path — 4 entries per pair, 3 at or above the
     rent-exempt minimum — dropping the ceiling to 15-20 (see section 4, batch-ceiling
     griefing).
   - Legacy transaction size: 450 + 33N bytes against the 1232-byte packet, so
     `MAX_PAIRS_PER_VALIDATE_LEGACY_TX` (23) pairs at 1209 bytes; 24 is 1242 and needs a
     v0 transaction with an address lookup table.
   - Compute: ~252k CU at 24 pairs through the mock, so the 1.4M transaction limit is not
     binding but the 200k default is — callers must raise it; Polymer's real
     `validate_event` sits on top (see
     `polymer_prover_context.rs::VALIDATE_COMPUTE_UNIT_LIMIT`).
   - Account locks (64): 10 fixed keys plus N, so 54 pairs — never binding.
   - Polymer's 3000-byte `unindexed_data` cap: 1632 bytes at 24 pairs
     (`64 + ceil32(8 + 64N)`), ~45 pairs — never binding.

   Operational guidance: keep EVM `Inbox.prove(prover = PolymerProver, …)` batches destined
   for Solana at or below `MAX_INTENTS_PER_PROVE` (24), symmetric with the outbound cap so
   one number covers both directions, and deliver them as a v0 transaction with an address
   lookup table; an ALT-free relayer stays at `MAX_PAIRS_PER_VALIDATE_LEGACY_TX`. An
   oversized or griefed event is not lost — `Inbox.claimants` persists and `Inbox.prove` can
   be re-called with a smaller batch, which is also the answer to
   `MaxInstructionTraceLengthExceeded` — but the Polymer proof already requested for it is
   wasted.
6. `emit_cpi!(IntentProven { intent_hash, claimant, destination })` for every pair,
   including no-op ones, so a consumer that missed an earlier delivery still sees the
   recorded state.

Compute: Polymer's `validate_event` is heavy (secp256k1 recovery plus SHA-256 IAVL paths).
Callers set the compute unit limit to 1.4M. Polymer's `unindexed_data` cap of 3000 bytes
bounds a single event to about 45 pairs, well above the instruction-trace ceiling in step 5,
so it never binds.

### 3.5 `prove(ProveArgs)` (outbound)

Reached only through Portal's `prove`, which signs `dispatcher_pda(&polymer_prover::ID)`
into us.

Accounts: `portal_dispatcher` (signer, address
`= portal::state::dispatcher_pda(&crate::ID)`, else `InvalidPortalDispatcher`). Portal
forwards whatever tail the caller supplied; this instruction needs none.

Checks:

- `proof_data.destination == CHAIN_ID`, else `InvalidDestination`.
- `1 <= pairs.len() <= 24`, else `EmptyProofData` / `TooManyIntents`. Solana truncates a
  transaction's log buffer at 10,000 bytes (`LOG_MESSAGES_BYTES_LIMIT`) while the
  transaction still succeeds, so a line that fell past the limit can never be proven. Each
  intent costs about 347 bytes end to end: the 222-byte `Prove:` line, the runtime's 13-byte
  `Program log: ` prefix, and Portal's own ~110-byte `Program data:` `IntentProven` event.
  24 x 347 = ~8.3 KB of intent log, ~8.9 KB measured with the transaction's own framing
  (invoke / success / consumed-CU lines), so ~1.1 KB of the 10 KB budget is left — about
  three more intents. Measured through `portal::prove` in litesvm: every
  `Prove:` line survives up to 27 intents. From 28 the log collector overflows — a single
  mechanism, not a gradient: it appends one `Log truncated` marker and from then on drops
  each message that would cross the limit (a shorter one that still fits is kept), so 28-31
  lose progressively more `Prove:` lines while the transaction still succeeds. 32 instead
  fails outright, exhausting Portal's 32 KB heap. 24 leaves margin for future Portal log
  additions. An integration test drives exactly `MAX_INTENTS_PER_PROVE` intents through
  Portal and asserts no truncation, and that at least two intents' worth of headroom
  remains, so the cap cannot drift up to the buffer.
  Caller contract: the 10,000-byte buffer is shared by every instruction and CPI in the
  transaction, so a `prove` at or near the cap must be the transaction's only log-emitting
  instruction — batching two capped proves, or a capped prove plus other logging
  instructions, truncates again and cannot be rejected on-chain, since the cap is per
  invocation. A truncated batch is recoverable: `portal::prove` writes no state, so the
  relayer can simply re-prove the missing hashes in a fresh transaction.
- `domain_id` is the EVM source chain ID and is passed through untouched. `data` is ignored,
  as in the Solidity `prove`.

Output: one `msg!` per pair.

```
Prove: program: <base58 program id>, <160 lowercase hex chars, no 0x>
```

The hex payload is 80 bytes:

| Offset | Bytes | Field |
|---|---|---|
| 0 | 8 | source chain ID, big-endian (`domain_id`) |
| 8 | 8 | destination chain ID, big-endian (`CHAIN_ID`) |
| 16 | 32 | intent hash |
| 48 | 32 | claimant (`Bytes32`, as recorded in `FulfillMarker`) |

The program ID is written from `ctx.program_id` as Polymer requires. Hex is encoded into a
fixed stack buffer to avoid heap churn. One self-contained line per intent means the EVM
parser never handles a split batch and Polymer's `logMessages` array returns the batch.

### 3.6 `close_proof`

Accounts: `portal_proof_closer` (signer, address
`= portal::state::proof_closer_pda(&crate::ID)`, else `InvalidPortalProofCloser`),
`proof` (mut, `Account<ProofAccount>`), `payer` (signer, mut). Closes the proof to `payer`,
the local-prover model: rent returns to whoever pays for `withdraw`.

### 3.7 Errors

`PolymerProverError`: `InvalidPortalDispatcher`, `InvalidPortalProofCloser`,
`InvalidConfig`, `TooManyWhitelistedEmitters`, `InvalidPolymerProver`,
`InvalidCacheAccount`, `InvalidResultAccount`, `InvalidInternalAccount`,
`PolymerProofInvalid`, `InvalidEmittingContract`, `InvalidTopicsLength`,
`InvalidEventSignature`, `InvalidSourceChain`, `InvalidDestinationChain`,
`InvalidEventData`, `InvalidProof`, `IntentAlreadyProven`, `InvalidDestination`,
`EmptyProofData`, `TooManyIntents`.

## 4. Security properties

- **Freshness.** Validation and result read happen in one instruction. Polymer overwrites
  the result and clears the cache on every `validate_event`, so a stale or foreign result
  cannot be presented.
- **Authenticity of the result.** Owner is Polymer's program, address is
  `["result", authority]` under it, discriminator matches. A look-alike account fails.
- **Emitter and chain binding.** Whitelisted emitter, event selector, topic 1 equals
  `CHAIN_ID`, payload destination equals Polymer's authenticated `chain_id`. These are the
  same four checks as the Solidity `validate`.
- **Replay.** Re-validating a proof is idempotent; a disagreeing proof for a recorded
  intent is rejected.
- **Batch-ceiling griefing (residual).** Anyone can pre-fund a Proof PDA before the
  relayer's `validate` lands; `create_account`'s griefing-resistant path then costs 4
  instruction-trace entries per pair instead of 2, and 7 such PDAs push a 24-pair batch
  past `MAX_INSTRUCTION_TRACE_LENGTH` (`3 + 2(24 - k) + 4k <= 64` gives `k <= 6`). The
  event aborts atomically — nothing is partially proven, no intent is lost,
  `Inbox.claimants` persists — but the Polymer proof is wasted; recovery is a smaller
  re-prove on EVM. Pinned by `validate_prefunded_proof_pdas_lower_the_batch_ceiling`.
- **Prover-scoped authorities.** `prove` accepts only `dispatcher_pda(&crate::ID)` and
  `close_proof` only `proof_closer_pda(&crate::ID)`, preserving the confused-deputy
  boundary documented in CLAUDE.md and exercised by `prove_confused_deputy.rs` and
  `withdraw_confused_deputy.rs`.
- **Config immutability.** `init` runs once. Changing the whitelist means a new release,
  consistent with the redeploy-never-upgrade policy. Residual: `init` is unauthenticated, so
  the first caller after deploy owns the whitelist forever (see Rollout step 4 for the gate);
  and rotating the EVM emitter set later strands in-flight intents that named this program
  ID, the escape hatch being a program upgrade that adds a set-emitters instruction — the
  programs are deployed upgradeable (`deploy-mainnet` in `Anchor.toml` uses plain
  `anchor deploy`, no `--final`).
- **No reentrancy surface.** Polymer's program performs no CPIs back into callers.
- **Atomic release.** `polymer-prover` joins the set that must ship from one tree with
  Portal, since its authorities derive from Portal's ID.

## 5. eco-routes changes (`contracts/prover/PolymerProver.sol`)

Inbound needs nothing: `prove()` already emits `IntentFulfilledFromSource(source,
encodedProofs)` with `sourceChainDomainID` set by the solver to Eco's Solana chain ID
(`1399811149` mainnet, `1399811150` devnet) and `encodedProofs` in the `ProofData` byte
layout.

Outbound additions:

- `ICrossL2ProverV2` gains
  `validateSolLogs(bytes) external view returns (uint32, bytes32, string[] memory)`.
- Constructor gains `uint32 _solanaPolymerChainId` (Polymer's identifier for Solana,
  documented as `2`, confirmed with Polymer per environment before deployment) and
  `uint64 _solanaChainId` (Eco's Solana chain ID). Both stored as immutables. The Solana
  program ID (raw 32 bytes) is added to the existing bytes32 whitelist. Both are rejected
  as zero (`InvalidSolanaChainConfig`). Deliberate: every argument is immutable with no
  setter, so a misconfiguration is only recoverable by redeploying at a new CREATE3 salt
  and re-whitelisting the new address on the Solana side; configured-wrong must fail at
  deploy time, not at first proof.
- `validateSolana(bytes calldata proof)` and `validateSolanaBatch(bytes[] calldata)`:
  1. `(chainId, programID, logs) = CROSS_L2_PROVER_V2.validateSolLogs(proof)`.
  2. `chainId == SOLANA_POLYMER_CHAIN_ID`, else `InvalidDestinationChain`.
  3. `isWhitelisted(programID)`, else `InvalidEmittingContract`-style error carrying the
     bytes32.
  4. For each log: split at the first comma. The head must be `program: <base58>`,
     optionally preceded by an un-stripped `Prove:` and/or the runtime's `Program log: `;
     the shapes parse identically. Require the base58 string to equal, byte for byte, the
     canonical base58 encoding of the authenticated `programID` (Polymer's recommended
     defense in depth against indexer misattribution of nested CPI logs). Comparing the
     canonical string rather than decoding means a non-canonical id (leading `1`
     padding), a non-alphabet character or an over-long field all fail closed with the
     contract's own `SolanaLogProgramMismatch` and no library error escapes. The tail must
     be exactly 160 hex characters; decode to 80 bytes. Require destination equals
     `SOLANA_CHAIN_ID`. Then, if source equals `block.chainid`,
     `processIntent(intentHash, claimant, destination)`, which skips claimants that are
     not 160-bit EVM addresses and, like `BaseProver`, zero claimants (a zero claimant is
     the "unproven" sentinel and must never be recorded or announced).
  5. Malformed logs revert; they indicate a bug or a hostile emitter, never normal traffic.
     A well-formed line whose source is another EVM chain is *not* malformed: a Polymer
     Solana proof covers a whole transaction while `source_chain_domain_id` is set per
     `portal::prove` instruction, so one transaction may carry lines for several source
     chains. Such lines are skipped, but at least one line must be for this chain or the
     call reverts `InvalidSourceChain`, so a proof sent to the wrong chain's prover still
     fails loudly.
- `prove()` and `getProofType()` are unchanged. `validate()` gains one shape check folded
  into the existing decode: a length floor (`decodedData.length < 8`, merged into the
  stride check so a short payload reverts `ArrayLengthMismatch` instead of underflowing to
  `Panic(0x11)`), for parity with `ProofData::from_bytes` in `eco-svm-std`. No other
  behaviour change: a destination-only (zero-pair) payload remains a successful no-op, as
  in `BaseProver`. It deliberately does **not** adopt this side's `EmptyProofData`
  rejection of a zero-pair payload: `Inbox.prove` has no empty-array guard and no access
  control, so anyone can emit a destination-only `IntentFulfilledFromSource` from any
  already-deployed Inbox for one cheap transaction, and because `validateBatch()` is
  atomic a revert there would let that free event discard a whole batch. The Solana
  `validate` can reject it because its own emitter, `portal::prove_intent`, never
  produces one. `validateBatch()`'s code is unchanged; only its NatSpec changes, to state
  the atomicity and that a destination-only element cannot poison a batch.
- Base58 decoding uses a small audited library (research existing Solidity
  implementations before writing one) or a compact in-house decoder if none fits.

Deployment: new `PolymerProver` on each EVM chain that pairs with Solana, Base first,
constructed with the existing Tron whitelist plus the Solana program ID. Every
`PolymerProver` deployable from this branch is therefore Solana-capable:
`POLYMER_SOLANA_CHAIN_ID`, `SOLANA_CHAIN_ID` and `POLYMER_SOLANA_PROVER` are all required
whenever `POLYMER_CROSS_L2_PROVER_V2` is set. A Polymer-only EVM↔EVM chain with no Solana
pairing is out of scope for this branch; if one is ever needed, the right shape is a
separate deploy-time opt-in flag, not a zeroed immutable (a prover whose whitelist omits
the Solana program can only ever revert `InvalidSolanaProgram`).

## 6. Testing

### Unit (goldie 0.7, snapshots in `<module>/testdata/`)

- `event.rs`: ABI `bytes` unwrap for happy path, wrong offset, short buffer, unpadded
  length; selector constant recomputed with `solana_program::keccak`; topic parsing.
- `prove.rs`: hex log line formatting for one and several pairs.
- `polymer.rs`: PDA derivations and both discriminators; `ValidationResult` decode from
  a hand-built buffer, and `try_from_account_info` over an in-memory `AccountInfo`: a
  Polymer-owned account decodes, a fake result account owned by another program, a wrong
  discriminator and a sub-8-byte account are all `InvalidResultAccount`. The foreign-owner
  case lives here rather than in `validate_polymer_prover.rs` because it is unreachable
  through a litesvm `validate` call: the `address = result_pda(&authority)` constraint plus
  the mock's owner-checked `Account<'info, ValidationResultAccount>` make the CPI abort
  with `AccountOwnedByWrongProgram` first.
- `state.rs`: PDAs, whitelist limit.

### Integration (litesvm)

- New `programs/mock-polymer-prover`, an Anchor program registered under
  `[programs.localnet]` only and declared at the devnet Polymer ID. It lives beside
  `dummy-ism` because `anchor build` compiles `programs/*`; `integration-tests/programs/`
  is only for non-Anchor programs built with `cargo build-sbf`. It implements
  `create_accounts`, `load_proof` and `validate_event` with Polymer's exact account seeds
  and layouts. Its `validate_event` interprets the accumulated cache bytes as a Borsh
  `ValidationResultAccount` body, writes it to the result account and clears the cache, so
  tests inject any EVM event. Kept out of devnet and mainnet artifacts by the explicit
  `--program-name` enumeration in `Anchor.toml` and `release.yml`.
- `polymer_prover_context.rs` beside the other contexts with `init`, `validate`, `prove`
  and `close_proof` builders, plus a helper that runs the mock's `create_accounts` and
  `load_proof` for a given event.
- Tests: `init_polymer_prover.rs` (including a full `MAX_WHITELIST_LEN` whitelist — the
  one-shot path that would burn the program ID at rollout), `validate_polymer_prover.rs`
  (happy path with several intents, idempotent re-validation, disagreeing claimant
  rejected, `is_valid == false` with Polymer's `error_message` asserted from the logs, a
  second `validate` without a reload aborting inside Polymer's frame, the claimant shapes
  the EVM leg skips recorded verbatim, the batch ceilings of step 5 — 30 fresh, 15 and 20
  pre-funded, 6 pre-funded within a 24-pair batch, 23 in a legacy packet — each rejected
  check, both directions of the account-count guard, wrong Polymer program account, wrong
  result PDA), `prove_polymer_prover.rs` (log lines asserted from transaction logs, the
  `MAX_INTENTS_PER_PROVE` cap at and above the limit with the log-budget headroom
  measured, non-dispatcher caller, and an empty batch — which Portal rejects with
  `PortalError::EmptyIntentHashes` before the CPI; `check_prove_args`'s
  `InvalidDestination` and `EmptyProofData` branches are defence in depth and unreachable
  through Portal, the only caller that can sign `dispatcher_pda`, since Portal always
  builds `ProofData::new(CHAIN_ID, ..)` and rejects an empty batch first, so they are
  pinned by the `check_prove_args` unit tests in
  `programs/polymer-prover/src/instructions/prove.rs` rather than by integration tests),
  `close_proof_polymer_prover.rs` (through Portal `withdraw`, asserting the account is
  gone and rent returned), and Polymer arms added to the confused-deputy tests where the
  malicious programs can target the new prover.
- `#[ignore]` smoke test: loads Polymer's real `.so` from a local path when present, seeds
  `["internal"]` with the fixture parameters from their repository (`proof_api`, their
  test sequencer address, peptide chain 901), loads their published fixture proof through
  the real `load_proof`, and expects `InvalidTopicsLength` from our `validate` — their
  fixture event carries four topics (a 128-byte blob), ours two, so the
  `topics.len() == 64` check in `IntentFulfilledFromSource::parse` rejects on length before
  it reaches the selector comparison. This proves CPI wiring and result decoding against the
  real binary. The `result` account bytes that real program wrote for the fixture are
  committed as `fixtures/polymer/validation-result-v1.0.4.hex` and decoded by a non-ignored
  test on every PR, so a field reorder or width change in the `polymer.rs` mirror fails
  without network access; a weekly `Polymer upstream drift` workflow re-runs the ignored
  test against the binaries deployed on devnet and mainnet-beta.

### EVM (Foundry, eco-routes)

`PolymerProver.t.sol` gains a mock `CrossL2ProverV2` returning canned `validateSolLogs`
output and covers: happy path single and batch, wrong Polymer chain ID, non-whitelisted
program, base58 mismatch, malformed hex, source chain mismatch, destination mismatch,
non-EVM claimant skipped, already-proven emits `IntentAlreadyProven`.

### Devnet

1. **CPI attribution check, first.** Emit a real `prove` log through Portal on devnet and
   request a proof from Polymer's API. Polymer's docs say logs must come "directly from the
   program you want to validate, not from a CPI". Our program emits its own log but is
   itself invoked by Portal via CPI. If Polymer does not index it, the fallback is a
   permissionless top-level `prove` on `polymer-prover` that reads Portal's `FulfillMarker`
   PDAs directly (they are Portal-owned and readable by anyone).
2. One full round trip in each direction against a devnet `PolymerProver` on an EVM
   testnet.

## 7. Rollout

1. Grind the `Eco…` keypair; set `declare_id!`, `Anchor.toml` entries for all clusters,
   `build-devnet`, `build-mainnet`, `deploy-devnet`, `deploy-mainnet` scripts, and the two
   program loops in `.github/workflows/release.yml`.
2. Add the mock program to `[programs.localnet]` and the integration-test `Context`.
3. Update CLAUDE.md architecture: sixth production program, Polymer's pull-based flow, the
   hand-rolled `polymer.rs` mirror, the atomic-release set.
4. `init` on devnet, then mainnet, as a verified, burn-on-failure gate: immediately after
   `anchor deploy --program-name polymer-prover`, send `init` with the exact EVM
   `PolymerProver` address set, then read `Config` back at `["config"]` and assert it
   byte-equals the intended emitter list. `init` is unauthenticated and the whitelist is
   immutable, so if `init` fails (`ConstraintZero` means someone else won the race) **or**
   the read-back does not match, the program ID is burned: grind a fresh `Eco…` keypair and
   redeploy. Do not proceed to step 5 until the read-back passes — that step is what makes
   the prover load-bearing, and nothing can route to a hijacked config before it.
5. eco-routes: extend `PolymerProver`, redeploy on Base with the Solana program ID
   whitelisted. eco-routes pins this program's `Prove:` wire format as a hand-copied
   literal (`SVM_GOLDEN_LOG` in `test/prover/PolymerProver.t.sol`, copied from
   `prove_log_line_format.golden`, plus `SVM_PROGRAM_KEY` and the non-mainnet `CHAIN_ID`
   behind `SVM_GOLDEN_DEST_CHAIN_ID`), and nothing mechanically enforces the pair: any
   change to `prove_log_line` (separator, field order or widths, prefix) or a golden
   regeneration must update those eco-routes constants in the same release, or both
   suites stay green while a real proof reverts on chain. A cross-repo drift check is a
   tracked follow-up (preferred home: eco-routes CI, checking out this repo at a pinned
   ref and `grep -F`ing the golden into the test file; a read-only cross-repo token is the
   only prerequisite).
6. Out of scope, tracked separately: eco-solver work to request proofs from Polymer's API
   (`polymer_requestProof` / `polymer_queryProof`), run `create_accounts` and `load_proof`,
   and call `validate`; and the equivalent EVM-side relaying of Solana log proofs.

## 8. Decisions and alternatives

- **Hand-rolled Polymer CPI** over a git dependency on `polymer-prover` with `features =
  ["cpi"]`. Avoids an unpublished dependency that pulls `libsecp256k1`, `sha2` and `sha3`
  into our build graph and pins Anchor 0.31.1 against our 1.1.2. Follows the `hyperlane.rs`
  precedent.
- **Caller is the Polymer authority** over proxying `load_proof` through a program PDA.
  Per-relayer cache accounts avoid interleaving between concurrent relayers, and Polymer
  already gives each signer its own pair.
- **One log line per intent** over packing several pairs per line. Keeps every line under
  Polymer's size guidance, keeps the EVM parser stateless, and lets a batch of any size
  ride on `logMessages`.
- **Relayer pays Proof rent, `withdraw` payer reclaims it** over a `pda_payer` pool. The
  relayer is expected to be the solver, who also withdraws, so incentives align.
- **Idempotent re-validation with disagreement as error**, matching the hyper-prover
  `handle` rule adopted in #82, rather than hyper-prover's older fail-on-exists behaviour.
