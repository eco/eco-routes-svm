use anchor_lang::prelude::*;
use anchor_lang::solana_program::entrypoint::ProgramResult;
use anchor_lang::solana_program::program::set_return_data;
use eco_svm_std::prover::{GetProofArgs, Proof, GET_PROOF_DISCRIMINATOR};

anchor_lang::solana_program::entrypoint!(process_instruction);

// Runtime program IDs let resource-limit tests install eight independent members.
fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let args = data
        .strip_prefix(&GET_PROOF_DISCRIMINATOR)
        .ok_or(ProgramError::InvalidInstructionData)?;
    let args = GetProofArgs::try_from_slice(args)?;
    let account_count = match args.data.as_slice() {
        [] => 1,
        [count] => usize::from(*count),
        _ => return Err(ProgramError::InvalidInstructionData),
    };
    if accounts.len() != account_count {
        return Err(ProgramError::NotEnoughAccountKeys);
    }
    let proof = accounts.first().ok_or(ProgramError::NotEnoughAccountKeys)?;
    let valid = Proof::get(proof, program_id, args)?;
    set_return_data(&borsh::to_vec(&valid)?);

    Ok(())
}
