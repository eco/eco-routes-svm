# LayerZero prover for Solana — design

Status: draft for review (2026-10-06)
Counterpart: `eco-routes` `contracts/prover/LayerZeroProver.sol` (LayerZero V2, unchanged by this design)

## 1. Goal

Add a LayerZero V2-backed prover to the Solana deployment, in **both directions**, so it can join the aggregator as a **liveness** member: a third independent bridge next to Hyperlane and Polymer, any one of which can prove an intent (1-of-N union, matching both aggregators today).

- **Solana → EVM (outbound):** an intent published on an EVM chain and fulfilled on Solana is proven by sending `(intent_hash, claimant)` pairs through LayerZero to the EVM `LayerZeroProver`.
- **EVM → Solana (inbound):** an intent published on Solana and fulfilled on an EVM chain is proven when the EVM `LayerZeroProver` sends pairs through LayerZero to this program, which writes standard `Proof` PDAs that `portal::withdraw`, `portal::refund` and `aggregator-prover::aggregate` consume unchanged.

Success: mainnet E2E both ways between Solana and Base, through the new aggregator pair, with every EVM fleet chain that has a LayerZero V2 endpoint configured as a peer.

Non-goals:
- A k-of-N security threshold. LayerZero is an additional liveness path; under a 1-of-N union it also widens the trust surface (its DVN set alone can prove). A threshold aggregator is a separate project.
- LayerZero compose, lzToken fees, or any admin after finalization.

## 2. Background: constraints that shape the design

Sources: `LayerZero-Labs/LayerZero-v2@9c741e7f` (`packages/layerzero-v2/solana/programs/*`), `LayerZero-Labs/devtools@4973ba8b` (`examples/oapp-solana`), LayerZero metadata API, docs. Re-verify every pinned ID against the metadata API at implementation time.

| | Value |
|---|---|
| EndpointV2 | `76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6` (mainnet and devnet) |
| ULN302 (send + receive lib) | `7a4WjyR8VZ7yZz5XJAKm39BUGn5iT9CKcv2pmG9tdXVH` |
| Executor program / config | `6doghB248px58JSSwG4qejQ46kFMW4AMj7vzJnWZHNZn` / `AwrbHeCyniXaQhiJZkLhgWdUCteeWSGaSN1sTfLiY7xK` |
| Pricefeed | `8ahPGPjEbpgGaZx2NV1iG5Shj7TDwvsjkEDcGWjt94TP` |
| Solana EID | `30168` mainnet, `40168` devnet |

### 2.1 CPI depth forbids sending inside `portal::prove`

`endpoint::send` nests OApp → Endpoint → ULN → worker (executor/DVN) → pricefeed: five invoke-stack frames when the OApp instruction is top-level, which is exactly Solana's limit. `portal::prove` → prover would add a sixth. SIMD-0268 (raise to 8, feature `6TkHkRmP7JZy1fdM6fg5uXn76wChQBWGokHBJzrLB3mj`) was **not activated on mainnet or devnet** when checked on 2026-10-06. The outbound path is therefore split into a commit (inside `portal::prove`) and a top-level `send`. If SIMD-0268 activates later, the split stays correct; collapsing it would be a new release.

### 2.2 Inbound delivery is a push, sized by the transaction

LayerZero's executor delivers by calling our `lz_receive` as a top-level instruction, in a transaction it builds from what our `lz_receive_types_v2` returns: `[ComputeBudget] → executor::pre_execute → lz_receive → executor::post_execute`. Each delivered pair needs its `Proof` PDA as a **static** account key (a not-yet-existing per-message address cannot come from a lookup table), so a pair costs ~97 bytes (64 in the message, 32 key, 1 index). With the fixed accounts in an ALT, the 1232-byte packet fits **~8 pairs per message** (estimate; pinned by measurement, §6).

### 2.3 Endpoint receive semantics we rely on

- `lz_receive` is permissionless once DVNs verified the message. Replay protection is `endpoint::clear`, which checks `keccak(guid ‖ message)` against the stored `PayloadHash` and closes it. `clear` must run **before** any state change.
- The endpoint does **not** check the peer; `lz_receive` must require `sender == peer[src_eid]`.
- Ordering is lazy: `clear` accepts any `nonce <= inbound_nonce` (the contiguous verified prefix). A message whose `lz_receive` fails (e.g. a conflicting `Proof`) stays verified and does not block later nonces.
- Rent for accounts created in `lz_receive` must not come from the executor's payer: the EVM `LayerZeroProver` options carry gas only (no lamport `value`), and `post_execute` bounds the payer's lamport loss. Rent therefore comes from our own reserve (§3.1 `pda_payer`), the same pattern as `hyper-prover`.

### 2.4 Path setup is delegate-gated

`register_oapp` sets a delegate; `init_nonce` (the `allowInitializePath` equivalent, required before any send or verification on a path), `init_send_library`, `init_receive_library`, `init_config`, `set_send_library`, `set_receive_library` and `set_config` all require the delegate (or the OApp PDA) to sign, and the `init_*` ones make the delegate **pay** via Anchor `init`. A data-holding PDA cannot pay; the delegate must be a system-owned account. Unpinned paths follow `DEFAULT_MESSAGE_LIB` and LayerZero's default ULN config, i.e. LayerZero governance would control our DVN security.

## 3. Solana program: `programs/layerzero-prover`

Anchor 1.1.2, vanity ID `EcoZ…` (ground before EVM deployment: the EVM whitelist needs the derived `Store` PDA, §5). No LayerZero crate dependency: `layerzero.rs` hand-mirrors program IDs, PDA seeds, discriminators and param structs the way `hyperlane.rs` and `polymer.rs` do, feature-gated on `mainnet` where they differ (EID). Depends on `eco-svm-std` and `portal` (`no-entrypoint`) like `hyper-prover`.

### 3.1 Layout and state

```
programs/layerzero-prover/src/
  lib.rs              # declare_id!, #[program]
  layerzero.rs        # mirrored LZ IDs, seeds, discriminators, ClearParams/SendParams/QuoteParams, options encoding
  state.rs            # Store, LzReceiveTypesAccount, PendingSend, ProofAccount, PDAs
  instructions/       # one file per instruction (below)
```

| PDA | Seeds | Contents / role |
|---|---|---|
| `Store` | `["Store"]` | The OApp address registered with the endpoint; signs `send`, `clear`, `register_oapp`. Holds `peers: Vec<Peer>` (≤ `MAX_PEERS` = 16), `alt: Pubkey` (the pinned lookup table returned to the executor), bumps. |
| `LzReceiveTypes` | `["LzReceiveTypes", store]` | Required by the V2 executor at fixed seeds; holds `store`. |
| `pda_payer` | `["pda_payer"]` | System-owned lamport reserve. Pays `Proof` rent in `lz_receive`; `close_proof` refunds to it. **Also the LayerZero delegate** (§2.4: must sign and pay). |
| `PendingSend` | `["pending_send", keccak(dst_eid_le ‖ receiver ‖ payload)]` | Outbound commit: `{ dst_eid: u32, receiver: Bytes32, payload: Vec<u8>, rent_payer: Pubkey }`. |
| `Proof` | `eco_svm_std::prover::Proof::pda(intent_hash, ID)` | Standard `ProofAccount(Proof { destination, claimant })`, byte-compatible with `hyper-prover`'s. |

```rust
pub struct Peer {
    pub eid: u32,          // LayerZero endpoint ID of the EVM chain
    pub address: Bytes32,  // that chain's EVM LayerZeroProver, left-padded
    pub chain_id: u64,     // the chain's EVM chain ID (the Proof.destination it may claim)
}
```

`peers` is the inbound sender whitelist, the `src_eid → chain_id` map, and the set of paths initialized at setup. Invariants enforced at `init`: non-empty, ≤ 16, unique `eid`, unique `chain_id`, nonzero fields (mirrors `MessageBridgeProver`'s domain-config validation).

### 3.2 Admin model: pin, then finalize

Setup instructions (`init`, `init_path`, `set_alt`) are gated on the program's upgrade authority, exactly as `aggregator-prover::init` (`program.programdata_address() == program_data`, `program_data.upgrade_authority_address == Some(authority)`). No human key is ever the LayerZero delegate: the delegate is `pda_payer`, which only this program can sign for, and no post-setup instruction uses it. **Finalizing the program (upgrade authority → none) is the revocation**: every setup instruction becomes permanently uncallable, so no separate revoke step can be forgotten. Consequence: a path not fully pinned before finalization is dead and requires a new program ID; the deploy checklist (§7) reads back every path before finalizing.

### 3.3 Setup instructions

- **`init(InitArgs { peers })`** — creates `Store` and `LzReceiveTypes` (griefing-resistant `create_account`), validates peers (§3.1), CPIs `endpoint::register_oapp(delegate = pda_payer)` signed by `Store`. `pda_payer` must be pre-funded (deploy step).
- **`init_path(eid, PathConfig)`** — for one configured peer: CPIs `init_nonce(remote_oapp = peer.address)`, `init_send_library`, `init_receive_library`, `set_send_library(ULN302)`, `set_receive_library(ULN302, grace 0)`, `init_config` (ULN send + receive), then `set_config` for the ULN send config (DVNs, confirmations), executor config (executor, max message size), and ULN receive config (DVNs, confirmations). All signed by `pda_payer` as delegate. `PathConfig` carries the explicit DVN set, thresholds, confirmations and executor; nothing is left on LayerZero defaults. May be split into `init_path` + `set_path_config` if one transaction exceeds CU or size; the split does not change semantics.
- **`set_alt(alt)`** — records the lookup table the executor uses for `lz_receive`'s static accounts (§3.5). The ALT itself must be frozen (authority removed) before finalization.

### 3.4 `prove(ProveArgs)` — outbound commit

Called only through `portal::prove` (`portal_dispatcher` must equal `portal::state::dispatcher_pda(&crate::ID)` and sign, as in every prover). Accounts: `portal_dispatcher` (signer), `payer` (signer, mut, forwarded by portal), `store`, `pending_send` (mut), `system_program`.

- `domain_id` is the destination LayerZero EID (as on EVM, where `domainID` is used directly as `dstEid`); must be ≤ `u32::MAX` and a configured peer's `eid` (`UnknownPeer`).
- `data` is exactly 32 bytes: the receiving EVM `LayerZeroProver` (`InvalidData` otherwise). It must equal that peer's `address` (`InvalidReceiver`): a send to any other address could never be received, so failing early is cheaper than a stranded fee.
- `proof_data.intent_hashes_claimants` is non-empty and ≤ `MAX_INTENTS_PER_PROVE` (provisionally 16; final value is the largest batch for which `[portal::prove, layerzero_prover::send]` fits one v0 transaction with ALTs, pinned by a test, §6).
- `payload = proof_data.to_bytes()` (8-byte `CHAIN_ID` header ‖ 64-byte pairs — the exact `encodedProofs` the EVM `_handleCrossChainMessage` parses). Claimants pass through unchanged, including the `CANCELLED` sentinel.
- Creates `PendingSend` with `rent_payer = payer`. If it already exists with identical content, no-op (re-proving a batch that is still pending is idempotent).

### 3.5 `send` — outbound dispatch (top-level, permissionless)

Args: `max_native_fee: u64` (the commit is identified by the `pending_send` account itself). Accounts: `payer` (signer, mut, pays the LayerZero fee), `store`, `pending_send` (mut, closed), `rent_payer` (mut, must equal `pending_send.rent_payer`), the endpoint `send` accounts and the ULN/executor/DVN/pricefeed remaining accounts (passed through, ~34; supplied via ALT by the caller).

- Builds options **on-chain**: type-3, executor `lzReceive` gas = `MIN_GAS_LIMIT (200_000) + n * GAS_PER_INTENT (50_000)`, the same floor the EVM side computes, so a permissionless caller cannot under-gas the EVM `lzReceive`.
- CPIs `endpoint::send(SendParams { dst_eid, receiver, message: payload, options, native_fee: max_native_fee, lz_token_fee: 0 })` signed by `Store`; the endpoint charges the quoted fee from `payer` and fails if `max_native_fee` is short.
- Closes `PendingSend` to `rent_payer`.
- Safe to be permissionless: only portal's dispatcher can create a `PendingSend`, so `send` can only ever transmit a portal-attested batch to its configured peer. A second send of the same batch after re-proving is harmless (the EVM side skips already-proven intents).

**`quote_send`** — read-only twin: same accounts (all read-only), CPIs `endpoint::quote` and returns `MessagingFee` via return data, for `simulateTransaction` fee quoting.

The solver places `portal::prove` and `send` in one v0 transaction when they fit; they may also be separate transactions (the commit persists and `send` is retryable).

### 3.6 Inbound: `lz_receive_types_info`, `lz_receive_types_v2`, `lz_receive`

Implements the executor's V2 interface (V1 `lz_receive_types` is not implemented; executors fall back to V1 only when `_info` is missing).

- **`lz_receive_types_info`** (accounts `[store, lz_receive_types]`): returns `(2, { accounts: [store, …] })`.
- **`lz_receive_types_v2`**: decodes `params.message` as `ProofData` and returns one `LzReceive` instruction whose accounts are: `store`, the endpoint `clear` accounts (endpoint program, `OApp` registry, `Nonce`, `PayloadHash` (mut), `Endpoint` (mut), endpoint event authority), `pda_payer` (mut), `system_program`, our event authority + program (`event_cpi`), then one `Proof` PDA (mut) per pair, in message order. Static accounts are referenced through `store.alt`. It must return exactly what `lz_receive` validates (a mismatch halts the executor).
- **`lz_receive(LzReceiveParams)`**, in order:
  1. CPI `endpoint::clear` signed by `Store` (burns the nonce; must precede state changes).
  2. `peer = store.peers[src_eid]` (`UnknownPeer`); require `sender == peer.address` (`InvalidSender`).
  3. `proof_data = ProofData::from_bytes(message)`; require `proof_data.destination == peer.chain_id` (`ChainIdMismatch`) — the EVM `_handleCrossChainMessage` cross-check, which `hyper-prover::handle` lacks; the endpoint authenticates `src_eid`, so the header cannot claim another chain.
  4. Require `remaining_accounts.len() == pairs.len()` and each equals `Proof::pda(intent_hash, ID)` (`InvalidProof`).
  5. Per pair, `hyper-prover`'s rule: absent → create `ProofAccount(Proof { destination, claimant })` funded and co-signed by `pda_payer`; present and identical → no-op; present and different → `IntentAlreadyProven`. Emit `IntentProven` via `emit_cpi!` either way.

If `pda_payer` lacks rent, `lz_receive` fails and the message stays verified; it is retried after a top-up. An over-cap message (more pairs than fit the delivery transaction) can never execute; the remedy is re-proving the intents from the EVM Inbox in smaller batches (the cap is enforced off-chain, §5).

### 3.7 `close_proof`

Identical to `hyper-prover`: signer must be `portal::state::proof_closer_pda(&crate::ID)`; closes the `ProofAccount` to `pda_payer`. Takes no other dependency, so it cannot fail once the program is finalized — required because `portal::refund` CPIs it on proven cancellations.

### 3.8 Errors

`InvalidPortalDispatcher`, `InvalidPortalProofCloser`, `InvalidAuthority`, `InvalidPeerSet`, `UnknownPeer`, `InvalidReceiver`, `InvalidData`, `InvalidDomainId`, `EmptyProof`, `TooManyIntents`, `InvalidSender`, `ChainIdMismatch`, `InvalidProof`, `IntentAlreadyProven`, `InvalidPdaPayer`, `InvalidPendingSend`, `InvalidRentPayer`, `InvalidEndpoint`.

## 4. Security properties

- **Prover-scoped dispatcher and closer.** `prove` accepts only `dispatcher_pda(&crate::ID)`; `close_proof` only `proof_closer_pda(&crate::ID)` — the load-bearing boundary every prover keeps.
- **Who can create a `Proof`.** Only `lz_receive` after a successful `clear` of a DVN-verified message from a configured peer whose header chain matches the peer's chain. Trust = the pinned DVN set per path (the same trust the EVM `LayerZeroProver` already accepts).
- **No admin after finalization.** Delegate is a program PDA; setup is upgrade-authority-gated; finalization removes both. No `skip`/`nilify`/`burn` exists afterwards: an unverifiable nonce would block later nonces on that path (DVNs verify every message, so this is a liveness risk, not a safety one, and the remedy is a new release).
- **Pinned config.** Every path pins send/receive library and ULN/executor config explicitly; nothing follows `DEFAULT_MESSAGE_LIB`, so LayerZero governance cannot change our DVN set.
- **Permissionless `send`** transmits only portal-attested commits to their configured peer with on-chain options.
- **`pda_payer` is not a value store for users**: it holds only operator-funded rent float; draining it is impossible (only `lz_receive` debits it, one rent-exempt `Proof` per verified pair, refunded on close).
- **Atomic release.** Like every prover, it compiles against portal's ID for `dispatcher_pda`/`proof_closer_pda`. It joins the current portal without a portal redeploy (portal derives per-prover PDAs for any prover); it must be built from the tree that matches the deployed portal.

## 5. Cross-repo touchpoints

**eco-routes (EVM)** — no contract change. A new fleet `LayerZeroProver` deployment (none is live in v2.12):
- `LAYERZERO_CROSS_VM_PROVERS` includes the Solana **`Store` PDA** (the OApp address LayerZero reports as `origin.sender`), not the program ID.
- `LAYERZERO_DOMAIN_CONFIG` includes `30168:1399811149` (mainnet; `40168:1399811150` devnet).
- The EVM endpoint path to Solana is pinned with the matching DVN set (delegate config, then `revokeDelegation()`).
- Plus a new EVM `AggregatorProver` whose members add `LayerZeroProver`.

**eco-routes-svm (this repo)** — new `aggregator-prover` deployment (new program ID) initialized with `[hyper-prover, polymer-prover, layerzero-prover]`; membership is immutable per deployment.

**eco-solver** —
- Solana-source prove: build `[ComputeBudget, portal::prove(prover = EcoZ…, domain = dst EID, data = EVM LayerZeroProver), layerzero_prover::send]` as v0 with ALTs (LayerZero accounts + FulfillMarkers); fee from simulating `quote_send`; batch ≤ `MAX_INTENTS_PER_PROVE`.
- EVM-source prove toward Solana: `Inbox.prove` via `LayerZeroProver` with `domain = 30168`, receiver = `Store` PDA; **batch ≤ `MAX_PAIRS_PER_MESSAGE` (8)** — the EVM contract cannot enforce it.
- Aggregator: add LayerZero as a member to the member listener / aggregate job; `pda_payer` balance alert.

## 6. Testing

**Unit (goldie)** — PDA derivations (`Store`, `LzReceiveTypes`, `pda_payer`, `PendingSend`, endpoint `Nonce`/`PayloadHash`/`OApp`), options encoding byte-for-byte against the EVM `_formatLayerZeroMessage` output for the same `n`, discriminators against `sha256("global:<name>")`, `lz_receive_types_v2` account list.

**Integration (litesvm)** — a localnet-only **`mock-layerzero-endpoint`** at the endpoint ID (like `mock-polymer-prover`) implementing `register_oapp`, the path-setup instructions as recorders, `send`/`quote` (records params, charges a fixed fee), a test-only `verify` that writes a `PayloadHash`, and `clear` with the real checks (hash match, close). Cases:
- `init` validation (peer set rules, authority gate), `init_path` CPIs recorded with the pinned config.
- `prove` via `portal::prove`: dispatcher gate, unknown EID, wrong receiver, empty/over-cap, idempotent re-commit, `CANCELLED` claimant passes through.
- `send`: options floor, closes commit to `rent_payer`, rejects foreign `rent_payer`, permissionless caller.
- `lz_receive`: clear-before-state ordering (a failed clear leaves no proof), unknown peer, wrong sender, chain-id mismatch, account count/address mismatch, idempotent redelivery, conflicting proof, `pda_payer` underfunded then retried, full flow into `aggregator-prover::aggregate` → `portal::withdraw` → `close_proof` rent back to `pda_payer`, and proven-cancellation `refund`.
- `lz_receive_types_v2` output equals the accounts `lz_receive` accepts.
- **Size pins:** largest `[portal::prove, send]` v0 transaction (sets `MAX_INTENTS_PER_PROVE`) and largest delivery transaction shaped as the executor builds it (sets `MAX_PAIRS_PER_MESSAGE`), each with a one-over failure.

**Real endpoint** — one litesvm test loading the endpoint and ULN programs dumped from devnet (`solana program dump`) to prove `register_oapp`/`init_path`/`clear` against the real code, not only the mock.

**Devnet E2E** — Solana devnet (EID 40168) ↔ Base Sepolia, both directions, real DVNs/executor; confirms: executor delivers with gas-only options (no lamport `value`), the gas floor is honored as CU, the delivery cap of 8, and fee quoting.

## 7. Rollout

1. Grind `EcoZ…`; derive `Store`; deploy EVM fleet `LayerZeroProver` (CREATE2, vanity `0xEC0…`) with the Solana `Store` whitelisted and `30168` in the domain config.
2. Deploy `layerzero-prover` (verifiable build); fund `pda_payer`; `init(peers)`; create and freeze the ALT; `set_alt`; `init_path` per peer.
3. **Read back** every path (Nonce, send/receive library, ULN send/receive config, executor config) against the plan; only then finalize. Upload IDL; OtterSec verify.
4. Pin the EVM side's Solana path config and `revokeDelegation()`.
5. Deploy the new aggregator pair (Solana program + EVM `AggregatorProver`) with LayerZero as a member.
6. Mainnet E2E both ways (Solana ↔ Base) through the new aggregators; then solver cut-over.

## 8. Decisions and alternatives

- **Record-then-send** over sending inside `portal::prove` — forced by CPI depth (§2.1).
- **Direct inbound** (create proofs in `lz_receive`, cap 8) over buffering the batch and fanning out later — buffering only raises the cap to ~12 (message bytes still ride in the delivery tx) for a second relayer step and another account type.
- **Portal stays the batch source** over a `send` that reads `FulfillMarker`s itself — portal's `fulfillment_claimant` owns the fulfilled/cancelled semantics; duplicating it would drift.
- **`pda_payer` as delegate and rent reserve** — the endpoint requires a signing, paying, system-owned delegate; one reserve serves both.
- **Upgrade-authority gating + finalization as revocation** over a separate revoke instruction.
- **V2 executor interface only** — V1 costs a CPI level, has no ALTs and a 1024-byte return cap.

### Open items to confirm during implementation

- The deployed Solana executor runs the V2 `pre_execute`/`post_execute` flow (source seen on a branch).
- Executor accepts gas-only options toward Solana; if it requires a lamport `value`, the EVM `LayerZeroProver` would need an options change (an EVM release).
- Exact `MAX_INTENTS_PER_PROVE` and `MAX_PAIRS_PER_MESSAGE` from the size tests.
- Endpoint `clear`/`init_*` account lists re-read from the endpoint source at a pinned tag.
