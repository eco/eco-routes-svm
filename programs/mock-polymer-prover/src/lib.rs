//! Test-only stand-in for Polymer's deployed `polymer_prover` program
//! (localnet only; excluded from devnet/mainnet builds). Instruction names and
//! account layouts match polymerdao/solana-prover-contracts v1.0.4 so the Anchor
//! discriminators and PDA seeds polymer-prover mirrors resolve here unchanged.
//!
//! Instead of verifying a proof, `validate_event` Borsh-decodes the bytes
//! accumulated in the cache as a `ValidationResultAccount` body and stores it,
//! so a test decides exactly which EVM event "was proven".

use anchor_lang::prelude::*;

// Polymer's devnet program ID: what polymer-prover targets in non-mainnet builds.
declare_id!("FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3");

const DISCRIMINATOR_SIZE: usize = 8;
/// Four 32-byte topics: an EVM log carries at most three indexed arguments
/// plus the selector.
const MAX_TOPICS_LEN: usize = 32 * 4;

#[account]
#[derive(InitSpace)]
pub struct ProofCacheAccount {
    #[max_len(3000)]
    pub cache: Vec<u8>,
}

#[account]
#[derive(InitSpace, Default)]
pub struct ValidationResultAccount {
    pub is_valid: bool,
    #[max_len(64)]
    pub error_message: String,
    pub chain_id: u32,
    pub emitting_contract: [u8; 20],
    #[max_len(MAX_TOPICS_LEN)]
    pub topics: Vec<u8>,
    #[max_len(3000)]
    pub unindexed_data: Vec<u8>,
}

#[derive(Accounts)]
pub struct CreateAccounts<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        seeds = [b"cache", authority.key().as_ref()],
        bump,
        payer = authority,
        space = DISCRIMINATOR_SIZE + ProofCacheAccount::INIT_SPACE,
    )]
    pub cache_account: Account<'info, ProofCacheAccount>,
    #[account(
        init,
        seeds = [b"result", authority.key().as_ref()],
        bump,
        payer = authority,
        space = DISCRIMINATOR_SIZE + ValidationResultAccount::INIT_SPACE,
    )]
    pub result_account: Account<'info, ValidationResultAccount>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LoadProof<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, seeds = [b"cache", authority.key().as_ref()], bump)]
    pub cache_account: Account<'info, ProofCacheAccount>,
}

#[derive(Accounts)]
pub struct ValidateEvent<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, seeds = [b"cache", authority.key().as_ref()], bump)]
    pub cache_account: Account<'info, ProofCacheAccount>,
    #[account(mut, seeds = [b"result", authority.key().as_ref()], bump)]
    pub result_account: Account<'info, ValidationResultAccount>,
    /// CHECK: seeds only; the real program reads sequencer config from here,
    /// the mock needs nothing.
    #[account(seeds = [b"internal"], bump)]
    pub internal: UncheckedAccount<'info>,
}

#[program]
pub mod mock_polymer_prover {
    use super::*;

    pub fn create_accounts(_ctx: Context<CreateAccounts>) -> Result<()> {
        Ok(())
    }

    pub fn load_proof(ctx: Context<LoadProof>, proof_chunk: Vec<u8>) -> Result<()> {
        ctx.accounts.cache_account.cache.extend(proof_chunk);
        Ok(())
    }

    pub fn validate_event(ctx: Context<ValidateEvent>) -> Result<()> {
        let mut body = ctx.accounts.cache_account.cache.as_slice();
        let result: ValidationResultAccount = AnchorDeserialize::deserialize(&mut body)?;
        ctx.accounts.result_account.set_inner(result);
        ctx.accounts.cache_account.cache.clear();
        Ok(())
    }
}
