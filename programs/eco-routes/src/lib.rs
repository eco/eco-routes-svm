use anchor_lang::prelude::*;

declare_id!("3zbEiMYyf4y1bGsVBAzKrXVzMndRQdTMDgx3aKCs8BHs");

pub mod encoding;
pub mod error;
pub mod instructions;
pub mod state;

use instructions::*;

pub mod hyperlane {
    use super::*;

    pub const DOMAIN_ID: u32 = 1;

    pub const MAILBOX_ID: Pubkey = pubkey!("3zbEiMYyf4y1bGsVBAzKrXVzMndRQdTMDgx3aKCs8BH1");
    pub const MULTISIG_ISM_ID: Pubkey = pubkey!("3zbEiMYyf4y1bGsVBAzKrXVzMndRQdTMDgx3aKCs8BH2");

    pub const HANDLE_DISCRIMINATOR: [u8; 8] = [33, 210, 5, 66, 196, 212, 239, 142];
    pub const HANDLE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] = [194, 141, 30, 82, 241, 41, 169, 52];
    pub const INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR: [u8; 8] =
        [45, 18, 245, 87, 234, 46, 246, 15];
    pub const INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] =
        [190, 214, 218, 129, 67, 97, 4, 76];
}

#[program]
pub mod eco_routes {

    use super::*;

    pub fn publish_intent(ctx: Context<PublishIntent>, args: PublishIntentArgs) -> Result<()> {
        instructions::publish_intent(ctx, args)
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
        _origin: u32,
        _sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        instructions::handle_account_metas(ctx, _origin, _sender, payload)
    }

    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_DISCRIMINATOR)]
    pub fn ism(ctx: Context<Ism>) -> Result<()> {
        instructions::ism(ctx)
    }

    #[instruction(discriminator = &hyperlane::INTERCHAIN_SECURITY_MODULE_ACCOUNT_METAS_DISCRIMINATOR)]
    pub fn ism_account_metas(ctx: Context<IsmAccountMetas>) -> Result<()> {
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
