use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;

pub trait AccountExt: AccountSerialize + AccountDeserialize + Owner + Space {
    fn init<'info>(
        self,
        account: &AccountInfo<'info>,
        payer: &AccountInfo<'info>,
        system_program: &Program<'info, System>,
        signer_seeds: &[&[&[u8]]],
    ) -> Result<()> {
        create_account(
            account,
            payer,
            &system_program.to_account_info(),
            &Self::owner(),
            8 + Self::INIT_SPACE,
            signer_seeds,
        )?;

        self.try_serialize(&mut &mut account.try_borrow_mut_data()?[..])?;

        Ok(())
    }
}

/// Allocates `data_len` bytes at `account` owned by `program_id`. Falls back to
/// transfer + allocate + assign when the PDA was pre-funded by a griefer, so a
/// stray lamport transfer cannot block account creation.
pub fn create_account<'info>(
    account: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    program_id: &Pubkey,
    data_len: usize,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    require!(
        account.data_is_empty() && *account.owner != *program_id,
        anchor_lang::error::ErrorCode::ConstraintZero
    );

    let min_balance = Rent::get()?.minimum_balance(data_len);

    match account.lamports() {
        0 => invoke_signed(
            &system_instruction::create_account(
                &payer.key(),
                &account.key(),
                min_balance,
                data_len as u64,
                program_id,
            ),
            &[
                payer.to_account_info(),
                account.to_account_info(),
                system_program.to_account_info(),
            ],
            signer_seeds,
        )?,
        existing => {
            if let Some(amount) = min_balance
                .checked_sub(existing)
                .filter(|amount| *amount > 0)
            {
                invoke_signed(
                    &system_instruction::transfer(&payer.key(), &account.key(), amount),
                    &[
                        payer.to_account_info(),
                        account.to_account_info(),
                        system_program.to_account_info(),
                    ],
                    signer_seeds,
                )?;
            }

            invoke_signed(
                &system_instruction::allocate(&account.key(), data_len as u64),
                &[account.to_account_info(), system_program.to_account_info()],
                signer_seeds,
            )?;
            invoke_signed(
                &system_instruction::assign(&account.key(), program_id),
                &[account.to_account_info(), system_program.to_account_info()],
                signer_seeds,
            )?;
        }
    }

    Ok(())
}
