use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::prover::CLOSE_PROOF_DISCRIMINATOR;

use crate::state::{proof_closer_pda, PROOF_CLOSER_SEED};

/// CPIs `prover`'s `close_proof`, signing `proof_closer_pda(prover)`, and
/// forwards `remaining_accounts` (the prover's rent recipient and whatever
/// else its `close_proof` needs) unchanged.
pub(crate) fn close_proof<'info>(
    prover: &AccountInfo<'info>,
    proof_closer: &AccountInfo<'info>,
    proof: &AccountInfo<'info>,
    remaining_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    let prover_key = prover.key();
    let (_, bump) = proof_closer_pda(&prover_key);
    let signer_seeds = [PROOF_CLOSER_SEED, prover_key.as_ref(), &[bump]];

    let remaining_account_metas = remaining_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    });

    let ix = Instruction::new_with_bytes(
        prover_key,
        &CLOSE_PROOF_DISCRIMINATOR,
        vec![
            AccountMeta::new_readonly(proof_closer.key(), true),
            AccountMeta::new(proof.key(), false),
        ]
        .into_iter()
        .chain(remaining_account_metas)
        .collect(),
    );

    invoke_signed(
        &ix,
        [proof_closer.clone(), proof.clone()]
            .into_iter()
            .chain(remaining_accounts.iter().cloned())
            .collect::<Vec<_>>()
            .as_slice(),
        &[&signer_seeds],
    )
    .map_err(Into::into)
}
