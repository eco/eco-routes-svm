use anchor_lang::prelude::*;

declare_id!("EcoZ3pDi8PJf9ohgg6HJdgPviSqQCnbgD14KCnE4rEZm");

pub mod constants;
pub mod instructions;
pub mod layerzero;
pub mod state;

use instructions::*;

#[program]
pub mod layerzero_prover {
    use super::*;

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn init_path(ctx: Context<InitPath>, eid: u32) -> Result<()> {
        instructions::init_path(ctx, eid)
    }

    pub fn set_path_config(
        ctx: Context<SetPathConfig>,
        eid: u32,
        config: PathConfig,
    ) -> Result<()> {
        instructions::set_path_config(ctx, eid, config)
    }

    pub fn set_alt(ctx: Context<SetAlt>) -> Result<()> {
        instructions::set_alt(ctx)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }

    pub fn prove(ctx: Context<Prove>, args: eco_svm_std::prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }

    pub fn send_message<'info>(
        ctx: Context<'info, SendMessage<'info>>,
        max_native_fee: u64,
    ) -> Result<()> {
        instructions::send_message(ctx, max_native_fee)
    }

    pub fn quote_message<'info>(
        ctx: Context<'info, QuoteMessage<'info>>,
        args: QuoteMessageArgs,
    ) -> Result<layerzero::MessagingFee> {
        instructions::quote_message(ctx, args)
    }
}
