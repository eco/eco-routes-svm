# Split-fulfillment venue fixtures

Real mainnet venues for the split path (`flash_fulfiller.prove_and_withdraw` plus a
top-level `portal.fulfill`), used by the venue tests in
`integration-tests/tests/prove_and_withdraw.rs` through
`tests/common/split_venue_context.rs`. There is one route per program the motivating
venue families CPI into between themselves and SPL Token: Helium's circuit breaker,
stabble's vault and Meteora's dynamic vault. stabble's weighted pools share the vault
program and the account list with its stable pools, and Meteora's constant-product
pools share both binaries and the account list with its stable pools.

Snapshot: mainnet-beta slots 471448476–471448513 (2026-11-19 19:12:36–19:12:44 UTC).
Every JSON file is `solana account <address> --output json` at that snapshot, and the
tests set the clock to `solana block-time 471448513` = 1795115563
(`VENUE_SNAPSHOT_UNIX_TIMESTAMP`), because the venues read it.

## Programs

| Program | ID | Last deployed in slot | Bytes | sha256 | License |
| --- | --- | --- | --- | --- | --- |
| Helium treasury-management | `treaf4wWBBty3fHdyBpo35Mz84M8k3heKXmjmi9vFt5` | 339861344 | 728992 | `b7569880426f864b9a23bb4efead42019c562fa38d68a67d2be12cbecddb4649` | Apache-2.0 |
| Helium circuit-breaker | `circAbx64bbsscPbQzZAUvuXpHqrCe6fLMzc2uKXz9g` | 339861292 | 638560 | `e07a3891c3ee5394810941db5ce56e480eb545a882e6a8b450f074c0c16e50c3` | Apache-2.0 |
| stabble stable-swap | `swapNyd8XiQwJ6ianp9snpu4brUqFxadzvHebnAXjJZ` | 471391543 | 1246000 | `f6277bbb38ff494944ba5fa21361ff09c8de672b3eb0633a585372cadd94ff2a` | all rights reserved |
| stabble vault | `vo1tWgqZMjG61Z2T9qUaMYKqZ75CYzMuaZ2LZP1n7HV` | 346353426 | 789000 | `5a9b742c972f94db0c0fe50b069eaa1779574693b0438b55f486b1a4d9747f51` | all rights reserved |
| Meteora DAMM v1 | `Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB` | 468782174 | 1924896 | `524353534312b332e486537f3c420b738cf09241e2f65b3eea7969ea28b8732a` | none published |
| Meteora dynamic vault | `24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi` | 457247303 | 1930560 | `2ca789753622df50bd4eb5d7a07769943d6795349196f7420657951875b827fd` | none published |

Helium publishes both programs under Apache-2.0 (helium/helium-program-library), so
they are vendored in `bins/` like the other external binaries, their digests are
pinned by `vendored_helium_binaries_match_their_dump_receipts`, and the Helium route
runs on every PR. stabble's source license reserves all rights and grants none for
distribution, and Meteora publishes no license, so their binaries are not committed:
their two tests are `#[ignore]`d and load binaries dumped on demand.

```bash
dir="$TMPDIR/split-venue-programs"; mkdir -p "$dir"
solana program dump swapNyd8XiQwJ6ianp9snpu4brUqFxadzvHebnAXjJZ "$dir/stabble_stable_swap.so" --url mainnet-beta
solana program dump vo1tWgqZMjG61Z2T9qUaMYKqZ75CYzMuaZ2LZP1n7HV "$dir/stabble_vault.so" --url mainnet-beta
solana program dump Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB "$dir/meteora_damm_v1.so" --url mainnet-beta
solana program dump 24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi "$dir/meteora_vault.so" --url mainnet-beta
SPLIT_VENUE_PROGRAMS="$dir" cargo test --test prove_and_withdraw -- --ignored --nocapture
```

The tests print each dumped binary's sha256, the packet size, the account locks and
the compute used; cite them with the date. A binary that differs from the table runs
against this snapshot's state, so a venue upgrade that changes an account layout fails
here first. The `Split venue drift` workflow (`.github/workflows/split-venue-drift.yml`)
runs the same tests weekly against what mainnet runs then.

## Accounts

| Fixture | Address | Role |
| --- | --- | --- |
| `mints/usdc.json` | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | USDC mint |
| `mints/usdt.json` | `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB` | USDT mint |
| `mints/mobile.json` | `mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6` | MOBILE mint (Helium supply mint) |
| `mints/hnt.json` | `hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux` | HNT mint (Helium treasury mint) |
| `helium-treasury/treasury-management.json` | `Aon7sbdvCGuXQJW8BEiDDWzsSvoycTL9v3L1S4GWhxNK` | MOBILE `TreasuryManagementV0` (curve `k = 0`) |
| `helium-treasury/treasury.json` | `BUCbfE1pbjUYrdw39kFWHqE3GuvwNVpN4gpWkmugQCpR` | its HNT treasury |
| `helium-treasury/circuit-breaker.json` | `AguTdjmW5SkhepT9qsKsj29SEqiVKsJchsap6Kma9i98` | the treasury's windowed breaker |
| `stabble-stable-swap/pool.json` | `5K7CHUbBYAh6wrantyJvDDqwT4VoKuZTi73CN1DTUUer` | USDT/USDC stable pool |
| `stabble-stable-swap/vault.json` | `stab1io8dHvK26KoHmTwwHyYmHRbUWbyEJx6CdrGabC` | its vault |
| `stabble-stable-swap/vault-authority.json` | `7imnGYfCovXjMWKdbQvETFVMe72MQDX4S5zW4GFxMJME` | vault authority (owns the vault token accounts) |
| `stabble-stable-swap/vault-token-usdc.json` | `AioJRQXvcDLRhHMd6DAkTbbMpgVx63qSGQYmRBS2vHYA` | vault USDC account |
| `stabble-stable-swap/vault-token-usdt.json` | `95QUtvDkuoDZrNJiuh9MdahkpRNtSVhZRe83oepd8AM7` | vault USDT account |
| `stabble-stable-swap/beneficiary-token-usdt.json` | `4CQUrzq6qaVtMVtWEL2CvaZjqBUxnMJtBgM6M3hHHDsJ` | fee beneficiary's USDT ATA |
| `meteora-damm-v1-stable/pool.json` | `32D4zRxNc1EssbJieVHfPhZM3rH6CzfUPrWUuWxD9prG` | USDC/USDT stable pool |
| `meteora-damm-v1-stable/a-vault.json` | `3ESUFCnRNgZ7Mn2mPPUMmXYaKU8jpnV9VtA17M7t2mHQ` | USDC dynamic vault |
| `meteora-damm-v1-stable/b-vault.json` | `5XCP3oD3JAuQyDpfBFFVUxsBxNjPQojpKuL4aVhHsDok` | USDT dynamic vault |
| `meteora-damm-v1-stable/a-token-vault.json` | `C2QoQ111jGHEy5918XkNXQro7gGwC9PKLXd1LqBiYNwA` | USDC vault's token account |
| `meteora-damm-v1-stable/b-token-vault.json` | `DQjGWHN9ERn1zSMpWLNvSpTFUSfnxbanBt9A7xyU2bVE` | USDT vault's token account |
| `meteora-damm-v1-stable/a-lp-mint.json` | `3RpEekjLE5cdcG15YcXJUpxSepemvq2FpmMcgo342BwC` | USDC vault LP mint |
| `meteora-damm-v1-stable/b-lp-mint.json` | `EZun6G5514FeqYtUv26cBHWLqXjAEdjGuoX6ThBpBtKj` | USDT vault LP mint |
| `meteora-damm-v1-stable/a-vault-lp.json` | `24NYE3hHQyUTrHUT4n1CcVrMP9Xy3ULuT1Uurw1HDeck` | pool's USDC vault LP account |
| `meteora-damm-v1-stable/b-vault-lp.json` | `Hv5ogVb2BZCF3ET2KnaEYj2seKHN5ffGDazm6BGt5DD9` | pool's USDT vault LP account |
| `meteora-damm-v1-stable/protocol-token-a-fee.json` | `4Qjrnzp5jXPSBhyv495ApB1SdDbXdZ5Pc9ZSiabf9NmJ` | USDC protocol-fee account |

stabble's vault withdraw authority, `8BSWYgAczR36C7ukr32v7uTepoRhYJYxAVnpBtYniZTm`,
is a signing PDA with no account, so it has no fixture.

## Refreshing

Re-dump every account above in one pass, each with
`solana account <address> --output json --url mainnet-beta > <fixture>`, note the
first and last slot, set `VENUE_SNAPSHOT_UNIX_TIMESTAMP` to
`solana block-time <last slot>`, and update the snapshot line here. When the Helium
programs are re-dumped too, update their table rows, the `HELIUM_*_SHA256` constants
and the slot in their doc comments in `split_venue_context.rs`.
