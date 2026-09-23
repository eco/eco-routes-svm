use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

/// Seed for per-order `escrow_authority` PDAs.
pub const ESCROW_SEED: &[u8] = b"escrow";

/// Transport only: never a token custody authority or an execution signer.
pub const ORDER_BUFFER_SEED: &[u8] = b"order_buffer";

/// Fixed Borsh header followed by exactly `order_len` raw Borsh Order bytes.
/// Keeping the payload outside a Vec avoids copying it on account load/exit.
#[account]
pub struct OrderBuffer {
    pub authority: Pubkey,
    /// Fresh random per-trade client seed; not a shared buffer index.
    pub seed: [u8; 32],
    pub order_commitment: Bytes32,
    pub order_len: u32,
    pub written: u32,
    pub sealed: bool,
    pub bump: u8,
}

impl OrderBuffer {
    pub const HEADER_LEN: usize = 8 + 32 + 32 + 32 + 4 + 4 + 1 + 1;

    pub fn pda(authority: &Pubkey, seed: &[u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[ORDER_BUFFER_SEED, authority.as_ref(), seed], &crate::ID)
    }
}

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

/// Intent2's vault, derived under the **order's** portal rather than a linked-in
/// constant.
///
/// Re-derived here rather than calling `portal::state::vault_pda`, which resolves
/// against portal's own `crate::ID` and would hard-bind this program to one portal
/// deployment. The seed is portal's own public
/// constant, so the derivation cannot drift from the one portal itself uses.
pub fn vault_pda(portal: &Pubkey, intent_hash: &Bytes32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[portal::state::VAULT_SEED, intent_hash.as_ref()], portal)
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
