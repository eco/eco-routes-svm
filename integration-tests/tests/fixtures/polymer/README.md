# Polymer fixtures

`op-proof-v2.hex` is Polymer's own test proof from
polymerdao/solana-prover-contracts (`programs/polymer-prover/src/instructions/test-data/`),
licensed Apache-2.0. It proves an OP Sepolia event (chain 11155420) emitted by
`0xf221750e52aa080835d2957f2eed0d5d7ddd8c38` with four topics, signed by Polymer's
test sequencer `0x8D3921B96A3815F403Fb3a4c7fF525969d16f9E0` for client type `proof_api`
on peptide chain 901.

Used only by the `#[ignore]` test `validate_polymer_prover_real.rs`, which needs the real
program binary:

    solana program dump FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3 /tmp/polymer_prover.so --url devnet
    POLYMER_PROVER_SO=/tmp/polymer_prover.so cargo test --test validate_polymer_prover_real -- --ignored
