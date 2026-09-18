use anchor_lang::prelude::*;
use eco_svm_std::prover::{self, IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::{Bytes32, CHAIN_ID};

use crate::state::{prove_authority_pda, PROVE_AUTHORITY_SEED};

/// CPIs `prover_program.prove` to credit `claimant` for a same-chain intent.
///
/// Signs as the prove-only credential scoped to `prover_program`'s own key, so
/// no other prover accepts it. `claimant` must be the account the caller then
/// passes to [`withdraw_intent`]; portal rejects the withdrawal otherwise.
///
/// [`withdraw_intent`]: super::withdraw::withdraw_intent
#[allow(clippy::too_many_arguments)]
pub fn prove_intent<'info>(
    prover_program: &AccountInfo<'info>,
    prove_authority: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    event_authority: &AccountInfo<'info>,
    proof: &AccountInfo<'info>,
    intent_hash: Bytes32,
    claimant: &Pubkey,
) -> Result<()> {
    let prover = prover_program.key();
    let (_, bump) = prove_authority_pda(&prover);

    prover::prove(
        prover_program,
        prove_authority,
        &[PROVE_AUTHORITY_SEED, prover.as_ref(), &[bump]],
        payer,
        system_program,
        event_authority,
        proof,
        ProveArgs {
            domain_id: CHAIN_ID,
            proof_data: ProofData {
                destination: CHAIN_ID,
                intent_hashes_claimants: vec![IntentHashClaimant {
                    intent_hash,
                    claimant: claimant.to_bytes().into(),
                }],
            },
            data: vec![],
        },
    )
}
