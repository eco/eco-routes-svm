use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("EcoZ3pDi8PJf9ohgg6HJdgPviSqQCnbgD14KCnE4rEZm");

pub mod constants;
pub mod instructions;
pub mod layerzero;
pub mod state;

use instructions::*;
use layerzero::{LzReceiveParams, LzReceiveTypesInfoResult, LzReceiveTypesV2Result};
use state::Peer;

#[program]
pub mod layerzero_prover {
    use super::*;

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn add_peers(ctx: Context<AddPeers>, peers: Vec<Peer>) -> Result<()> {
        instructions::add_peers(ctx, peers)
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

    pub fn get_proof(
        ctx: Context<GetProof>,
        args: prover::GetProofArgs,
    ) -> Result<Option<prover::Proof>> {
        instructions::get_proof(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>, args: prover::CloseProofArgs) -> Result<()> {
        instructions::close_proof(ctx, args)
    }

    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
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

    pub fn lz_receive<'info>(
        ctx: Context<'info, LzReceive<'info>>,
        params: LzReceiveParams,
    ) -> Result<()> {
        instructions::lz_receive(ctx, params)
    }

    pub fn lz_receive_types_info(
        ctx: Context<LzReceiveTypesInfo>,
        params: LzReceiveParams,
    ) -> Result<LzReceiveTypesInfoResult> {
        instructions::lz_receive_types_info(ctx, params)
    }

    pub fn lz_receive_types_v2(
        ctx: Context<LzReceiveTypesV2>,
        params: LzReceiveParams,
    ) -> Result<LzReceiveTypesV2Result> {
        instructions::lz_receive_types_v2(ctx, params)
    }
}
