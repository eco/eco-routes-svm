use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

/// Seed for per-order `escrow_authority` PDAs.
pub const ESCROW_SEED: &[u8] = b"escrow";

/// Per-order custody authority: owns the ATA that intent1 delivers its output
/// into, and signs the push of that balance into intent2's vault.
///
/// **Keep it seeded by the order commitment — this is a security boundary.**
///
/// `chain` is permissionless and imposes no signer check, exactly as the EVM
/// `IntentChainer` does, because route calls reach every program through
/// portal's single shared executor and so prove nothing about whose intent is
/// running. On EVM the authorization anchor is intent1's own hash: the whole
/// order rides inside `intent1.route.calls[k].data`, which is covered by the
/// intent hash the inbox re-derives before executing anything.
///
/// Solana cannot reproduce that directly — intent2's vault address depends on the
/// measured amount, and every account a transaction touches must be named up
/// front, so the push cannot happen inside intent1's fulfillment (see the module
/// docs on `lib.rs`). The balance is therefore **at rest** between intent1's
/// fulfillment and the `chain` call, and a single shared custody account would
/// let whoever calls `chain` first sweep it into an order of their own choosing —
/// with `reward.creator` set to themselves and a short deadline, they refund it
/// out.
///
/// Seeding custody by the commitment restores the EVM property transitively:
/// intent1's route names this address as its swap recipient, that route is
/// hash-committed, and the address encodes exactly one order. Funding it *is*
/// approval of the order behind it, and no other order can derive it. Collapsing
/// these seeds to anything the order does not determine reopens the sweep.
pub fn escrow_authority_pda(order_commitment: &Bytes32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[ESCROW_SEED, order_commitment.as_ref()], &crate::ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escrow_authority_pda_deterministic() {
        goldie::assert_json!(escrow_authority_pda(&Bytes32::from([42u8; 32])));
    }

    #[test]
    fn escrow_authority_pda_varies_by_commitment() {
        let (a, _) = escrow_authority_pda(&Bytes32::from([1u8; 32]));
        let (b, _) = escrow_authority_pda(&Bytes32::from([2u8; 32]));

        assert_ne!(a, b);
    }
}
