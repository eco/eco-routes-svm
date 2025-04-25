//! Helpers shared by every flow–test.
use anchor_lang::solana_program::example_mocks::solana_sdk::system_program;
use anyhow::Result;
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::Instruction,
    message::{v0::Message, VersionedMessage},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    sysvar::clock::Clock,
    transaction::VersionedTransaction,
};
use spl_token::state::Mint;

pub fn airdrop_initial_amount(svm: &mut LiteSVM, k: &Pubkey) -> Result<()> {
    let mut svm2 = LiteSVM::new();

    svm2.airdrop(&k, 1)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    svm.airdrop(&k, 1).unwrap();
    Ok(())
}

pub fn now_ts(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<Clock>().unix_timestamp
}

pub fn write_account(svm: &mut LiteSVM, pubkey: Pubkey, data: &[u8], owner: Pubkey) -> Result<()> {
    svm.set_account(
        pubkey,
        Account {
            data: data.to_vec(),
            owner: owner,
            lamports: svm.minimum_balance_for_rent_exemption(data.len()),
            executable: false,
            rent_epoch: 0,
        },
    )?;

    Ok(())
}

pub fn send_instructions_signed(
    svm: &mut LiteSVM,
    instructions: &[Instruction],
    signers: &[&impl Signer],
) -> Result<()> {
    let message = Message::try_compile(
        &signers[0].pubkey(),
        instructions,
        &[],
        svm.latest_blockhash(),
    )?;

    let transaction = VersionedTransaction::try_new(VersionedMessage::V0(message), signers)?;

    svm.send_transaction(transaction)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    Ok(())
}

pub fn write_mint_with_distribution(
    svm: &mut LiteSVM,
    mint: &Keypair,
    decimals: u8,
    distribution: Vec<(&Pubkey, &Pubkey, u64)>,
) -> Result<Pubkey> {
    let total_supply = distribution.iter().map(|(_, _, amount)| amount).sum();

    let mint_slice: &mut [u8] = &mut [0u8; Mint::LEN];
    Mint::pack(
        Mint {
            mint_authority: solana_sdk::program_option::COption::None,
            supply: total_supply,
            decimals,
            is_initialized: true,
            freeze_authority: solana_sdk::program_option::COption::None,
        },
        mint_slice,
    )?;

    write_account(svm, mint.pubkey(), mint_slice, spl_token::id())?;

    for (owner, token_account, amount) in distribution {
        let account_slice: &mut [u8] = &mut [0u8; spl_token::state::Account::LEN];
        spl_token::state::Account::pack(
            spl_token::state::Account {
                mint: mint.pubkey(),
                owner: owner.clone(),
                amount: amount,
                delegate: solana_sdk::program_option::COption::None,
                close_authority: solana_sdk::program_option::COption::None,
                state: spl_token::state::AccountState::Initialized,
                is_native: solana_sdk::program_option::COption::None,
                delegated_amount: 0,
            },
            account_slice,
        )?;

        write_account(svm, token_account.clone(), account_slice, spl_token::id())?;
    }

    Ok(mint.pubkey())
}
