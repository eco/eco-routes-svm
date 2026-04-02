use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::system_program;

use crate::instructions::PayForGas as PayForGasAccounts;

#[cfg(feature = "mainnet")]
pub const IGP_PROGRAM_ID: Pubkey = pubkey!("JAvHW21tYXE9dtdG83DReqU2b4LUexFuCbtJT5tF8X6M");
#[cfg(not(feature = "mainnet"))]
pub const IGP_PROGRAM_ID: Pubkey = pubkey!("9SQVtTNsbipdMzumhzi6X8GwojiSMwBfqAhS7FgyTcqy");

#[cfg(feature = "mainnet")]
pub const MAILBOX_ID: Pubkey = pubkey!("E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi");
#[cfg(not(feature = "mainnet"))]
pub const MAILBOX_ID: Pubkey = pubkey!("75HBBLae3ddeneJVrZeyrDfv6vb7SMC3aCpBucSXS5aR");

/// Hyperlane IGP instruction enum. Variant order is critical for Borsh
/// serialization — the `PayForGas` variant must remain at index 3.
#[derive(BorshSerialize, BorshDeserialize)]
#[allow(dead_code)]
pub enum IgpInstruction {
    Init,
    InitIgp(InitIgp),
    InitOverheadIgp(InitOverheadIgp),
    PayForGas(PayForGasData),
    QuoteGasPayment(QuoteGasPayment),
    TransferIgpOwnership(Option<Pubkey>),
    TransferOverheadIgpOwnership(Option<Pubkey>),
    SetIgpBeneficiary(Pubkey),
    SetDestinationGasOverheads(Vec<GasOverheadConfig>),
    SetGasOracleConfigs(Vec<GasOracleConfig>),
    Claim,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct InitIgp {
    pub salt: [u8; 32],
    pub owner: Option<Pubkey>,
    pub beneficiary: Pubkey,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct InitOverheadIgp {
    pub salt: [u8; 32],
    pub owner: Option<Pubkey>,
    pub inner: Pubkey,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PayForGasData {
    pub message_id: [u8; 32],
    pub destination_domain: u32,
    pub gas_amount: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct QuoteGasPayment {
    pub destination_domain: u32,
    pub gas_amount: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct GasOverheadConfig {
    pub destination_domain: u32,
    pub gas_overhead: Option<u64>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct GasOracleConfig {
    pub domain: u32,
    pub gas_oracle: Option<GasOracle>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct GasOracle {
    pub token_exchange_rate: u128,
    pub gas_price: u128,
}

pub fn pay_for_gas(
    ctx: &Context<PayForGasAccounts>,
    message_id: [u8; 32],
    destination_domain: u32,
    gas_amount: u64,
) -> Result<()> {
    let igp_instruction = IgpInstruction::PayForGas(PayForGasData {
        message_id,
        destination_domain,
        gas_amount,
    });

    let mut accounts = vec![
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new(ctx.accounts.payer.key(), true),
        AccountMeta::new(ctx.accounts.igp_program_data.key(), false),
        AccountMeta::new_readonly(ctx.accounts.unique_gas_payment.key(), true),
        AccountMeta::new(ctx.accounts.gas_payment_pda.key(), false),
        AccountMeta::new(ctx.accounts.igp_account.key(), false),
    ];
    if let Some(overhead_igp) = &ctx.accounts.overhead_igp {
        accounts.push(AccountMeta::new_readonly(overhead_igp.key(), false));
    }

    let ix = Instruction {
        program_id: ctx.accounts.igp_program.key(),
        accounts,
        data: igp_instruction.try_to_vec()?,
    };

    let mut account_infos = vec![
        ctx.accounts.system_program.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.igp_program_data.to_account_info(),
        ctx.accounts.unique_gas_payment.to_account_info(),
        ctx.accounts.gas_payment_pda.to_account_info(),
        ctx.accounts.igp_account.to_account_info(),
    ];
    if let Some(overhead_igp) = &ctx.accounts.overhead_igp {
        account_infos.push(overhead_igp.to_account_info());
    }

    invoke(&ix, &account_infos).map_err(Into::into)
}
