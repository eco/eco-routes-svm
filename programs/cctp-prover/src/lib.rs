use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("Ecor8EE3VDHBu8JGU7LxoFYkfXDyLdsheRzDMmT4iWjz");

pub mod cctp;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod cctp_prover {
    use super::*;

    pub fn get_proof(
        ctx: Context<GetProof>,
        args: prover::GetProofArgs,
    ) -> Result<Option<prover::Proof>> {
        instructions::get_proof(ctx, args)
    }

    /// Circle's transmitter CPIs this for `finality_threshold_executed >= 2000`.
    pub fn handle_receive_finalized_message<'info>(
        ctx: Context<'info, HandleReceiveMessage<'info>>,
        params: cctp::HandleReceiveMessageParams,
    ) -> Result<()> {
        instructions::handle_receive_message(ctx, params)
    }

    /// Circle's transmitter CPIs this for `finality_threshold_executed < 2000`.
    pub fn handle_receive_unfinalized_message<'info>(
        ctx: Context<'info, HandleReceiveMessage<'info>>,
        params: cctp::HandleReceiveMessageParams,
    ) -> Result<()> {
        instructions::handle_receive_message(ctx, params)
    }

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }
}
