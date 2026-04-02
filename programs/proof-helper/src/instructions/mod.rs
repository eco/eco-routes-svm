use anchor_lang::prelude::*;

mod pay_for_gas;

pub use pay_for_gas::*;

#[error_code]
pub enum ProofHelperError {
    #[msg("Dispatched message account is not owned by the Mailbox program")]
    InvalidDispatchedMessageOwner,
    #[msg("Invalid dispatched message discriminator or data")]
    InvalidDispatchedMessage,
    #[msg("IGP program address does not match expected")]
    InvalidIgpProgram,
}
