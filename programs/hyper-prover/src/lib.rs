use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("B4pMQaAGPZ7Mza9XnDxJfXZ1cUa4aa67zrNkv8zYAjx4");

pub mod hyperlane;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod hyper_prover {
    use super::*;

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }

    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }

    #[instruction(discriminator = &hyperlane::HANDLE_DISCRIMINATOR)]
    pub fn handle<'info>(
        ctx: Context<'_, '_, '_, 'info, Handle<'info>>,
        origin: u32,
        sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        instructions::handle(ctx, origin, sender, payload)
    }

    #[instruction(discriminator = &hyperlane::HANDLE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn handle_account_metas(
        ctx: Context<HandleAccountMetas>,
        origin: u32,
        sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        instructions::handle_account_metas(ctx, origin, sender, payload)
    }

    /// Called by Hyperlane to discover this recipient's ISM. Returns `None`,
    /// indicating that the mailbox's default ISM should be used for message
    /// verification rather than a custom ISM.
    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR)]
    pub fn ism(ctx: Context<Ism>) -> Result<()> {
        instructions::ism(ctx)
    }

    /// Called by Hyperlane to discover the accounts required by this
    /// recipient's ISM. Returns an empty list because no custom ISM is
    /// configured; the mailbox's default ISM is used instead.
    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn ism_account_metas(ctx: Context<IsmAccountMetas>) -> Result<()> {
        instructions::ism_account_metas(ctx)
    }
}
