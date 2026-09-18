# Intent chainer: nested amount and vault templates

SVM semantic counterpart of [eco-routes PR #438](https://github.com/eco/eco-routes/pull/438),
pinned to **`f8572a1971a7302d719878281bd3556fcb183463`**. Equivalent supported inputs
produce the same remote route/reward bytes, hashes and recipients. The Order encoding,
numeric domain, transaction mechanics and resource limits intentionally differ.

## What moves, and what is only data

`chain` measures the order-specific local escrow ATA, renders a child route, fills its
local reward amount, validates the supplied local vault/ATA, transfers the measured
tokens there, and optionally calls the committed local Portal's `publish`.

A template can contain an amount or a downstream vault recipient. For example, a later
EVM route can contain a complete CCTP `depositForBurn(amount, domain, mintRecipient, token)`
whose amount AND mintRecipient depend on the initial measurement. The downstream
recipient is inserted into route bytes. **It is never a local transfer target.**
There is no bridge adapter, CCTP execution in `chain`, or additional escrow layer.

```text
intent1 fulfillment → per-order SVM escrow ATA
                              │
                    separate chain transaction
                              ├─ measure Input and derive Output once
                              ├─ resolve remote recipients into route DATA
                              └─ transfer Input → LOCAL child vault ATA
                                                     │
                                      later child fulfillment executes its route
```

Solana accounts must be declared before execution. The caller reads the balance and
derives the local child accounts; the program remeasures and checks them. A stale list
fails atomically, leaving custody unchanged. Remote address rendering does not remove
this scheduling constraint. `portal → chain → portal::publish` is also prohibited by
SVM reentrancy rules. This is **not** EVM's atomic/no-balance-at-rest architecture.

## Templates and amounts

```text
Program { vaults: [Vault, ...], route: Template }
Vault   { destination, route: Template, reward: Template, derivation }
Template { segments: [bytes, ...], items: [Item, ...] }

render = segments[0] || render(items[0]) || segments[1] || ...
segments.len == items.len + 1
```

Vault nodes are dependency-first. Node i may reference only nodes below i; the root may
reference any existing node. This excludes self/forward/missing references and cycles,
including inside unused nodes. Each node is evaluated once; multiple references reuse
its 32-byte recipient.

The amount context never changes during nesting:

| Value | Meaning |
| --- | --- |
| Input | Initial on-chain measured `amount_in: u64`; funds the local reward |
| Output | `ceil(Input * order.scale / 1e18): u128` |
| Amount item | `ceil(selected_context_value * item.scale / 1e18)` |

Both scales are explicit positive u128 WAD values. `Amount::output(width, little_endian)`
is a builder for Output with an identity additional scale. Input items do not implicitly
apply `order.scale`. Nested Output items do not compound ancestor scales.

Multiplication uses a full 256-bit intermediate with exact division and ceiling
rounding, then checks that the result fits u128. It does not narrow Output to u64 and
does not falsely overflow merely because the intermediate product exceeds u128.
A 6→18 decimal conversion (`scale=1e30`) is supported when the final value fits.

An item encodes 1..32 bytes, either endian, zero-padding when wider than its value.
Widths, scales, overflow and fit are checked; no truncation. Unlike EVM uint256,
**SVM scales and every scaled result are limited to u128** (Input remains u64).
A 32-byte encoding does not enlarge this arithmetic domain. Unsupported values fail.

Equivalent old amount-only outputs are obtained with no nodes and Output/identity items.
There is no legacy Order entrypoint, hash, or escrow-address compatibility.

## Remote hashes and recipients

For each node, render its route and its **entire reward serialization**, then:

```text
intent_hash = keccak256(
    uint64_be(destination) ||
    keccak256(rendered_route) ||
    keccak256(rendered_reward)
)
```

`destination` is the downstream route-execution chain, not necessarily the chain hosting
its reward vault. Remote bytes are opaque to this chainer; the author must use the
target protocol's encoding:

- EVM: complete `abi.encode(Route)` / `abi.encode(Reward)`, including tuple offsets,
  heads, array lengths and tails. Neither Borsh nor hashing just the amount is equivalent.
- SVM: the actual Portal Borsh Route / Reward serialization.

Template validation cannot attest that an opaque route is executable, its reward
describes what the bridge delivers, or its deadlines/prover/token identifiers are correct.

### EVM and TRON

The EVM-tagged configuration explicitly supplies portal/deployer (20 bytes), prefix
(one byte), implementation (20 bytes), and init-code hash (32 bytes). Every parameter
must be nonzero.

```text
recipient = zero_pad_left_32(
    last20(keccak256(prefix || portal_20 || intent_hash || init_code_hash))
)
```

Use `0xff` for standard EVM CREATE2 and `0x41` for TRON. TRON must also supply the
remote **VaultTron** implementation and its corresponding init-code hash, not just a
different prefix. Worldchain works by supplying its different Portal; no chain-ID
special cases exist.

The init-code hash is the remote deployment's
`keccak256(remote Proxy.creationCode || abi.encode(remote implementation))`.
Never assume local Portal, implementation, Proxy bytecode or a universal hash.
Implementation presence is checked, but this calculation **cannot attest its relationship
to the supplied init-code hash or deployed code**. The author must verify that pair.
The implementation field is still committed even though the address calculation consumes
the already-computed init-code hash.

### Solana

The Solana-tagged configuration explicitly supplies nonzero Portal program, token program
and mint:

```text
vault     = canonical_PDA([b"vault", intent_hash], portal_program)
recipient = canonical_ATA(vault, token_program, mint)
```

The recipient is the full **ATA**, not the vault PDA. Native Solana PDA/ATA facilities
select canonical bumps on every render. No bump is supplied, cached as a constant,
or overridden by callers; runtime-dependent hashes can change both bumps.

The pinned EVM implementation limits its manual search to 32 attempts (255..224).
SVM delegates to the native canonical search rather than importing MODEXP/field
arithmetic or that EVM-specific budget (the SDK search considers 255..1).
Consequently a canonical result beyond EVM's search budget can succeed on SVM where
that EVM implementation rejects it. PDA search compute is input-dependent.

## Exact Borsh schema

All scalars and vector lengths below are **little-endian**. Pubkeys/address arrays are
raw bytes. Bool is one byte, 0 or 1. Enum tags are one byte; unknown tags are rejected.
There are no opaque config blobs, padding/default fields, or bump fields.

```text
Order =
  portal[32] | base_mint[32] | destination:u64 |
  TemplateProgram | Reward | scale:u128 | min_amount_in:u64 | require_publish:bool

TemplateProgram = vault_count:u32 | Vault[vault_count] | Template(root)
Vault = destination:u64 | Template(route) | Template(reward) | Derivation
Template = segment_count:u32 | (byte_count:u32 | bytes)[segment_count] |
           item_count:u32 | Item[item_count]

Item tag 0 = AmountSource:u8 | scale:u128 | width:u8 | little_endian:bool
Item tag 1 = vault_index:u8
AmountSource tag 0 = Input
AmountSource tag 1 = Output

Derivation tag 0 = portal[20] | prefix:u8 | implementation[20] | init_code_hash[32]
Derivation tag 1 = portal_program[32] | token_program[32] | mint[32]

Reward = deadline:u64 | creator[32] | prover[32] | native_amount:u64 |
         token_count:u32 | (mint[32] | amount:u64)[token_count]

chain arguments = Order | publish:bool
announce_order arguments = Order
init_order_buffer arguments = seed[32] | order_commitment[32] | order_len:u32 |
                              chunk_len:u32 | chunk_bytes
write_order_buffer arguments = offset:u32 | chunk_len:u32 | chunk_bytes
seal_order_buffer arguments = (none)
chain_from_account arguments = publish:bool
close_order_buffer arguments = (none)
```

These arguments follow their Anchor instruction discriminator. Standalone Borsh decoders
must consume exactly the expected schema (`try_from_slice` rejects trailing bytes).
Appending bumps is not a configuration mechanism.

`Order.hash() = keccak256(borsh(Order))`, streamed without a serialization copy.
Every nested literal, item kind/source/scale/width/endian, reference, node destination,
reward byte and remote configuration is committed. Mutating any of them changes the
order hash and `escrow_authority_pda(order_hash)`.

## Custody, publication and retry boundaries

- Escrow authority remains `PDA([b"escrow", Order.hash()], chainer_program)`.
  Intent1 commits to its escrow ATA as the output recipient. Presenting another order
  cannot authorize spending that escrow.
- `Order.portal` remains a committed **local** reward Portal. One chainer can serve
  multiple Portal deployments. It is distinct from Portals in remote configurations.
- The local reward must have exactly one base-mint token authored at amount zero and
  native amount zero. The runtime measured Input fills that token amount.
- Callers may strengthen publication, never weaken it:
  `effective_publish = order.require_publish || call.publish`.
- Local account checks and the WithdrawnMarker check apply regardless of publication.
  Existing vault balances of any size are allowed. `PushShortfall` requires the vault's
  balance increase to equal the measured input; existing funds cannot mask a transfer
  fee. A short transfer rolls back the escrow debit, vault credit, withheld fees and
  any new ATA. Portal withdraw/refund behavior is unchanged; this does not add general
  Token-2022 extension/transfer-hook support.
- A failed transaction rolls back its transfer and ATA creation, not intent1's earlier
  fulfillment. A retry retains the exact committed order and recomputes the account list.
- There is **no chainer deadline/buffer gate**. Builders budget deadline headroom before
  funding intent1. After funding, solvers should still forward to the local vault even
  near/after expiry, so Portal can refund an unproven intent or pay its proven claimant.
  A valid proof blocks refund until withdrawal. All other chainer checks still apply.
- Invalid amount widths, an unmet minimum, an unusable remote template or an existing
  withdrawn child are not repaired by an announcement. There is no generic
  chainer refund or arbitrary sweep. Keeping the preimage is necessary, not a guarantee
  that every permanently invalid funded order can recover.

### Child identity belongs to the builder

Use a fresh child route salt for every independent execution and coherent token-leg/call
amounts. Uniqueness is required for each nested child as well as the root. Different
parents or Order commitments do not imply different children: changing only minimum input
can change the escrow while leaving the rendered child intent unchanged.

Reusing the child's salt, destination, all route/reward bytes and measured amount derives
the same intent hash and vault. Funding it again funds that same intent; it does not buy
another fulfillment. A vault balance cannot prove freshness, and legitimate prefunding
can have any size, so the chainer imposes no empty-vault or already-funded-vault policy.
The reserved `VaultAlreadyFunded` error number remains for ABI compatibility but is never
emitted. `PushShortfall` checks the current transfer independently of existing funds.

Portal's refund does not write a terminal marker. A delayed proof for a refunded hash
still refers to that exact child; re-funding the same hash can pay its original claimant.
Builders must not treat a refund as permission to recycle a child salt. A fresh salt
derives a different vault and proof PDA. The chainer keeps the WithdrawnMarker check,
but adds no consumption registry or settlement nonce. Repeating `chain` after draining
the escrow fails with `ZeroAmount`; putting new tokens into that escrow is a separate
funding action, not replay of the parent's fulfillment.

A live escrow balance can change before execution; custody/account checks remain
authoritative, and callers must rederive the child accounts when it does.

## Durable announcements and events

Announce the complete order **before funding intent1**, or retain it durably elsewhere.
Losing an unannounced preimage leaves no way to reconstruct it from the escrow address.

`announce_order` validates the complete static preimage and records `OrderAnnounced`
as an **Anchor self-CPI event**, not an ordinary `Program data:` log. It requires two
read-only accounts: the chainer's `PDA([b"__event_authority"])` and the chainer program.
The event authority signs the self-CPI. Its instruction data is:

```text
Anchor EVENT_IX_TAG_LE[8] | OrderAnnounced discriminator[8] |
order_commitment[32] | escrow_authority[32] | Borsh Order
```

At the 4096-byte Order cap that is 4176 bytes. One exactly pre-sized buffer avoids the
default event macro's repeated allocations on the non-freeing heap. Failure to record
the CPI event fails the instruction; exhausting ordinary logs does not lose the preimage.

Indexers must read successful transactions' inner instructions, verify the emitting
chainer program/event framing, and validate the commitment. A logs-only subscription
is insufficient. Persist finalized preimages; RPC historical retention is an operational
dependency, not perpetual storage furnished by this program.

`IntentChained` retains Input, Output, root route hash, local vault, order commitment and
effective publication flag. Reconstruct using **both** initial amounts. `IntentPublished`
is still Portal's ordinary full-route event; run the tested dedicated chain transaction,
not an arbitrary log-heavy bundle. Its byte-exact visibility is tested at the root cap.

## Bounds, resource measurements and transport

### Native staged-order path

Use **`chain_from_account`** when the inline Order does not fit. This adds transport
state, not custody state. The existing `chain` and `announce_order` remain available
for small orders; Order bytes, hashes and escrow derivation are unchanged by staging.
Merely making the existing one-shot `announce_order(Order)` persist would still send
the oversized preimage. Chunked upload plus a small seal/announcement handles both
prepare and execution, without a generic external execution adapter.

```text
authority A: init(first chunk) → append chunks as needed → seal + announce
                                                               │ read-only
any payer B:                                     chain_from_account
authority A: close separately, in any state → rent returned to A
```

The buffer PDA is `PDA([b"order_buffer", authority[32], seed[32]], chainer_program)`.
Generate a fresh random 32-byte seed per trade; do not use a shared index/default seed.
Authority A signs and pays buffer rent on initialization. No separate rent recipient
is accepted: only A can write, seal, or close, and all closing lamports return to A.
Chain's existing payer B pays transaction fees/local vault ATA rent; B can differ from A.
Execution has **no buffer-authority account/signature**, no buffer signer seeds, no
arbitrary execution target, and no combined execute-and-close instruction.

| Instruction | Accounts (in order) | Behavior |
| --- | --- | --- |
| `init_order_buffer` | authority signer/writable, buffer writable, System | Validate 1..4096-byte length and <=800-byte first chunk; griefing-resistant allocation; bind authority, seed and declared commitment |
| `write_order_buffer` | authority signer, buffer writable | Append 1..800 bytes at exactly `written`, within declared length; no overwrite or gaps |
| `seal_order_buffer` | authority signer, buffer writable, event authority, chainer program | Require complete bytes; exact bounded Borsh decode; shared `validate_order`; hash match; full OrderAnnounced self-CPI; seal atomically |
| `chain_from_account` | buffer read-only, then all existing Chain accounts | Require sealed, exact decode; invoke the SAME chain handler directly, recomputing hash and checking both buffer commitment and supplied escrow before value movement |
| `close_order_buffer` | authority signer/writable, buffer writable | Close in any state; return rent to original authority; no token movement |

All buffer consumers check program ownership, discriminator and its authority/seed PDA.
The immutable fixed Borsh header (except `written`/`sealed` during upload) is followed
by raw payload bytes, **not another Vec length**:

```text
OrderBuffer discriminator[8] = [150,173,11,86,146,96,229,77]
authority[32] | seed[32] | order_commitment[32] |
order_len:u32 | written:u32 | sealed:bool | canonical_bump:u8
raw Borsh Order[order_len]    // payload starts at byte 114
```

The IDL account type describes the fixed header; fetch raw account data to access the
tail. Instruction/account discriminators and nested Chain account groups are in the
generated IDL. The bump is chosen by initialization, never accepted as an argument.
This transport bump is unrelated to remote vault derivation, which still accepts none.

Sealed buffers cannot be rewritten, including by A. Before sealing, even A can only
append; retry upload by reading `written`, not resending an already committed chunk.
A bad/incomplete upload can always be closed and recreated. Failed seal/publication
does not seal it or prevent rent recovery. Keep the Order preimage independently and
wait for successful seal/announcement before funding intent1.

Closing can race execution and cause an account-not-found failure; it cannot move
escrow funds. A may recreate the same PDA, but different bytes change Order.hash()
and fail against the original escrow. Anyone can stage identical bytes in another
buffer and retry; a third party staging different bytes cannot authorize that escrow.
Closing cannot erase the recorded announcement or the escrow's commitment.

Buffers are reusable transport, **not single-use settlement records**. Replaying after
a drain fails on the empty escrow. Replaying against a different order's new escrow
fails its commitment check. Re-funding the same order still follows the existing
measurement, transfer-delivery and WithdrawnMarker rules; staging adds no new settlement nonce.
Every execution revalidates templates/configs/rewards, remeasures, checks local accounts,
and applies publication strengthening. There is **no deadline gate**, including at seal.
A stale account list or failed transfer/publication leaves both buffer and escrow intact.

### Packet sizes and client integration

The 781-byte transport fixture has a nested Solana vault, complete Portal Borsh
route/reward bodies, and a root call with runtime amount/recipient. It is a
production-shaped fixture, not the solver's exact captured quote. Tests sign and
submit these legacy packets with a blockhash, CU limit **and CU price**, no ALT:

| Transaction | Wire bytes |
| --- | ---: |
| Inline announce + escrow ATA (not submitted) | 1247 — too large |
| Inline chain (not submitted) | 1376 — too large |
| Prepare 1: init + all 781 Order bytes | **1150** |
| Prepare 2: seal/announce + idempotent escrow ATA creation | **499** |
| Execute: chain_from_account | **627** |
| Close separately | 263 |

Prepare is explicitly **two transactions**, not one. The small differences from a
solver's 1254/1383-byte envelopes depend on framing; clients must still size their own
final signed transactions. No stage packet carries an authority signature into execution.
For a 4096-byte Order, upload is init with 800 bytes plus five append transactions;
then seal/announce and execute. Init at 800 bytes is 1169 bytes, each full append
1072 bytes, standalone seal 297 bytes, execution still 627 bytes. Every submitted
native-path test packet is checked against 1232 bytes.

Client changes (solver implementation remains out of scope):

1. Use the new chainer artifact/IDL; compute the SAME Order commitment and escrow.
2. Generate and retain a random seed, stage through the authority's wallet, then seal
   (optionally bundled with escrow ATA creation). Persist the full announcement.
3. After intent1 settles, read the escrow balance and derive the existing local child
   accounts. Send `chain_from_account(publish)` with read-only buffer followed by the
   existing Chain account list. The buffer authority is absent; the rent payer signs.
4. Retry unchanged Orders with freshly derived local accounts when needed. Close the
   buffer separately through its authority; retain preimages for restaging/recovery.
5. Keep quote-time packet/CU admission checks. Staging Order data does not stage later
   fulfillment calls, compress account keys, or remove SVM scheduling constraints.

### Runtime bounds

| Bound | SVM |
| --- | --- |
| Vault nodes | 8 |
| Items per template | 8 (up to 136 over 17 templates) |
| Aggregate rendered node routes + rewards + root | 2048 bytes |
| Root route | 2048 bytes, within the aggregate bound |
| Borsh Order preimage | 4096 bytes |

Do not substitute EVM's 32 KiB rendered cap. Deserialization bounds vector counts and
segment lengths before allocation and limits the Order reader to 4096 bytes. Shape,
configuration, references and aggregate output are validated before hashing. Recipient
results use a fixed array; downstream templates feed borrowed segments and stack-encoded
items into native scatter/gather Keccak, without allocating route/reward bodies. The
final root is pre-sized once. Success-path arithmetic
does not allocate discarded Anchor errors. The stock allocator remains 32 KiB; no custom
heap or heap-frame request is required.

Fresh SBF/LiteSVM measurements (Anchor 1.1.2, platform-tools v1.52 / rustc 1.89.0-dev,
LiteSVM 0.16.0; instrumented heap peaks before the metrics log):

| Case | Order bytes | Root bytes | Chain CU (observed) | Chainer heap peak |
| --- | ---: | ---: | ---: | ---: |
| EVM / Worldchain / TRON CCTP route, one nested node | 1845 | 928 | 561–590k | 8217 |
| Solana remote vault, Borsh reward | 1728 | 928 | 545–562k | 8081 |
| Three levels, shared references, mixed VMs | 1299 | 109 | 352k | 4525 |
| Eight nodes, eight items in every template, both byte caps | 4096 | 21 | 944k | 11141 |
| Eight Solana nodes, near-maximum root | 3317 | 2032 | 1.083M | 14921 |
| Maximum Order announcement (separate invocation) | 4096 | — | 532k | 13106 |

PDA bumps make CU vary; these are measurements, not an upper bound for every possible
configuration. Request up to 1.4M CU for dense cases and simulate the exact submission.
The tests also cover root-only 2048-byte publication and deliberately exhausted logs
before a full 4096-byte announcement.

**Runtime acceptance is not packet transport.** The earlier claim that a ~960-byte route
fits one ordinary chain transaction was incorrect: LiteSVM can execute oversized
transactions. Tests now count actual wire bytes and submit only <=1232-byte packets.

The historical test-only generic buffer remains for inline ABI/CPI/log-pressure
regressions; it is not deployed or required by the native path. Production clients
should use the native path above. An ALT only compresses account keys, not Order data.

Native-path measurements retain the same 4096/2048 limits. A 4 KiB **encoded Order**
does not authorize 4 KiB of rendered output. Native execution borrows the account's
payload and deserializes one bounded Order, with no full-byte staging copy or extra
execution CPI. Measured stock-heap peaks before the optional metrics log:

| Native case | Chain CU (observed) | Chain heap | Seal/announcement heap |
| --- | ---: | ---: | ---: |
| 781-byte nested Order | ~285k | 4125 | 2351 |
| 4096-byte Order, 8 nodes/136 items, 2048 aggregate rendered | ~922–934k | 11261 | 13490 |
| 3317-byte Order, 8 Solana nodes, 2032-byte root | ~1.074–1.076M | 15041 | 8327 |

Sealing the dense 4096-byte case uses about 533k CU and records all 4176 event bytes.
These tests use the real SBF artifact and <=1232-byte native submissions, not host
renderer calls or a generic-buffer execution shortcut. Keep both bounds: there is
measured stock-32-KiB headroom, but no evidence here justifies larger limits. Native
PDA search is input-dependent; simulate exact orders and request an adequate CU limit
(up to 1.4M for dense cases). Do not interpret observations as a universal CU guarantee.

## Build, validation and release

Use CLAUDE.md's pinned host and SBF toolchains. Build fresh artifacts before tests:

```sh
anchor build --program-name intent-chainer --ignore-keys --no-idl
anchor build --program-name portal --ignore-keys --no-idl
anchor build --program-name local-prover --ignore-keys --no-idl
cargo build-sbf --tools-version v1.52 --manifest-path integration-tests/programs/template-buffer/Cargo.toml --sbf-out-dir target/deploy
cargo test --locked -p intent-chainer
cargo test --locked --test chain --test chain_nested --test refund --test withdraw
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt
cargo sort --workspace --check
anchor idl build --program-name intent-chainer --out target/idl/intent_chainer.json --out-ts target/types/intent_chainer.ts
```

`--ignore-keys` avoids rewriting program IDs from unrelated local keypairs. For a full
workspace test, also build all other programs and mock-igp as CLAUDE.md specifies.
Optional local heap profiling adds `-- --features resource-metrics` to the chainer SBF
build; leave that feature disabled for release and rebuild uninstrumented for final tests.

Independent fixtures:

- `solana-vault-vectors.json` is copied verbatim from the pinned EVM corpus: Portal's
  own vault golden, SDK ATA/bump fixtures, and 128 independent PDA vectors.
- `nested-vectors.json` has full ABI/CCTP routes and complete ABI or Borsh rewards;
  ethers/Solana SDK expectations were checked against the pinned Solidity harness on
  local Anvil. `generate-nested-vectors.cjs` documents regeneration using a fresh local
  Anvil instance. Neither generator calls a live chain.
- The existing amount-only cross-VM output fixture is retained. The Order-hash golden
  changes intentionally and was independently checked with manual Borsh + ethers Keccak.

Redeploy the chainer under a **new program ID**; do not upgrade in place. Regenerate IDL
and TS types, migrate all Order builders/encoders/commitment derivation, add CPI-event
indexing and packet-checked staging, and stop authoring old-schema orders for the new ID.
The new program cannot sign an old chainer's escrow PDAs. Existing old orders stay with
their old program/preimages and recovery process. Respect the repo's coordinated
Portal/prover release policy whenever those programs are released together.
An unfunded pre-release deployment may retain its upgrade authority for testing and be
finalized separately when approved. Retaining authority does not make schema-breaking
upgrades safe for live orders; that authority controls the code authorizing escrow
spends. Never use the old deployed ID for this Order ABI change.
No deployment is part of this change. Separately discovered deployed-code fixes follow
SECURITY.md's private advisory process, never a public feature PR.
