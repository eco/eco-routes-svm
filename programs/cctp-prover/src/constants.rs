/// Largest batch `prove` accepts: `[ComputeBudget, portal::prove]` fits one
/// 1232-byte v0 transaction (payer + event keypair signatures) with the
/// FulfillMarkers and static accounts in one lookup table. Measured: 23
/// intents take 1227 bytes (24 take 1261) and 33 of 64 account locks, and
/// execute in about 225k CU. The 5 bytes of headroom are thin: one extra
/// instruction, such as a ~12-byte compute-unit-price, drops the ceiling to
/// 22, and the ceiling assumes exactly one lookup table holding the
/// FulfillMarkers. Pinned by
/// `cctp_prover_batch_limits::outbound_ceiling_matches_max_intents_per_prove`.
pub const MAX_INTENTS_PER_PROVE: usize = 23;

/// Largest EVM -> Solana batch one relay transaction can deliver: every pair
/// needs its brand-new `Proof` PDA as a static key plus 64 body bytes, next to
/// a 148-byte header and a 130-byte (2-of-2) attestation, in a v0 transaction
/// with `cctp::receive_lookup_table_addresses()` in a lookup table. Measured: 6
/// pairs take 1184 bytes (48 bytes headroom), 18 of 64 account locks and 229k
/// CU with a 2-of-2 attestation; a legacy transaction fits only 4. The EVM
/// `CCTPProver` must pin `maxPairs` for domain 5 to this value
/// (`CCTP_MAX_PAIRS_CONFIG=5:6`) so larger batches revert at send. Pinned by
/// `cctp_prover_batch_limits::inbound_ceiling_matches_max_pairs_per_message`.
pub const MAX_PAIRS_PER_MESSAGE: usize = 6;
