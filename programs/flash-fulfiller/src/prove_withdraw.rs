//! The same-chain prove → withdraw credit shared by both fulfillment paths.

use anchor_lang::prelude::*;
use eco_svm_std::prover::{self, IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::instructions::WithdrawArgs;
use portal::types::{Reward, TokenTransferAccounts};

use crate::cpi;
use crate::state::{prove_authority_pda, PROVE_AUTHORITY_SEED};

/// Accounts both legs of one credit share.
pub(crate) struct ProveWithdraw<'a, 'info> {
    pub payer: &'a AccountInfo<'info>,
    /// Named in the proof and paid by the withdrawal. `flash_fulfill` passes its
    /// `flash_vault` PDA; `prove_and_withdraw` passes the solver.
    pub claimant: &'a AccountInfo<'info>,
    pub proof: &'a AccountInfo<'info>,
    pub intent_vault: &'a AccountInfo<'info>,
    pub withdrawn_marker: &'a AccountInfo<'info>,
    pub proof_closer: &'a AccountInfo<'info>,
    pub portal_program: &'a AccountInfo<'info>,
    pub local_prover_program: &'a AccountInfo<'info>,
    pub prove_authority: &'a AccountInfo<'info>,
    pub local_prover_event_authority: &'a AccountInfo<'info>,
    pub token_program: &'a AccountInfo<'info>,
    pub token_2022_program: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
}

impl<'info> ProveWithdraw<'_, 'info> {
    /// Mints a local proof naming `claimant`, then withdraws the reward to it.
    ///
    /// One `claimant` feeds both legs: portal pays whoever the proof names, so
    /// the two must never diverge.
    ///
    /// The signed `prove_authority` is scoped to `local_prover_program`'s own
    /// key, so no other prover accepts it. Both legs share one `proof` account,
    /// which pins `reward.prover` to that same program before portal forwards
    /// anything to it.
    pub(crate) fn execute(
        self,
        intent_hash: Bytes32,
        route_hash: Bytes32,
        reward: Reward,
        reward_transfers: &[TokenTransferAccounts<'info>],
    ) -> Result<()> {
        let local_prover = self.local_prover_program.key();
        let (_, bump) = prove_authority_pda(&local_prover);
        let seeds: &[&[u8]] = &[PROVE_AUTHORITY_SEED, local_prover.as_ref(), &[bump]];

        prover::prove(
            self.local_prover_program,
            self.prove_authority,
            seeds,
            self.payer,
            self.system_program,
            self.local_prover_event_authority,
            self.proof,
            ProveArgs {
                domain_id: CHAIN_ID,
                proof_data: ProofData {
                    destination: CHAIN_ID,
                    intent_hashes_claimants: vec![IntentHashClaimant {
                        intent_hash,
                        claimant: self.claimant.key().to_bytes().into(),
                    }],
                },
                data: vec![],
            },
        )?;

        cpi::withdraw::withdraw_intent(
            self.portal_program,
            self.payer,
            self.claimant,
            self.intent_vault,
            self.proof,
            self.proof_closer,
            self.local_prover_program,
            self.withdrawn_marker,
            self.token_program,
            self.token_2022_program,
            self.system_program,
            reward_transfers,
            WithdrawArgs {
                destination: CHAIN_ID,
                route_hash,
                reward,
            },
        )
    }
}
