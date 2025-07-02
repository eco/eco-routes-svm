use crate::hyperlane::SimulationReturnData;
use anchor_lang::prelude::*;

declare_id!("a6BKzp2ixm6ogEJ268UT4UGFMLnsgPWnVm93vsjupc3");

pub mod encoding;
pub mod error;
pub mod hyperlane;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod eco_routes {
    use super::*;

    pub fn publish_intent(ctx: Context<PublishIntent>, args: PublishIntentArgs) -> Result<()> {
        instructions::publish_intent(ctx, args)
    }

    pub fn initialize_eco_routes(
        ctx: Context<InitializeEcoRoutes>,
        args: InitializeEcoRoutesArgs,
    ) -> Result<()> {
        instructions::initialize_eco_routes(ctx, args)
    }

    pub fn set_authority(ctx: Context<SetAuthority>) -> Result<()> {
        instructions::set_authority(ctx)
    }

    pub fn set_authorized_prover(
        ctx: Context<SetAuthorizedProver>,
        args: SetAuthorizedProverArgs,
    ) -> Result<()> {
        instructions::set_authorized_prover(ctx, args)
    }

    pub fn fund_intent_spl(ctx: Context<FundIntentSpl>, args: FundIntentSplArgs) -> Result<()> {
        instructions::fund_intent_spl(ctx, args)
    }

    pub fn fund_intent_native(
        ctx: Context<FundIntentNative>,
        args: FundIntentNativeArgs,
    ) -> Result<()> {
        instructions::fund_intent_native(ctx, args)
    }

    pub fn refund_intent_native(
        ctx: Context<RefundIntentNative>,
        args: RefundIntentNativeArgs,
    ) -> Result<()> {
        instructions::refund_intent_native(ctx, args)
    }

    pub fn refund_intent_spl(
        ctx: Context<RefundIntentSpl>,
        args: RefundIntentSplArgs,
    ) -> Result<()> {
        instructions::refund_intent_spl(ctx, args)
    }

    #[instruction(discriminator = &hyperlane::HANDLE_DISCRIMINATOR)]
    pub fn handle<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, Handle<'info>>,
        origin: u32,
        sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        instructions::handle(ctx, origin, sender, payload)
    }

    #[instruction(discriminator = &hyperlane::HANDLE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn handle_account_metas(
        ctx: Context<HandleAccountMetas>,
        _origin: u32,
        _sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<SimulationReturnData<Vec<SerializableAccountMeta>>> {
        instructions::handle_account_metas(ctx, _origin, _sender, payload)
    }

    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR)]
    pub fn ism(ctx: Context<Ism>) -> Result<()> {
        instructions::ism(ctx)
    }

    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn ism_account_metas(
        ctx: Context<IsmAccountMetas>,
    ) -> Result<SimulationReturnData<Vec<SerializableAccountMeta>>> {
        instructions::ism_account_metas(ctx)
    }

    pub fn fulfill_intent<'info>(
        ctx: Context<'_, '_, '_, 'info, FulfillIntent<'info>>,
        args: FulfillIntentArgs,
    ) -> Result<()> {
        instructions::fulfill_intent(ctx, args)
    }

    pub fn close_intent(ctx: Context<CloseIntent>) -> Result<()> {
        instructions::close_intent(ctx)
    }

    pub fn claim_intent_native(
        ctx: Context<ClaimIntentNative>,
        args: ClaimIntentNativeArgs,
    ) -> Result<()> {
        instructions::claim_intent_native(ctx, args)
    }

    pub fn claim_intent_spl(ctx: Context<ClaimIntentSpl>, args: ClaimIntentSplArgs) -> Result<()> {
        instructions::claim_intent_spl(ctx, args)
    }
}
