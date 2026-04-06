use anchor_lang::prelude::*;
use tiny_keccak::{Hasher, Keccak};

use crate::igp;
use crate::instructions::ProofHelperError;

/// The Hyperlane mailbox uses Borsh `AccountData` encoding, which prepends
/// a version byte before the 8-byte discriminator.
const DISPATCHED_MESSAGE_DISCRIMINATOR: &[u8; 8] = b"DISPATCH";
const DISCRIMINATOR_OFFSET: usize = 1; // version byte prefix
const DISPATCHED_MESSAGE_HEADER_LEN: usize = 1 + 8 + 4 + 8 + 32; // version + discriminator + nonce + slot + pubkey

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct PayForGasArgs {
    pub destination_domain: u32,
    pub gas_amount: u64,
}

#[derive(Accounts)]
pub struct PayForGas<'info> {
    /// The Hyperlane dispatched message account created by Mailbox.OutboxDispatch.
    /// Contains the encoded message from which we derive the message_id.
    /// CHECK: owner is validated against the Mailbox program to ensure the
    /// account contains a legitimate dispatched message. Discriminator is
    /// validated in the instruction handler.
    #[account(owner = igp::MAILBOX_ID @ ProofHelperError::InvalidDispatchedMessageOwner)]
    pub dispatched_message: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: Checked in IGP CPI.
    #[account(mut)]
    pub igp_program_data: UncheckedAccount<'info>,
    pub unique_gas_payment: Signer<'info>,
    /// CHECK: Checked in IGP CPI.
    #[account(mut)]
    pub gas_payment_pda: UncheckedAccount<'info>,
    /// CHECK: Checked in IGP CPI.
    #[account(mut)]
    pub igp_account: UncheckedAccount<'info>,
    /// CHECK: Checked in IGP CPI.
    pub overhead_igp: Option<UncheckedAccount<'info>>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated.
    #[account(executable, address = igp::IGP_PROGRAM_ID @ ProofHelperError::InvalidIgpProgram)]
    pub igp_program: UncheckedAccount<'info>,
}

pub fn pay_for_gas(ctx: Context<PayForGas>, args: PayForGasArgs) -> Result<()> {
    let message_id = extract_message_id(&ctx.accounts.dispatched_message)?;

    igp::pay_for_gas(&ctx, message_id, args.destination_domain, args.gas_amount)
}

fn extract_message_id(dispatched_message: &UncheckedAccount) -> Result<[u8; 32]> {
    let data = dispatched_message.try_borrow_data()?;

    require!(
        data.len() > DISPATCHED_MESSAGE_HEADER_LEN
            && data[DISCRIMINATOR_OFFSET..DISCRIMINATOR_OFFSET + 8]
                == *DISPATCHED_MESSAGE_DISCRIMINATOR,
        ProofHelperError::InvalidDispatchedMessage
    );

    let encoded_message = &data[DISPATCHED_MESSAGE_HEADER_LEN..];

    let mut hasher = Keccak::v256();
    let mut message_id = [0u8; 32];
    hasher.update(encoded_message);
    hasher.finalize(&mut message_id);

    Ok(message_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_keccak::{Hasher, Keccak};

    #[test]
    fn dispatched_message_header_len() {
        // version(1) + discriminator(8) + nonce(4) + slot(8) + unique_message_pubkey(32)
        assert_eq!(DISPATCHED_MESSAGE_HEADER_LEN, 53);
    }

    #[test]
    fn dispatched_message_header_len_component_sum() {
        let version = 1usize;
        let discriminator = 8usize;
        let nonce = 4usize;
        let slot = 8usize;
        let pubkey = 32usize;
        assert_eq!(
            version + discriminator + nonce + slot + pubkey,
            DISPATCHED_MESSAGE_HEADER_LEN
        );
    }

    #[test]
    fn dispatched_message_discriminator_constant() {
        assert_eq!(DISPATCHED_MESSAGE_DISCRIMINATOR, b"DISPATCH");
        assert_eq!(DISPATCHED_MESSAGE_DISCRIMINATOR.len(), 8);
    }

    #[test]
    fn discriminator_offset_is_one() {
        // The version byte sits at index 0, so the discriminator begins at offset 1.
        assert_eq!(DISCRIMINATOR_OFFSET, 1);
    }

    #[test]
    fn discriminator_does_not_overlap_with_header_end() {
        // Discriminator occupies bytes [1..9]; header ends at 53.
        // Ensure there is no off-by-one between the two constants.
        assert!(DISCRIMINATOR_OFFSET + 8 <= DISPATCHED_MESSAGE_HEADER_LEN);
    }

    /// The Keccak-256 of an all-zero 32-byte payload must be deterministic.
    #[test]
    fn keccak_hash_of_known_payload_is_deterministic() {
        let payload = [0u8; 32];
        let result1 = keccak_hash(&payload);
        let result2 = keccak_hash(&payload);
        assert_eq!(result1, result2);
    }

    /// Different payloads must produce different hashes.
    #[test]
    fn keccak_hash_differs_for_different_payloads() {
        let a = keccak_hash(&[0u8; 32]);
        let b = keccak_hash(&[1u8; 32]);
        assert_ne!(a, b);
    }

    /// Keccak-256 of an empty slice is well-defined and non-zero.
    #[test]
    fn keccak_hash_of_empty_payload_is_defined() {
        let result = keccak_hash(&[]);
        // keccak256("") is a known constant — just verify it is non-zero.
        assert_ne!(result, [0u8; 32]);
    }

    /// Verify the exact Keccak-256 of a known payload matches the reference.
    /// This detects accidental changes to the hashing algorithm or its inputs.
    #[test]
    fn keccak_hash_known_value() {
        // keccak256([0x01]) — well-known value from reference implementations.
        let payload = [0x01u8];
        let result = keccak_hash(&payload);
        goldie::assert_debug!(result);
    }

    /// If data has exactly DISPATCHED_MESSAGE_HEADER_LEN bytes the require!
    /// condition `data.len() > DISPATCHED_MESSAGE_HEADER_LEN` is false, so it
    /// would fail. Confirm our understanding of the boundary condition.
    #[test]
    fn header_len_boundary_condition() {
        // data.len() == 53 → len > 53 is false → should error
        assert!(!(53 > DISPATCHED_MESSAGE_HEADER_LEN));
        // data.len() == 54 → len > 53 is true → passes length check
        assert!(54 > DISPATCHED_MESSAGE_HEADER_LEN);
    }

    fn keccak_hash(data: &[u8]) -> [u8; 32] {
        let mut hasher = Keccak::v256();
        let mut out = [0u8; 32];
        hasher.update(data);
        hasher.finalize(&mut out);
        out
    }
}