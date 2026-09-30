use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::{get_return_data, invoke, invoke_signed};

use super::{
    CloseProofArgs, GetProofArgs, Proof, ProveArgs, ProverError, CLOSE_PROOF_DISCRIMINATOR,
    GET_PROOF_DISCRIMINATOR, PROVE_DISCRIMINATOR,
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

    read_proof_return_data(&prover.key())
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
        accounts: iter::once(AccountMeta::new_readonly(authority.key(), true))
            .chain(accounts.iter().map(|account| AccountMeta {
                pubkey: account.key(),
                is_signer: account.is_signer,
                is_writable: account.is_writable,
            }))
            .collect(),
        data,
    };
    let infos = iter::once(authority.clone())
        .chain(accounts.iter().cloned())
        .collect::<Vec<_>>();

    invoke_signed(&instruction, &infos, signer_seeds).map_err(Into::into)
}

/// CPIs a prover program's `prove` instruction for a single intent.
///
/// Generic over any prover that follows the standard `prove(ProveArgs)` shape
/// (local-prover, hyper-prover, etc.). `caller` is signed via `caller_seeds`, so
/// it must be an authority the prover accepts — `portal::state::dispatcher_pda(prover)`
/// or `flash_fulfiller::state::prove_authority_pda(prover)`, both scoped to the
/// prover being dispatched to.
///
/// Never pass a fund-holding or claimant identity (e.g. `flash_vault`) as
/// `caller`. `prover_program` is caller-chosen at every current call site, so
/// whatever signs here is handed to a program the caller picked; scoping the
/// authority to that program is what makes the signature useless to it. A
/// credential that also controls funds would hand over both.
///
/// The instruction built here is a fixed six accounts and forwards no tail, so a
/// caller-chosen `prover_program` receives no other program to replay `caller`
/// into. That is load-bearing — see `flash_fulfill_confused_deputy.rs`.
#[allow(clippy::too_many_arguments)]
pub fn prove<'info>(
    prover_program: &AccountInfo<'info>,
    caller: &AccountInfo<'info>,
    caller_seeds: &[&[u8]],
    payer: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    event_authority: &AccountInfo<'info>,
    proof: &AccountInfo<'info>,
    args: ProveArgs,
) -> Result<()> {
    let mut data = PROVE_DISCRIMINATOR.to_vec();
    args.serialize(&mut data)?;

    let accounts = vec![
        AccountMeta::new_readonly(caller.key(), true),
        AccountMeta::new(payer.key(), true),
        AccountMeta::new_readonly(system_program.key(), false),
        AccountMeta::new_readonly(event_authority.key(), false),
        AccountMeta::new_readonly(prover_program.key(), false),
        AccountMeta::new(proof.key(), false),
    ];

    let infos = [
        caller.to_account_info(),
        payer.to_account_info(),
        system_program.to_account_info(),
        event_authority.to_account_info(),
        prover_program.to_account_info(),
        proof.to_account_info(),
    ];

    let instruction = Instruction {
        program_id: prover_program.key(),
        accounts,
        data,
    };

    invoke_signed(&instruction, &infos, &[caller_seeds]).map_err(Into::into)
}

fn read_proof_return_data(prover: &Pubkey) -> Result<Option<Proof>> {
    let (program, data) = get_return_data().ok_or(ProverError::InvalidReturnData)?;
    require_keys_eq!(program, *prover, ProverError::InvalidReturnData);

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
