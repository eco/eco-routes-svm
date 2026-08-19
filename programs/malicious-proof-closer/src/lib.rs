use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use eco_svm_std::prover::CLOSE_PROOF_DISCRIMINATOR;

declare_id!("3AArgehkyg8pPZUEfSQqZEp9WNJLCQStdWjz9HcrVPTp");

/// Test-only stand-in "prover" for the per-prover proof_closer scoping regression
/// (localnet only; excluded from devnet/mainnet builds). `portal::withdraw` accepts
/// it as `reward.prover`, then CPIs its `close_proof` with the portal `proof_closer`
/// PDA as an inherited signer and the withdraw call's forwarded remaining accounts.
/// This handler re-CPIs the real local-prover's `close_proof` with the inherited
/// signer to close a proof it does not own. With `proof_closer` scoped per-prover,
/// local-prover rejects the forwarded signer.
///
/// `close_proof` shares the standard Anchor discriminator with the real provers'
/// instruction (`sha256("global:close_proof")[..8]`), so `portal::withdraw`'s CPI
/// dispatches here.
#[program]
pub mod malicious_proof_closer {
    use super::*;

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        let proof_closer = &ctx.accounts.proof_closer;
        let local_prover = &ctx.accounts.local_prover;
        let target_proof = &ctx.accounts.target_proof;
        let payer = &ctx.accounts.payer;

        msg!("malicious-proof-closer: re-CPI local-prover close_proof via forwarded proof_closer signer");

        // local-prover `CloseProof` order: portal_proof_closer, proof, payer. The
        // proof_closer signature flows through this plain `invoke` because it is a
        // signer of the current instruction.
        let metas = vec![
            AccountMeta::new_readonly(proof_closer.key(), true),
            AccountMeta::new(target_proof.key(), false),
            AccountMeta::new(payer.key(), true),
        ];
        let infos = [
            proof_closer.to_account_info(),
            target_proof.to_account_info(),
            payer.to_account_info(),
        ];

        invoke(
            &Instruction {
                program_id: local_prover.key(),
                accounts: metas,
                data: CLOSE_PROOF_DISCRIMINATOR.to_vec(),
            },
            &infos,
        )
        .map_err(Into::into)
    }
}

/// Account order as `portal::withdraw` builds the `close_proof` CPI: `proof_closer`
/// and the withdrawn intent's own `proof`, then the forwarded remaining accounts
/// (here: the real local-prover program, the target proof PDA, and the rent payer).
#[derive(Accounts)]
pub struct CloseProof<'info> {
    /// CHECK: inherited signer, forwarded verbatim to the nested CPI
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: the withdrawn intent's own proof (unused)
    pub own_proof: UncheckedAccount<'info>,
    /// CHECK: the real local-prover program (nested CPI target)
    pub local_prover: UncheckedAccount<'info>,
    /// CHECK: the proof this program does not own
    #[account(mut)]
    pub target_proof: UncheckedAccount<'info>,
    /// CHECK: rent recipient of the closed account
    #[account(mut)]
    pub payer: UncheckedAccount<'info>,
}
