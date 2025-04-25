use anchor_lang::prelude::*;

declare_id!("3zbEiMYyf4y1bGsVBAzKrXVzMndRQdTMDgx3aKCs8BHs");

pub mod error;
pub mod instructions;
pub mod state;

use instructions::*;

pub mod hyperlane {
    use super::*;

    pub const DOMAIN_ID: u32 = 1;

    pub const MAILBOX_ID: Pubkey = pubkey!("11111111111111111111111111111111");
    pub const MULTISIG_ISM_ID: Pubkey = pubkey!("11111111111111111111111111111111");

    pub const HANDLE_DISCRIMINATOR: [u8; 8] = [33, 210, 5, 66, 196, 212, 239, 142];
    pub const HANDLE_ACCOUNT_METAS_DISCRIMINATOR: [u8; 8] = [194, 141, 30, 82, 241, 41, 169, 52];
}

pub const AUTHORITY: Pubkey = pubkey!("11111111111111111111111111111111");

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

    pub fn dispatch_intent(ctx: Context<DispatchIntent>) -> Result<()> {
        instructions::dispatch_intent(ctx)
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

    pub fn handle_blueprint(
        ctx: Context<HandleBlueprint>,
        args: HandleBlueprintArgs,
        origin: u32,
        sender: [u8; 32],
    ) -> Result<()> {
        instructions::handle_blueprint(ctx, args, origin, sender)
    }

    pub fn handle_fulfilled_ack(
        ctx: Context<HandleFulfilledAck>,
        args: HandleFulfilledAckArgs,
        origin: u32,
        sender: [u8; 32],
    ) -> Result<()> {
        instructions::handle_fulfilled_ack(ctx, args, origin, sender)
    }

    pub fn fulfill_intent(ctx: Context<FulfillIntent>, args: FulfillIntentArgs) -> Result<()> {
        instructions::fulfill_intent(ctx, args)
    }

    pub fn close_intent(ctx: Context<CloseIntent>) -> Result<()> {
        instructions::close_intent(ctx)
    }

    pub fn set_domain_registry(
        ctx: Context<SetDomainRegistry>,
        args: SetDomainRegistryArgs,
    ) -> Result<()> {
        instructions::set_domain_registry(ctx, args)
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
