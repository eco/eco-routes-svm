/// Largest batch `prove` accepts. Chosen so `[ComputeBudget, portal::prove,
/// send_message]` fits one 1232-byte v0 transaction with the FulfillMarkers
/// and LayerZero accounts in address lookup tables; pinned by
/// `layerzero_prover_batch_limits::outbound_ceiling_matches_max_intents_per_prove`.
///
/// The margin is thin. The measurement assumes exactly one lookup table and at
/// most 4 DVNs per path; 21 intents serialize to 1214 bytes, so a
/// compute-unit-price instruction (12 bytes) still fits but a second table
/// pushes 21 over the packet limit. The fallback is to send `portal::prove`
/// and `send_message` in separate transactions (the `PendingSend` commit
/// persists between them).
pub const MAX_INTENTS_PER_PROVE: usize = 21;

/// Largest inbound batch the executor's delivery transaction can carry: every
/// pair needs its brand-new `Proof` PDA as a static key (a lookup table cannot
/// hold an address nobody pre-registered) plus 64 message bytes. The EVM
/// `LayerZeroProver` cannot enforce it, so the solver must batch EVM
/// `Inbox.prove` calls toward Solana at or below this. Pinned by
/// `layerzero_prover_batch_limits::inbound_ceiling_matches_max_pairs_per_message`.
pub const MAX_PAIRS_PER_MESSAGE: usize = 7;

/// Same floor the EVM `LayerZeroProver` computes, so a permissionless
/// `send_message` caller cannot under-gas the EVM `lzReceive`.
///
/// These must equal the EVM `LayerZeroProver`'s `MIN_GAS_LIMIT` (its
/// constructor `minGasLimit`, deployed as 200000 by eco-routes
/// `scripts/Deploy.s.sol` `deployLayerZeroProver`) and its `GAS_PER_INTENT`
/// constant (50_000). An EVM prover deployed with a higher floor would receive
/// under-gassed messages that need a manual `lzReceive` retry.
pub const MIN_GAS_LIMIT: u128 = 200_000;
pub const GAS_PER_INTENT: u128 = 50_000;

pub fn lz_receive_gas(intents: usize) -> u128 {
    MIN_GAS_LIMIT + intents as u128 * GAS_PER_INTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz_receive_gas_matches_evm_floor() {
        assert_eq!(lz_receive_gas(0), 200_000);
        assert_eq!(lz_receive_gas(2), 300_000);
        assert_eq!(lz_receive_gas(MAX_INTENTS_PER_PROVE), 1_250_000);
    }
}
