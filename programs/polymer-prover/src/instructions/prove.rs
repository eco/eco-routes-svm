use anchor_lang::prelude::*;
use eco_svm_std::prover::{IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::CHAIN_ID;

use crate::instructions::PolymerProverError;

/// Solana truncates a transaction's log buffer at 10,000 bytes
/// (`LOG_MESSAGES_BYTES_LIMIT`) while the transaction still succeeds, so an
/// intent whose line fell past the limit can never be proven. Each intent costs
/// ~347 bytes end to end: the 222-byte `Prove:` line, its 13-byte
/// `Program log: ` runtime prefix, and Portal's own ~110-byte
/// `Program data: ` `IntentProven` event. 24 x 347 = ~8.3 KB of the 10 KB
/// budget; 26 is the measured ceiling through `portal::prove`, and the margin
/// absorbs future Portal log additions. Pinned by the
/// `prove_via_portal_at_max_intents_emits_every_log_untruncated` integration
/// test.
pub const MAX_INTENTS_PER_PROVE: usize = 24;
/// hex(source u64 ‖ destination u64 ‖ intent_hash ‖ claimant) = 2 * 80.
pub const PROVE_LOG_PAYLOAD_LEN: usize = 160;
const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Accounts)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda(&crate::ID).0 @ PolymerProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
}

/// Emits one Polymer-provable log line per intent:
/// `Prove: program: <base58 program id>, <160 hex chars>`. The EVM
/// `PolymerProver.validateSolana` parses these back; the layout is a shared ABI.
pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        domain_id,
        proof_data,
        ..
    } = args;

    check_prove_args(&proof_data)?;

    for pair in &proof_data.intent_hashes_claimants {
        let payload = prove_log_payload(domain_id, CHAIN_ID, pair);
        msg!(
            "Prove: program: {}, {}",
            ctx.program_id,
            core::str::from_utf8(&payload).expect("hex is ascii")
        );
    }

    Ok(())
}

pub fn check_prove_args(proof_data: &ProofData) -> Result<()> {
    require!(
        proof_data.destination == CHAIN_ID,
        PolymerProverError::InvalidDestination
    );
    require!(
        !proof_data.intent_hashes_claimants.is_empty(),
        PolymerProverError::EmptyProofData
    );
    require!(
        proof_data.intent_hashes_claimants.len() <= MAX_INTENTS_PER_PROVE,
        PolymerProverError::TooManyIntents
    );
    Ok(())
}

/// Lowercase hex of `source ‖ destination ‖ intent_hash ‖ claimant` (80 bytes),
/// encoded into a stack buffer.
pub fn prove_log_payload(
    source: u64,
    destination: u64,
    pair: &IntentHashClaimant,
) -> [u8; PROVE_LOG_PAYLOAD_LEN] {
    let mut out = [0u8; PROVE_LOG_PAYLOAD_LEN];
    let bytes = source
        .to_be_bytes()
        .into_iter()
        .chain(destination.to_be_bytes())
        .chain(pair.intent_hash)
        .chain(pair.claimant);
    for (i, byte) in bytes.enumerate() {
        out[2 * i] = HEX[usize::from(byte >> 4)];
        out[2 * i + 1] = HEX[usize::from(byte & 0x0f)];
    }
    out
}

/// The exact line `prove_intent` logs (without the `Program log: ` runtime prefix).
pub fn prove_log_line(program_id: &Pubkey, payload: &[u8; PROVE_LOG_PAYLOAD_LEN]) -> String {
    format!(
        "Prove: program: {}, {}",
        program_id,
        core::str::from_utf8(payload).expect("hex is ascii")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> IntentHashClaimant {
        IntentHashClaimant::new([0x11; 32].into(), [0x22; 32].into())
    }

    #[test]
    fn prove_log_payload_layout() {
        let payload = prove_log_payload(8453, CHAIN_ID, &pair());
        goldie::assert_debug!(core::str::from_utf8(&payload).unwrap());
    }

    #[test]
    fn prove_log_line_format() {
        let payload = prove_log_payload(8453, CHAIN_ID, &pair());
        let line = prove_log_line(&crate::ID, &payload);
        assert!(line.starts_with(&format!("Prove: program: {}, ", crate::ID)));
        assert!(line.len() < 500);
        goldie::assert_debug!(line);
    }

    #[test]
    fn check_prove_args_rejects_wrong_destination() {
        let proof_data = ProofData::new(CHAIN_ID + 1, vec![pair()]);
        assert_eq!(
            check_prove_args(&proof_data).unwrap_err(),
            PolymerProverError::InvalidDestination.into()
        );
    }

    #[test]
    fn check_prove_args_rejects_empty() {
        let proof_data = ProofData::new(CHAIN_ID, vec![]);
        assert_eq!(
            check_prove_args(&proof_data).unwrap_err(),
            PolymerProverError::EmptyProofData.into()
        );
    }

    #[test]
    fn check_prove_args_rejects_too_many() {
        let proof_data = ProofData::new(CHAIN_ID, vec![pair(); MAX_INTENTS_PER_PROVE + 1]);
        assert_eq!(
            check_prove_args(&proof_data).unwrap_err(),
            PolymerProverError::TooManyIntents.into()
        );
    }

    #[test]
    fn check_prove_args_accepts_max() {
        let proof_data = ProofData::new(CHAIN_ID, vec![pair(); MAX_INTENTS_PER_PROVE]);
        assert!(check_prove_args(&proof_data).is_ok());
    }
}
