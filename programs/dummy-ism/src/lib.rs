use anchor_lang::prelude::*;

declare_id!("4GHxwWyKB9exhKG4fdyU2hfLgfFzhHp2WcsSKc2uNR1k");

#[program]
pub mod dummy_ism {
    use super::*;

    pub fn init(ctx: Context<Init>) -> Result<()> {
        let ism_state = &mut ctx.accounts.ism_state;
        ism_state.accept = true;
        Ok(())
    }

    pub fn set_accept(ctx: Context<SetAccept>, accept: bool) -> Result<()> {
        ctx.accounts.ism_state.accept = accept;
        Ok(())
    }

    #[instruction(discriminator = &[243, 53, 214, 0, 208, 18, 231, 67])]
    pub fn verify(ctx: Context<Verify>, _message: Vec<u8>, _metadata: Vec<u8>) -> Result<()> {
        require!(
            ctx.accounts.ism_state.accept,
            DummyIsmError::MessageRejected
        );
        Ok(())
    }

    pub fn verify_account_metas(
        _ctx: Context<VerifyAccountMetas>,
        _message: Vec<u8>,
    ) -> Result<()> {
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Init<'info> {
    #[account(
        init,
        payer = payer,
        space = 8 + IsmState::INIT_SPACE,
        seeds = [b"ism_state"],
        bump
    )]
    pub ism_state: Account<'info, IsmState>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetAccept<'info> {
    #[account(
        mut,
        seeds = [b"ism_state"],
        bump
    )]
    pub ism_state: Account<'info, IsmState>,
}

#[derive(Accounts)]
pub struct Verify<'info> {
    #[account(seeds = [b"ism_state"], bump)]
    pub ism_state: Account<'info, IsmState>,
}

#[derive(Accounts)]
pub struct VerifyAccountMetas<'info> {
    #[account(seeds = [b"ism_state"], bump)]
    pub ism_state: Account<'info, IsmState>,
}

#[account]
#[derive(InitSpace)]
pub struct IsmState {
    pub accept: bool,
}

#[error_code]
pub enum DummyIsmError {
    #[msg("Message verification rejected")]
    MessageRejected,
}
