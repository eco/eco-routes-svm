use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::{invoke, set_return_data};
use eco_svm_std::prover::{
    CloseProofArgs, GetProofArgs, Proof, CLOSE_PROOF_DISCRIMINATOR, GET_PROOF_DISCRIMINATOR,
};
use eco_svm_std::Bytes32;

declare_id!("3AArgehkyg8pPZUEfSQqZEp9WNJLCQStdWjz9HcrVPTp");

/// Localnet fixture for malformed validation responses and replayed cleanup authority.
#[program]
pub mod malicious_proof_closer {
    use super::*;

    pub fn get_proof<'info>(
        ctx: Context<'info, GetProof<'info>>,
        args: GetProofArgs,
    ) -> Result<()> {
        let data = ctx.accounts.proof.try_borrow_data()?;
        if data.len() != 1 {
            drop(data);
            let valid = Proof::get(&ctx.accounts.proof, &crate::ID, args)?;
            set_return_data(&borsh::to_vec(&valid)?);

            return Ok(());
        }
        match data[0] {
            0 => (),
            1 => set_return_data(&[2]),
            2 => set_return_data(&[0, 0]),
            3 => {
                let (program, accounts) = ctx
                    .remaining_accounts
                    .split_first()
                    .ok_or(ProgramError::NotEnoughAccountKeys)?;
                let mut data = GET_PROOF_DISCRIMINATOR.to_vec();
                args.serialize(&mut data)?;
                invoke(
                    &Instruction {
                        program_id: program.key(),
                        accounts: accounts
                            .iter()
                            .map(|account| AccountMeta::new_readonly(account.key(), false))
                            .collect(),
                        data,
                    },
                    accounts,
                )?;
            }
            4 => return Err(ProgramError::InvalidInstructionData.into()),
            6..=9 => {
                let proof = Proof::new(
                    0,
                    if data[0] == 7 {
                        Pubkey::default()
                    } else {
                        crate::ID
                    },
                );
                let mut response = borsh::to_vec(&Some(proof))?;
                match data[0] {
                    8 => {
                        response.pop();
                    }
                    9 => response.push(0),
                    _ => (),
                }
                set_return_data(&response);
            }
            _ => {
                require!(
                    !ctx.accounts.proof.is_signer
                        && !ctx.accounts.proof.is_writable
                        && ctx
                            .remaining_accounts
                            .iter()
                            .all(|account| !account.is_signer && !account.is_writable),
                    anchor_lang::error::ErrorCode::ConstraintRaw
                );
                set_return_data(&[0]);
            }
        }

        Ok(())
    }

    pub fn close_proof(ctx: Context<CloseProof>, args: CloseProofArgs) -> Result<()> {
        let data = ctx.accounts.own_proof.try_borrow_data()?;
        let intent_hash = if data.len() == 32 {
            Bytes32::try_from_slice(&data)?
        } else {
            args.intent_hash
        };
        let mut data = CLOSE_PROOF_DISCRIMINATOR.to_vec();
        CloseProofArgs::new(intent_hash, vec![]).serialize(&mut data)?;
        let accounts = vec![
            AccountMeta::new_readonly(ctx.accounts.proof_closer.key(), true),
            AccountMeta::new(ctx.accounts.target_proof.key(), false),
            AccountMeta::new(ctx.accounts.payer.key(), ctx.accounts.payer.is_signer),
        ];
        let infos = [
            ctx.accounts.proof_closer.to_account_info(),
            ctx.accounts.target_proof.to_account_info(),
            ctx.accounts.payer.to_account_info(),
        ];
        invoke(
            &Instruction {
                program_id: ctx.accounts.prover.key(),
                accounts,
                data,
            },
            &infos,
        )
        .map_err(Into::into)
    }
}

#[derive(Accounts)]
pub struct GetProof<'info> {
    /// CHECK: proof or test-controlled response mode.
    pub proof: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CloseProof<'info> {
    /// CHECK: inherited authority deliberately forwarded without checking it.
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: optional substituted hash controlled by the test.
    pub own_proof: UncheckedAccount<'info>,
    /// CHECK: nested CPI target controlled by the test.
    pub prover: UncheckedAccount<'info>,
    /// CHECK: victim proof controlled by the test.
    #[account(mut)]
    pub target_proof: UncheckedAccount<'info>,
    /// CHECK: leaf rent recipient, with its original signer privileges.
    #[account(mut)]
    pub payer: UncheckedAccount<'info>,
}
