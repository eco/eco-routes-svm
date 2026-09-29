use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::{get_return_data, invoke, invoke_signed};

use super::{
    ProverError, ValidateProofArgs, CLOSE_PROOF_DISCRIMINATOR, VALIDATE_PROOF_DISCRIMINATOR,
};
use crate::{claimant, Bytes32};

pub fn validate_proof<'info>(
    prover: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    intent_hash: Bytes32,
    destination: u64,
    claimant: Pubkey,
) -> Result<bool> {
    if !claimant::is_payable(&claimant) {
        return Ok(false);
    }

    invoke_validate_proof(
        prover,
        accounts,
        ValidateProofArgs::new(intent_hash, destination, Some(claimant)),
    )
}

pub fn validate_cancelled<'info>(
    prover: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    intent_hash: Bytes32,
    destination: u64,
) -> Result<bool> {
    invoke_validate_proof(
        prover,
        accounts,
        ValidateProofArgs::new(intent_hash, destination, Some(claimant::cancelled())),
    )
}

pub fn has_proof<'info>(
    prover: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    intent_hash: Bytes32,
    destination: u64,
) -> Result<bool> {
    invoke_validate_proof(
        prover,
        accounts,
        ValidateProofArgs::new(intent_hash, destination, None),
    )
}

pub fn invoke_validate_proof<'info>(
    prover: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    args: ValidateProofArgs,
) -> Result<bool> {
    let mut data = VALIDATE_PROOF_DISCRIMINATOR.to_vec();
    args.serialize(&mut data)?;

    let instruction = Instruction {
        program_id: prover.key(),
        accounts: accounts
            .iter()
            .map(|account| AccountMeta::new_readonly(account.key(), false))
            .collect(),
        data,
    };
    invoke(&instruction, accounts)?;

    let (program, data) = get_return_data().ok_or(ProverError::InvalidReturnData)?;
    require_keys_eq!(program, prover.key(), ProverError::InvalidReturnData);

    match data.as_slice() {
        [0] => Ok(false),
        [1] => Ok(true),
        _ => err!(ProverError::InvalidReturnData),
    }
}

pub fn close_proof<'info>(
    prover: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    intent_hash: Bytes32,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut data = CLOSE_PROOF_DISCRIMINATOR.to_vec();
    intent_hash.serialize(&mut data)?;

    let instruction = Instruction {
        program_id: prover.key(),
        accounts: std::iter::once(AccountMeta::new_readonly(authority.key(), true))
            .chain(accounts.iter().map(|account| AccountMeta {
                pubkey: account.key(),
                is_signer: account.is_signer,
                is_writable: account.is_writable,
            }))
            .collect(),
        data,
    };
    let infos = std::iter::once(authority.clone())
        .chain(accounts.iter().cloned())
        .collect::<Vec<_>>();

    invoke_signed(&instruction, &infos, signer_seeds).map_err(Into::into)
}
