use anchor_lang::prelude::*;
use tiny_keccak::{Hasher, Keccak};

use crate::igp;
use crate::instructions::ProofHelperError;

const DISPATCHED_MESSAGE_DISCRIMINATOR: &[u8; 8] = b"DISPATCH";
const DISPATCHED_MESSAGE_HEADER_LEN: usize = 8 + 4 + 8 + 32; // discriminator + nonce + slot + pubkey

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct PayForGasArgs {
    pub destination_domain: u32,
    pub gas_amount: u64,
}

#[derive(Accounts)]
pub struct PayForGas<'info> {
    /// The Hyperlane dispatched message account created by Mailbox.OutboxDispatch.
    /// Contains the encoded message from which we derive the message_id.
    /// CHECK: discriminator is validated in the instruction handler.
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
            && data[..8] == *DISPATCHED_MESSAGE_DISCRIMINATOR,
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

    #[test]
    fn dispatched_message_header_len() {
        // discriminator(8) + nonce(4) + slot(8) + unique_message_pubkey(32)
        assert_eq!(DISPATCHED_MESSAGE_HEADER_LEN, 52);
    }
}
