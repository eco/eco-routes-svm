use anchor_lang::prelude::*;
use anchor_lang::solana_program::entrypoint::ProgramResult;
use anchor_lang::solana_program::program::set_return_data;
use eco_svm_std::prover::{Proof, ValidateProofArgs, VALIDATE_PROOF_DISCRIMINATOR};

anchor_lang::solana_program::entrypoint!(process_instruction);

// Runtime program IDs let resource-limit tests install eight independent members.
fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let args = data
        .strip_prefix(&VALIDATE_PROOF_DISCRIMINATOR)
        .ok_or(ProgramError::InvalidInstructionData)?;
    let args = ValidateProofArgs::try_from_slice(args)?;
    let proof = accounts.first().ok_or(ProgramError::NotEnoughAccountKeys)?;
    let valid = Proof::validate(proof, program_id, args)?;
    set_return_data(&[u8::from(valid)]);

    Ok(())
}
