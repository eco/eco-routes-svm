//! Transaction-level authorization for the split fulfillment path.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program_error::ProgramError;
use anchor_lang::Discriminator;
use eco_svm_std::Bytes32;
use portal::state::FulfillMarker;
// Anchor 1.1.2 does not re-export the checked loaders; use its underlying crate.
use solana_instructions_sysvar::{load_current_index_checked, load_instruction_at_checked};

use crate::instructions::FlashFulfillerError;

const FULFILL_DISCRIMINATOR: &[u8] = <portal::instruction::Fulfill as Discriminator>::DISCRIMINATOR;
const _: () = assert!(FULFILL_DISCRIMINATOR.len() == 8);

/// Requires another top-level `portal.fulfill` for this intent and solver.
///
/// The Instructions sysvar contains only TOP-LEVEL instructions: a fulfill
/// nested under another program cannot authorize withdrawal. Both orders are
/// allowed. An earlier fulfill has already succeeded; a later fulfill must
/// succeed or Solana rolls back the entire transaction, including withdrawal.
/// Skip the current top-level index even when this handler is called by CPI.
///
/// Only the program, discriminator, intent hash, solver signer and marker PDA
/// are checked here. Portal validates the complete args, route commitment,
/// deadline, transfers and calls when fulfill executes. Its claimant need not
/// equal the solver: the reward recipient is the solver proven by this handler.
/// A matching prefix with an invalid tail cannot commit because portal rejects it.
pub(crate) fn require_paired_fulfill(
    instructions: &AccountInfo,
    intent_hash: &Bytes32,
    solver: &Pubkey,
) -> Result<()> {
    let current = load_current_index_checked(instructions)?;
    let marker = FulfillMarker::pda(intent_hash).0;
    for index in 0..=u16::MAX {
        if index == current {
            continue;
        }
        match load_instruction_at_checked(usize::from(index), instructions) {
            Ok(instruction) if matches_fulfill(&instruction, intent_hash, solver, &marker) => {
                return Ok(());
            }
            Ok(_) => {}
            // The checked loader maps an index beyond the instruction count
            // to InvalidArgument. Other sysvar errors must propagate.
            Err(ProgramError::InvalidArgument) => break,
            Err(error) => return Err(error.into()),
        }
    }
    err!(FlashFulfillerError::MissingPairedFulfill)
}

fn matches_fulfill(
    instruction: &Instruction,
    intent_hash: &Bytes32,
    solver: &Pubkey,
    marker: &Pubkey,
) -> bool {
    instruction.program_id == portal::ID
        && instruction.data.get(..8) == Some(FULFILL_DISCRIMINATOR)
        && instruction.data.get(8..40) == Some(intent_hash.as_ref())
        && instruction
            .accounts
            .get(1)
            .is_some_and(|account| account.pubkey == *solver && account.is_signer)
        && instruction
            .accounts
            .get(3)
            .is_some_and(|account| account.pubkey == *marker)
}

#[cfg(test)]
mod tests {
    use anchor_lang::solana_program::instruction::{BorrowedAccountMeta, BorrowedInstruction};
    use anchor_lang::{InstructionData, ToAccountMetas};
    use portal::instructions::FulfillArgs;
    use portal::types::Route;
    use solana_instructions_sysvar::{
        construct_instructions_data, store_current_index_checked, ID,
    };

    use super::*;

    fn hash() -> Bytes32 {
        [42; 32].into()
    }
    const SOLVER: Pubkey = Pubkey::new_from_array([5; 32]);

    fn fulfill() -> Instruction {
        Instruction {
            program_id: portal::ID,
            accounts: portal::accounts::Fulfill {
                payer: Pubkey::new_unique(),
                solver: SOLVER,
                executor: portal::state::executor_pda().0,
                fulfill_marker: FulfillMarker::pda(&hash()).0,
                token_program: anchor_spl::token::ID,
                token_2022_program: anchor_spl::token_2022::ID,
                associated_token_program: anchor_spl::associated_token::ID,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: portal::instruction::Fulfill {
                args: FulfillArgs {
                    intent_hash: hash(),
                    route: Route {
                        salt: [17; 32].into(),
                        deadline: 123,
                        portal: portal::ID.to_bytes().into(),
                        native_amount: 456,
                        tokens: vec![],
                        calls: vec![],
                    },
                    reward_hash: [18; 32].into(),
                    claimant: [19; 32].into(),
                },
            }
            .data(),
        }
    }

    fn unrelated() -> Instruction {
        Instruction {
            program_id: crate::ID,
            accounts: vec![],
            data: vec![],
        }
    }

    fn check(instructions: &[Instruction], current: u16, key: Pubkey) -> Result<()> {
        let borrowed: Vec<_> = instructions
            .iter()
            .map(|ix| BorrowedInstruction {
                program_id: &ix.program_id,
                accounts: ix
                    .accounts
                    .iter()
                    .map(|meta| BorrowedAccountMeta {
                        pubkey: &meta.pubkey,
                        is_signer: meta.is_signer,
                        is_writable: meta.is_writable,
                    })
                    .collect(),
                data: &ix.data,
            })
            .collect();
        let mut data = construct_instructions_data(&borrowed);
        store_current_index_checked(&mut data, current).unwrap();
        let mut lamports = 0;
        let owner = Pubkey::default();
        require_paired_fulfill(
            &AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false),
            &hash(),
            &SOLVER,
        )
    }

    fn assert_missing(instructions: &[Instruction], current: u16) {
        assert_eq!(
            check(instructions, current, ID).unwrap_err(),
            error!(FlashFulfillerError::MissingPairedFulfill)
        );
    }

    #[test]
    fn fulfill_abi_pins_hash_offset_and_account_indices() {
        // Use generated Anchor serialization/metas, not a hand-built lookalike.
        // Reordering FulfillArgs or Fulfill accounts must break this test.
        let ix = fulfill();
        assert_eq!(&ix.data[..8], FULFILL_DISCRIMINATOR);
        assert_eq!(&ix.data[8..40], hash().as_ref());
        assert_eq!(ix.accounts[1].pubkey, SOLVER);
        assert!(ix.accounts[1].is_signer);
        assert_eq!(ix.accounts[3].pubkey, FulfillMarker::pda(&hash()).0);
        assert!(matches_fulfill(
            &ix,
            &hash(),
            &SOLVER,
            &FulfillMarker::pda(&hash()).0
        ));
    }

    #[test]
    fn scans_before_and_after_current_across_unrelated_instructions() {
        check(&[fulfill(), unrelated(), unrelated()], 2, ID).unwrap();
        check(&[unrelated(), unrelated(), fulfill()], 0, ID).unwrap();
        check(&[unrelated(), fulfill(), unrelated()], 0, ID).unwrap();
    }

    #[test]
    fn skips_current_even_if_it_looks_like_fulfill() {
        assert_missing(&[fulfill()], 0);
        assert_missing(&[unrelated(), fulfill(), unrelated()], 1);
    }

    #[test]
    fn rejects_missing_pair() {
        assert_missing(&[unrelated()], 0);
        assert_missing(&[unrelated(), unrelated()], 1);
    }

    #[test]
    fn rejects_each_mismatched_field() {
        let original = fulfill();
        let mut candidates = vec![original.clone(); 6];
        candidates[0].program_id = crate::ID;
        candidates[1].data[0] ^= 1;
        candidates[2].data[8] ^= 1;
        candidates[3].accounts[1].pubkey = Pubkey::new_unique();
        candidates[4].accounts[1].is_signer = false;
        candidates[5].accounts[3].pubkey = Pubkey::new_unique();
        for ix in candidates {
            assert_missing(&[unrelated(), ix], 0);
        }
    }

    #[test]
    fn all_short_data_and_account_prefixes_fail_without_panicking() {
        for len in 0..40 {
            let mut ix = fulfill();
            ix.data.truncate(len);
            assert_missing(&[unrelated(), ix], 0);
        }
        for len in 0..4 {
            let mut ix = fulfill();
            ix.accounts.truncate(len);
            assert_missing(&[unrelated(), ix], 0);
        }
    }

    #[test]
    fn ignores_bad_candidates_and_finds_a_later_match() {
        let mut bad = fulfill();
        bad.data.truncate(39);
        check(&[unrelated(), bad, unrelated(), fulfill()], 0, ID).unwrap();
    }

    #[test]
    fn rejects_forged_sysvar_address() {
        assert_eq!(
            check(&[unrelated(), fulfill()], 0, Pubkey::new_unique()).unwrap_err(),
            ProgramError::UnsupportedSysvar.into(),
        );
    }

    #[test]
    fn existing_error_codes_stay_stable() {
        assert_eq!(u32::from(FlashFulfillerError::InvalidPortalProgram), 6009);
        assert_eq!(u32::from(FlashFulfillerError::MissingPairedFulfill), 6010);
    }
}
