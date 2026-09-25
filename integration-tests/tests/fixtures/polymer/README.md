# Polymer fixtures

## `op-proof-v2.hex`

Polymer's own test proof, licensed Apache-2.0, vendored 2026-09-22 from
polymerdao/solana-prover-contracts at commit 04f74b1ea9fdbdf8d08879c5437b1694af2d9ea1
(the last upstream commit to touch `programs/polymer-prover/src/instructions/test-data/`;
2184 bytes, sha256 3e2bf488b7af08d1e2ae9fcf73061905787b3b2e19ed85c6b0461787df09a22e). To
re-verify the local blob is what was vendored:

    curl -sSL https://raw.githubusercontent.com/polymerdao/solana-prover-contracts/04f74b1ea9fdbdf8d08879c5437b1694af2d9ea1/programs/polymer-prover/src/instructions/test-data/op-proof-v2.hex \
      | shasum -a 256

It proves an OP Sepolia event (chain 11155420) emitted by
`0xf221750e52aa080835d2957f2eed0d5d7ddd8c38` with four topics, signed by Polymer's
test sequencer `0x8D3921B96A3815F403Fb3a4c7fF525969d16f9E0` for client type `proof_api`
on peptide chain 901.

Used by the `#[ignore]` test in `validate_polymer_prover_real.rs`, which needs the real
program binary:

    solana program dump FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3 /tmp/polymer_prover.so --url devnet
    POLYMER_PROVER_SO=/tmp/polymer_prover.so cargo test --test validate_polymer_prover_real -- --ignored

The scheduled `Polymer upstream drift` workflow (`.github/workflows/polymer-upstream-drift.yml`)
runs the same test weekly against the binaries deployed on devnet and mainnet-beta.

## `validation-result-v1.0.4.hex`

The raw `["result", authority]` account (all 3141 bytes: 8-byte discriminator, Borsh body,
zero padding to `INIT_SPACE`) that Polymer's **real devnet program** wrote after
`validate_event` accepted `op-proof-v2.hex`. Captured 2026-09-22 in litesvm from
`FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3` as dumped that day (program last deployed in
slot 463905187; `.so` sha256 76a78e45b46c1d3d22ea033022cfe7c6d8d6ae119e1e27534f7e8f2c506593d0),
whose source is upstream tag v1.0.4.

Used by the non-ignored `polymer_written_result_account_decodes_with_our_mirror` test in
`validate_polymer_prover_real.rs`: it feeds these bytes to our hand-rolled
`polymer::ValidationResult::try_from_account_info` and asserts the values the fixture implies,
so a field reorder or width change in the mirror fails on every PR without network access. If
Polymer changes the layout, re-capture the bytes with a fresh dump and bump the version in the
file name.
