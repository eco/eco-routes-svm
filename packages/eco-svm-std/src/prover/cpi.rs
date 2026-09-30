use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::{get_return_data, invoke, invoke_signed};

use super::{
    CloseProofArgs, GetProofArgs, Proof, ProverError, CLOSE_PROOF_DISCRIMINATOR,
    GET_PROOF_DISCRIMINATOR,
};

pub fn get_proof<'info>(
    prover: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    args: GetProofArgs,
) -> Result<Option<Proof>> {
    let mut data = GET_PROOF_DISCRIMINATOR.to_vec();
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

    let proof =
        Option::<Proof>::try_from_slice(&data).map_err(|_| ProverError::InvalidReturnData)?;
    require!(
        proof
            .as_ref()
            .is_none_or(|proof| proof.claimant != Pubkey::default()),
        ProverError::InvalidReturnData
    );

    Ok(proof)
}

pub fn close_proof<'info>(
    prover: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    args: CloseProofArgs,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut data = CLOSE_PROOF_DISCRIMINATOR.to_vec();
    args.serialize(&mut data)?;

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
