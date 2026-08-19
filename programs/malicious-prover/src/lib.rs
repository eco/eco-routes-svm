use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use eco_svm_std::prover::{ProveArgs, PROVE_DISCRIMINATOR};

declare_id!("HuYH6b8g196g3Zi5nvSadhQLBYMnLsy5RCkx4xmCXxoE");

/// Test-only stand-in "prover" for the per-prover dispatcher scoping regression
/// (localnet only; excluded from devnet/mainnet builds). `portal::prove` invokes
/// it with the dispatcher PDA as an inherited signer, then it re-CPIs the real
/// local-prover with `ProofData` taken from `portal::prove`'s verbatim `data`
/// passthrough. With the dispatcher scoped per-prover, local-prover rejects the
/// forwarded signer.
///
/// `prove` shares the standard Anchor discriminator with the real provers'
/// instruction (`sha256("global:prove")[..8]`), so `portal::prove`'s CPI
/// dispatches here.
#[program]
pub mod malicious_prover {
    use super::*;

    pub fn prove<'info>(
        ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
        args: ProveArgs,
    ) -> Result<()> {
        let caller = &ctx.accounts.caller;
        let payer = &ctx.accounts.payer;
        let system_program = &ctx.accounts.system_program;
        let event_authority = &ctx.accounts.event_authority;
        // tail: the real local-prover program, then the proofs to mint
        let (local_prover, proofs) = ctx
            .remaining_accounts
            .split_first()
            .ok_or(ProgramError::NotEnoughAccountKeys)?;

        msg!("malicious-prover: re-CPI local-prover via forwarded caller signer");

        // `args.data` is a caller-supplied borsh local-prover `ProveArgs`. local-prover
        // `Prove` order: caller, payer, system_program, event_authority, program,
        // [remaining: proofs]. The dispatcher signature flows through this plain
        // `invoke` because it is a signer of the current instruction.
        let mut data = PROVE_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&args.data);

        let metas = [
            AccountMeta::new_readonly(caller.key(), true),
            AccountMeta::new(payer.key(), true),
            AccountMeta::new_readonly(system_program.key(), false),
            AccountMeta::new_readonly(event_authority.key(), false),
            AccountMeta::new_readonly(local_prover.key(), false),
        ]
        .into_iter()
        .chain(
            proofs
                .iter()
                .map(|proof| AccountMeta::new(proof.key(), false)),
        )
        .collect::<Vec<_>>();
        let infos = [
            caller.to_account_info(),
            payer.to_account_info(),
            system_program.to_account_info(),
            event_authority.to_account_info(),
            local_prover.to_account_info(),
        ]
        .into_iter()
        .chain(proofs.iter().cloned())
        .collect::<Vec<_>>();

        invoke(
            &Instruction {
                program_id: local_prover.key(),
                accounts: metas,
                data,
            },
            &infos,
        )
        .map_err(Into::into)
    }
}

/// Account order as `portal::prove` builds the CPI: the dispatcher PDA it prepends,
/// then the caller-supplied `prove_accounts` tail (the real local-prover program,
/// payer, system program, event authority, then the target proof PDAs).
#[derive(Accounts)]
/// Mirrors `eco_svm_std::prover::prove`'s fixed account order, so this program is
/// reachable from **both** dispatch paths: portal's `invoke_prover_prove` (where
/// the caller orders the tail) and flash-fulfiller's prove leg (where the order
/// is fixed by the helper). The nested CPI target comes from the tail rather than
/// a fixed slot, because slot 4 is the callee — i.e. this program.
pub struct Prove<'info> {
    /// CHECK: inherited signer, forwarded verbatim to the nested CPI
    pub caller: UncheckedAccount<'info>,
    /// CHECK: payer / rent source for the minted proofs
    #[account(mut)]
    pub payer: UncheckedAccount<'info>,
    /// CHECK: system program
    pub system_program: UncheckedAccount<'info>,
    /// CHECK: local-prover event authority
    pub event_authority: UncheckedAccount<'info>,
    /// CHECK: this program (the dispatch target), unused
    pub this_program: UncheckedAccount<'info>,
}
