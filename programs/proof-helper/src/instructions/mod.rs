use anchor_lang::prelude::*;

mod pay_for_gas;

pub use pay_for_gas::*;

#[error_code]
pub enum ProofHelperError {
    #[msg("Invalid dispatched message discriminator")]
    InvalidDispatchedMessage,
    #[msg("IGP program address does not match expected")]
    InvalidIgpProgram,
}
