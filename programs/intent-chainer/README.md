# Intent Chaining

`intent-chainer` publishes an intent whose amount does not exist until another intent executes.

It is the Solana counterpart of the EVM [`IntentChainer`](https://github.com/eco/eco-routes/blob/feat/intent-chainer/contracts/chain/IntentChainer.sol)
(eco-routes#438). The encoding, the economics and the validation set are deliberately identical. The
**shape** is not, and the reason is worth reading before changing anything here.

## The problem

An intent's reward amount is part of `Reward`, which is part of the intent hash, which is the seed of its
vault PDA. So an intent whose amount is only known at execution time cannot be built, hashed, or funded
ahead of time — the vault address itself moves with the amount.

That blocks any flow where one intent's output feeds another's input. The motivating case is a swap
expressed as two intents:

```
Solana                                           Solana / Base
─────────────────────────────────────            ─────────────────────
intent1  (same-chain, SVM → SVM)
  route.tokens  [WSOL, amountIn]
  route.calls
    [0] swap(WSOL → USDC, → escrow ATA)   ── produces an amount nobody knew in advance
                           │
chain(order)  ─────────────┤   (its own transaction)
                           ├─ measures the escrow's USDC balance      = amountIn
                           ├─ splices ceil(amountIn * scale / WAD) into the route
                           ├─ pushes amountIn into intent2's vault
                           └─ optionally CPIs portal::publish
                                                 │
intent2  (SVM → SVM, or SVM → Base)              ▼
  reward.tokens [USDC, amountIn]     solver delivers the scaled amount on the destination,
  route         amountIn * scale     whose calls pay the user
```

The user signs and funds only intent1.

## Why this is a separate transaction, and the EVM one is not

The EVM chainer runs *inside* intent1's fulfillment, as its last `Call`: it measures, calls
`Portal.publish`, and pushes into the vault address that call returns — atomically. That atomicity is the
source of its best property, that the intended flow never leaves a balance at rest, which is in turn why it
needs no access control and no per-order state.

Solana cannot host that shape. Two independent reasons, and the first is decisive:

- **Accounts are declared up front.** Intent2's vault, and its ATA, are PDAs of the intent hash, which
  depends on the amount. A transaction must name every account it touches before it runs, so the account the
  push targets cannot be known inside the transaction that discovers the amount. On EVM an address is just a
  value returned from a call; here it is a scheduling input.
- **Reentrancy.** Even setting the accounts aside, `portal::fulfill → chain → portal::publish` puts portal on
  the instruction stack twice, and the runtime rejects it with `ReentrancyNotAllowed`. This is the same rule
  that makes `flash-fulfiller` a separate program. `chain_cannot_publish_from_inside_a_route_call` pins it.

A third constraint bounds how far the EVM shape could be pushed even if those were solved: intent2's route
has to be carried somewhere, and a realistic `abi.encode(Route)` for a two-call EVM swap is ~960 bytes
against the ~500 that remain inside intent1's own 1232-byte transaction.

So `chain` is its own permissionless transaction. The caller reads the escrow balance, derives intent2's
vault from it, and submits; the program re-measures on-chain and refuses to proceed unless the accounts it
was handed match what the measurement implies. The measurement stays authoritative — a caller cannot declare
an amount that is not there — but the caller carries the scheduling.

One guarantee is genuinely weaker as a result, and it is worth stating rather than eliding. On EVM a failure
inside `chain` reverts **intent1 whole**: the solver's input is untouched and the swap never happened. Here
intent1 was fulfilled and settled in an earlier transaction and nothing can unwind it. What survives is that
every check runs before any value moves, so a rejected order leaves the measured balance in the escrow,
recoverable by re-running `chain` with a corrected one. Preserving the escrow is the whole of the guarantee.

## What that costs, and how it is paid for

Splitting the transaction means the balance **is** at rest between intent1's fulfillment and the `chain`
call, and `chain` has no signer to gate. A single shared custody account would let whoever calls first sweep
that balance into an order of their own authorship — with `reward.creator` set to themselves and a short
deadline, they refund it out.

The fix is that custody is **seeded by the order's own commitment**:

```rust
escrow_authority_pda(keccak(borsh(order)))
```

intent1's route names that address as its swap recipient, that route is hash-committed into intent1's own
intent hash, and no other order can derive it. Funding the address *is* approval of the order behind it.
This restores the EVM property — "intent1's hash authorizes the order" — transitively rather than directly.

**It is a security boundary.** Collapsing the seeds to anything the order does not determine reopens the
sweep. `a_foreign_order_cannot_reach_another_orders_escrow` and `every_order_field_moves_the_escrow_address`
pin it.

The reward leg's `amount` must be authored as zero, which is what makes the commitment preimage canonical:
otherwise two orders differing only in a field the program overwrites would be the same intent with two
different escrows.

## Two amounts, one measurement

| value                            | goes to                   | meaning                                                |
| -------------------------------- | ------------------------- | ------------------------------------------------------ |
| `amount_in`                      | `reward.tokens[0].amount` | escrowed on the source; what intent2's solver collects |
| `ceil(amount_in * scale / WAD)`  | every route slot          | what that solver must deliver on the destination       |

One committed number, `scale`, does the whole source-to-destination transform. The reward leg escrows the
full measured `amount_in` while the route obliges only the scaled amount, so the gap between them is
intent2's solver's entire margin.

### Units are not the same across chains

Part of `scale` is a unit conversion: "the same token" is not the same unit everywhere. USDC is 6 decimals
on Solana and Base, but Binance-Peg USDC on BNB Chain is 18.

| lane                         | `scale`           |
| ---------------------------- | ----------------- |
| same units, no spread        | `1e18`            |
| same units, less 100bps      | `0.99e18`         |
| 6 → 18 decimals              | `1e30`            |
| 18 → 6 decimals              | `1e6`             |
| 6 → 18 decimals, less 100bps | `1e30 * 99 / 100` |

The denominator is **decimal, not binary**, on purpose. Unit conversions are powers of ten, so a decimal
denominator represents every one of them exactly in both directions; a binary denominator (Q128 and friends)
cannot — `2^128 / 1e12` is not an integer, so a downscaling lane would lean on rounding to recover a value it
should have computed exactly.

Rounding is toward the user (up), because the written value is the solver's delivery **floor**. Ceil rounding
also makes a zero obligation unreachable: with `amount_in >= 1` and a reduced numerator `>= 1` the quotient is
always at least 1, so the program carries no explicit zero-obligation check.

### The gcd reduction is load-bearing on Solana

Solana has no 256-bit integer, and the naive `amount_in * scale` overflows `u128` exactly on the lanes the
unit conversion exists for: a 6→18 lane is `scale = 1e30`, and 1M USDC (`amount_in = 1e12`) gives `1e42`
against a `u128` ceiling of `~3.4e38`.

`scale_amount` therefore reduces the fraction by `gcd(scale, WAD)` before multiplying, which turns that same
lane into `amount_in * 1e12 / 1`. Because unit conversions are powers of ten and `WAD` is `1e18`, the reduced
numerator is small for every lane in the table above. A `scale` pathological enough to overflow anyway fails
loudly as `ScaleOverflow` rather than wrapping. This is the one arithmetic difference from the EVM contract,
which gets 256-bit multiplication for free.

### The spread is proportional, not flat

There is no separate flat fee field, matching the EVM contract after `e354167`. A flat fee and a ratio are
different functions of `amount_in` — flat keeps the solver's take constant as the amount moves, proportional
lets it grow — and a flat one cannot be folded into a ratio. The trade is deliberate:

- **Lost:** pricing destination gas independently of size, which is genuinely fixed.
- **Kept:** everything else, because the only thing that moves `amount_in` here is swap slippage, a percent
  or so around a known expectation, over which the two are indistinguishable.

Use `min_amount_in` to say "too small to be worth filling".

## Slots and segments

The route is opaque bytes; for an EVM destination it is not Borsh at all. Rather than carry the whole blob
plus numeric write offsets, an order carries the literal bytes **around** each amount:

```
route = segments[0] ‖ enc(slots[0]) ‖ segments[1] ‖ … ‖ segments[n]
```

with `segments.len() == slots.len() + 1`. A mis-stated write position is not expressible, and the same
representation serves an EVM destination and a Borsh one without the program knowing which it holds.

Each `Slot` is just geometry — `width` in bytes and `little_endian` — because every slot receives the same
number. A value that does not fit its width reverts; it is never truncated. Silent truncation is the
dangerous case: an amount wrapped into a Solana `u64` would publish an intent2 that is well-formed, fillable,
and pays out a fraction of what was escrowed.

Offsets would be actively wrong here, for two independent reasons:

- Canonical `abi.encode(Route)` puts every argument inside `calls[k].data` at an absolute offset
  `≡ 4 (mod 32)`, because the 4-byte selector shifts the payload — so the 32-byte alignment invariant one
  would naturally assert rejects every real EVM swap route.
- In a Solana route the amount appears twice, and the second position moves with the call's account list
  while the first does not.

### Solana

A Solana route is Borsh, and the amount appears **twice** — once as `route.tokens[0].amount` and again inside
the SPL `transfer_checked` instruction data. Both are 8-byte little-endian `u64`, so amounts above
`u64::MAX` are rejected. For the shape `DepositAddress_USDCTransfer_Solana._encodeRoute` emits (one token,
one call, four account metas):

```
  0  salt               32
 32  deadline            8  u64 LE
 40  portal             32
 72  native_amount       8  u64 LE
 80  tokens.len          4  u32 LE
 84  tokens[0].token    32
116  tokens[0].amount    8  u64 LE   ← slot
124  calls.len           4  u32 LE
128  calls[0].target    32
160  calls[0].data.len   4  u32 LE
164  instrData.len       4  u32 LE
168  0x0c                1            transfer_checked discriminator
169  amount              8  u64 LE   ← slot
177  decimals            1
```

Those offsets hold only for that shape — a second call or a different account list moves the one at 169,
which is exactly why orders carry segments rather than offsets.

The SDK builds segments by encoding the route with a **sentinel** in every runtime position and splitting on
it. That detail matters: it is what stops the segment table from being generated by the same offset
arithmetic it is meant to replace. `integration-tests/tests/common/intent_chainer_context.rs` cuts segments
exactly this way, and `chain_splices_both_solana_amount_positions` requires the spliced result to equal, byte
for byte, what the reference encoder emits for that amount.

### EVM

An EVM route is `abi.encode(Route)`, and the amount typically appears in `route.tokens[0].amount` and again
inside `calls[k].data`. Both are 32-byte big-endian words, i.e. `Slot { width: 32, little_endian: false }`.

## Why it pushes and never funds

`chain` moves value with a plain `transfer_checked` into intent2's vault ATA. It never calls `portal::fund`.

- **Funding is unnecessary.** `withdraw_token` pays `min(reward_token_amount, vault_ata.amount)` and
  `withdraw_native` pays `min(reward.native_amount, vault.lamports())`. There is no funded flag and no
  `IntentFunded` precondition anywhere in `withdraw`, so a pushed intent is fully withdrawable by the proven
  claimant. This mirrors the EVM `Status.Initial` property.
  `chain_funds_the_vault_without_portal_fund` pins it.
- **Funding is awkward from here.** `fund` requires `funder` to be a `Signer`; its ATA-creating path does a
  system transfer from `payer`, and the system program refuses a transfer whose source carries data; and its
  native leg computes `min(needed, funder.lamports())`, draining a short funder outright.

Because portal's `publish` is stateless, it cannot supply the EVM `publish`'s already-settled rejection. The
stand-in is a direct read of `WithdrawnMarker::pda(intent_hash)` — a push into an already-withdrawn vault
would be unrecoverable by the claimant, so it is refused
(`chain_refuses_to_push_into_an_already_withdrawn_intent`).

`PushShortfall` re-reads the vault ATA after the transfer, rejecting a mint that delivers less than it was
sent. On Solana that is the token-2022 transfer-fee case.

## The `publish` flag

Unlike the EVM contract, where `publish` is unconditional because it is the only way to learn the vault
address and to reject a settled hash, here it is a genuine choice: the vault is a derivable PDA and the
settled check reads `WithdrawnMarker` directly, so `publish` buys **only** discoverability.

- `true` — portal emits its canonical `IntentPublished` carrying intent2's route as complete bytes. Use this
  for anything an off-chain solver must find without bespoke indexing.
- `false` — no portal CPI at all. Use it when the caller is itself the solver, or when the indexer
  reconstructs intent2 from the transaction: the whole `Order` is in the instruction data and the splice is
  deterministic given `amount_out`, so the route is always recoverable. `IntentChained` carries `route_hash`
  so a reconstruction can be verified rather than trusted.

### The route-length ceiling, measured

`MAX_ROUTE_LEN` is **2 KB, set by measurement**. Three ceilings were candidates and the intuitive ranking
turned out to be wrong. Driving `chain` with `publish` at increasing route lengths:

| route bytes | compute units | transaction log bytes |
| ----------: | ------------: | --------------------: |
|        1024 |       450,496 |                 3,819 |
|        2048 |       749,781 |                 5,181 |
|        2560 |       877,855 |                 5,861 |
|        3072 |     1,049,099 |                 6,546 |
|        3584 |      heap OOM |                     — |

- **The heap binds first.** This program installs no custom allocator, so it runs on the stock 32 KB heap,
  and the route exists there several times over — Anchor's argument deserialization, the splice, and the
  `publish` CPI's own serialization — on an allocator that never frees. Past ~3 KB that is an unrecoverable
  `ProgramFailedToComplete`.
- **Compute binds next**, at roughly 300 CU per route byte, because `portal::publish` hashes the route with
  `tiny_keccak` in-program rather than through the keccak syscall. 3 KB already costs 1.05M of the 1.4M a
  transaction can request, so publishing near the cap means raising the compute limit above the 400k default.
- **The log budget never binds** in the reachable range. It looked like the dangerous ceiling because it fails
  *silently* — `LogCollector` drops an oversized `Program data:` line without failing the transaction — but
  the two hard ceilings arrive first and fail loudly.

That last point is why `published_event_survives_the_log_budget_at_max_route_len` asserts the *event itself* is
present and byte-exact at the cap, rather than trusting the arithmetic: the constant will drift and an
assertion about the constant drifts with it. Raising `MAX_ROUTE_LEN` past ~3 KB requires the `flash-fulfiller`
treatment — a custom `BumpAllocator` plus a mandatory `request_heap_frame` on every client transaction.

Two consequences shaped the code. Shape checks (`TooManySlots`, `SegmentCountMismatch`, `RouteTooLong`) run in
`validate_order` **before** `Order::hash` streams the order through keccak, so an over-length order is rejected
while it is still cheap — otherwise the caller pays the full hash to be told the route was too long, and at
the default compute limit runs out before hearing it. And `Order::hash` streams through `KeccakWriter` rather
than `borsh::to_vec`, so deriving the commitment never materialises a second copy of the order on that heap.

## Who lands the `chain` transaction

Worth stating, because splitting the transaction moves this from "nobody has to care" to a real operational
question. On EVM the chainer runs inside intent1's fulfillment, so intent1's solver pays for it as part of a
job they already wanted to do; nobody else need be motivated.

Here `chain` is a separate, permissionless transaction that nobody is *obliged* to send. Candidates, in the
order they are likely to act:

- **The party that authored the chain** — the user's relayer or frontend. It wants intent2 to exist and
  already holds the order, so this is the intended path.
- **Intent2's prospective solver.** It can call `chain` itself and immediately fulfill what it just created,
  which is the closest analogue to the EVM flow and needs no coordination.
- **Anyone**, for the price of a transaction, since the call is permissionless and its outcome is fixed by the
  order.

If none of them acts, the measured balance simply stays in the escrow — recoverable only by running `chain`,
because custody is order-scoped. That is a liveness dependency the EVM design does not have, and it is the
main operational cost of the split.

## What the SDK must get right

Three invariants are not enforceable on-chain:

- **Fresh salt per order.** Intent2's salt is fixed in the committed template, so two orders sharing a salt
  *and* landing on the same measured amount produce the same intent hash. That is now **refused**, not merged:
  `chain` reads the vault before pushing and rejects `VaultAlreadyFunded` if it already holds at least the
  amount being pushed. Nothing legitimate can pre-fund intent2's vault — the address is unknowable until the
  amount is measured — so a balance already there means the hash is not fresh. This is the SVM stand-in for
  the EVM `publish` rejecting an already-settled hash; loud beats a silent top-up that funds a delivery which
  already has a claimant. A dust donation does not trip it
  (`a_dust_donation_to_the_vault_does_not_block_the_chain`).

  Note one aliasing worth knowing: `min_amount_in` is part of the order's commitment but **not** of the intent
  hash, so two orders differing only in their floor derive different escrows and the *same* vault.
- **Deadline headroom.** Intent2's deadlines are absolute and fixed when intent1 is authored, but intent1 may
  be fulfilled any time up to its own route deadline. The program rejects a reward deadline inside
  `MIN_DEADLINE_BUFFER` (5 minutes), but leaving real headroom is the builder's job.
- **Slot and call agreement.** If a route's token leg and the calldata that moves it are cut as separate
  slots they receive the same value — but if the template's *calls* expect a different amount than the token
  leg declares, the difference is left on portal's shared executor, claimable by the next fulfiller of any
  intent. `executor_atas_digest` will not save you: it protects `owner`, `delegate` and `close_authority`,
  deliberately not `amount`. Emit both numbers from one value.

## When a route does not fit

The order travels in `chain`'s own instruction data, which is what keeps this program at zero stored state.
**Measured capacity is ~1,376 bytes of route** — `a_realistic_evm_swap_route_fits_one_transaction` walks it.

That is worth stating precisely, because the intuitive figure is wrong and pessimistic. ~500 bytes is what
remains inside intent1's `fulfill` transaction, which carries far more accounts; it is the number that would
matter if this program had the EVM's shape. `chain`'s own transaction is much leaner. A realistic two-call
Base swap route (`approve` + `swapExactTokensForTokens`) is ~960 bytes of `abi.encode(Route)`, so **an
SVM → EVM lane with a destination swap fits today**, with roughly 400 bytes of margin.

Staging is therefore not a blocker for the canonical lanes — it is what a route beyond ~1.3 KB would need: a
longer swap path, three or more calls, or a destination that inflates the encoding. Adding accounts or
arguments to `chain` eats directly into that budget, which is what the test guards.

**Stage it content-addressed, not writer-owned.** The obvious shape — a `(writer, id)`-seeded buffer with an
open/append/seal lifecycle — makes the staging account a security boundary and drags in squatting rules,
a seal ceremony, and a corrupt-buffer story (an RPC retry that duplicates a chunk silently poisons it). None
of that is necessary. Derive the staging PDA from the order commitment the escrow already uses, and have
`chain` verify the account's contents hash to it. Then:

- The staging account is **untrusted data** that either hashes to the commitment or does not. Writer-binding,
  squatting and the seal ceremony all stop being load-bearing.
- A duplicated chunk becomes a failed hash check rather than a poisoned order, so a reset instruction is a
  convenience rather than a correctness fix.
- The commitment is already covered by intent1's hash through the escrow address, so the authorization story
  does not change at all.

This is the design to reach for when those lanes are needed; it is deliberately not built yet, since nothing
in the current matrix requires it. Credit to the EVM side for the framing.

## Recovery

| state                                      | who recovers                                                                 | how                                                  |
| ------------------------------------------ | ---------------------------------------------------------------------------- | ---------------------------------------------------- |
| swap under-delivered below `min_amount_in` | nobody needs to — `chain` fails and the escrow is untouched                   | retry when more arrives                              |
| amount will not fit a slot's width         | nobody needs to — `chain` fails and the escrow is untouched                   | —                                                    |
| intent2 published and funded, never solved | `reward.creator`                                                             | `portal::refund` after `reward.deadline`             |
| intent2 solved                             | claimant takes `amount_in`; any surplus in the vault goes to `reward.creator` | `portal::withdraw`, then `portal::refund`            |
| tokens sent to an escrow whose order is never chained | anyone holding the order preimage                                | `chain` it; custody is order-scoped, so nobody else can |

The last row is the one place this port is structurally riskier than EVM, and it needs more than a habit.
There, the whole order rides inside `intent1.route.calls[k].data`, so `IntentPublished` records it on-chain
forever as a side effect of intent1 existing. Here the order travels only in `chain`'s own instruction data —
so if `chain` is never called, the order was never on-chain at all. An indexer watching intent1 can see the
escrow address but has no path from the address back to the order, and no sweep exists that does not need the
order to derive its signer. **Losing the preimage strands the balance permanently.**

`announce_order` closes that. It is permissionless, takes no accounts, and does nothing but emit the order
alongside the commitment and escrow authority it derives. Call it in the same transaction that publishes
intent1 and the preimage is durable public data, recoverable by anyone. It grants nothing — `chain` was
already permissionless and its outcome is fixed by the order — so there is no reason not to.

An SDK that durably keeps its own orders needs neither; this is the on-chain option for those that would
rather not carry that liability.

## Tests

```bash
anchor build                       # required first: integration tests embed the .so
cargo test --package intent-chainer   # 30 unit tests: splice, slot encoding, scale, commitment, cross-VM
cargo test --test chain               # 36 integration tests
```

Three tests carry more weight than the rest.

`splice_reproduces_the_evm_chainers_route_byte_for_byte` is the **cross-VM** check, and it reads
`testdata/cross-vm-vectors.json` — a fixture meant to be checked into **both repos** and asserted by both
suites. The two ports owe each other agreement on the bytes they produce, not on the shape of the input that
produces them, and nothing else checks that. The vector is not hand-written: its segments are cut from the
route the production Borsh encoder emits, and `expected_route` is what the **EVM** `IntentChainer` published
for `amount_in = 2_500_000` at `scale = 0.96e18` (`eco-routes@e354167`). A transform mismatch, an endianness
flip, or ceil landing on the wrong side would each pass every same-side test and still disagree across the
boundary; this is the one failure mode that would otherwise surface as a solver delivering the wrong amount on
a live lane. `fixture_wad_matches_this_crate` guards the fixture's own premise.

`intent1_hash_commits_to_the_order_through_the_escrow_address` walks the authorization chain end to end —
altering any order field moves the escrow address, which moves intent1's route hash, which moves intent1's own
intent hash — rather than asserting it in prose.

`chain_cannot_publish_from_inside_a_route_call` drives a real `portal::fulfill` and asserts the exact
`InstructionError::ReentrancyNotAllowed`, so the rule the design rests on is verified, not assumed.

The integration suite is otherwise built around one end-to-end path that only uses real portal entry points —
`chained_svm_to_svm_intent_is_funded_and_withdrawable`: fulfill intent1 so its route call delivers into the
escrow, `chain` to resolve and fund intent2 from the measured balance, then prove and withdraw intent2 so the
pushed tokens actually reach a claimant. The delivered amount is deliberately not a round number and not
anything the order declares, so the test can only pass if the amount is genuinely measured.
