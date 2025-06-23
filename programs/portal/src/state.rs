use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use eco_svm_std::Bytes32;

pub const VAULT_SEED: &[u8] = b"vault";
pub const CLAIMED_MARKER_SEED: &[u8] = b"claimed_marker";

pub fn vault_pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, intent_hash.as_ref()], &crate::ID)
}

#[account]
#[derive(InitSpace, Debug)]
pub struct WithdrawnMarker {}

impl WithdrawnMarker {
    pub fn pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CLAIMED_MARKER_SEED, intent_hash.as_ref()], &crate::ID)
    }

    fn data_len() -> usize {
        8 + Self::INIT_SPACE
    }

    pub fn min_balance(rent: Rent) -> u64 {
        rent.minimum_balance(Self::data_len())
    }

    pub fn init<'info>(
        claimed_marker: &AccountInfo<'info>,
        payer: &Signer<'info>,
        system_program: &Program<'info, System>,
        signer_seeds: &[&[u8]],
    ) -> Result<()> {
        let min_balance = Self::min_balance(Rent::get()?);

        match claimed_marker.lamports() {
            0 => {
                invoke_signed(
                    &system_instruction::create_account(
                        &payer.key(),
                        &claimed_marker.key(),
                        min_balance,
                        Self::data_len() as u64,
                        &crate::ID,
                    ),
                    &[
                        payer.to_account_info(),
                        claimed_marker.to_account_info(),
                        system_program.to_account_info(),
                    ],
                    &[signer_seeds],
                )?;
            }
            vault_balance => {
                if let Some(amount) = min_balance
                    .checked_sub(vault_balance)
                    .filter(|amount| *amount > 0)
                {
                    invoke_signed(
                        &system_instruction::transfer(&payer.key(), &claimed_marker.key(), amount),
                        &[
                            payer.to_account_info(),
                            claimed_marker.to_account_info(),
                            system_program.to_account_info(),
                        ],
                        &[signer_seeds],
                    )?;
                }

                invoke_signed(
                    &system_instruction::allocate(&claimed_marker.key(), Self::data_len() as u64),
                    &[
                        claimed_marker.to_account_info(),
                        system_program.to_account_info(),
                    ],
                    &[signer_seeds],
                )?;
                invoke_signed(
                    &system_instruction::assign(&claimed_marker.key(), &crate::ID),
                    &[
                        claimed_marker.to_account_info(),
                        system_program.to_account_info(),
                    ],
                    &[signer_seeds],
                )?;
            }
        }

        Self {}.try_serialize(&mut &mut claimed_marker.try_borrow_mut_data()?[..])?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{self, Reward, TokenAmount};

    #[test]
    fn vault_pda_deterministic() {
        let destination_chain = [5u8; 32].into();
        let route_hash = [6u8; 32].into();
        let reward = Reward {
            deadline: 1640995200,
            creator: Pubkey::new_from_array([1u8; 32]),
            prover: Pubkey::new_from_array([2u8; 32]),
            native_amount: 1_000_000_000,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 100,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([4u8; 32]),
                    amount: 200,
                },
            ],
        };

        goldie::assert_json!(vault_pda(&types::intent_hash(
            &destination_chain,
            &route_hash,
            &reward.hash(),
        )));
    }

    #[test]
    fn withdrawn_marker_pda_deterministic() {
        let destination_chain = [5u8; 32].into();
        let route_hash = [6u8; 32].into();
        let reward = Reward {
            deadline: 1640995200,
            creator: Pubkey::new_from_array([1u8; 32]),
            prover: Pubkey::new_from_array([2u8; 32]),
            native_amount: 1_000_000_000,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 100,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([4u8; 32]),
                    amount: 200,
                },
            ],
        };

        goldie::assert_json!(WithdrawnMarker::pda(&types::intent_hash(
            &destination_chain,
            &route_hash,
            &reward.hash(),
        )));
    }

    #[test]
    fn withdrawn_marker_min_balance_deterministic() {
        let rent = Rent {
            lamports_per_byte_year: 3480,
            exemption_threshold: 2.0,
            burn_percent: 50,
        };

        goldie::assert_json!(WithdrawnMarker::min_balance(rent));
    }
}
