use anchor_lang::{AnchorDeserialize, AnchorSerialize, Discriminator};
use anyhow::Result;
use litesvm::LiteSVM;
use rand::Rng;
use solana_sdk::{account::Account, clock::Clock, pubkey::Pubkey};

pub fn sol_amount(amount: f64) -> u64 {
    (amount * 1_000_000_000.0) as u64
}

pub fn usdc_amount(amount: f64) -> u64 {
    (amount * 1_000_000.0) as u64
}

pub fn write_account_no_data(svm: &mut LiteSVM, pubkey: Pubkey, lamports: u64) -> Result<()> {
    svm.set_account(
        pubkey,
        Account {
            data: vec![],
            owner: solana_system_interface::program::ID,
            lamports,
            ..Account::default()
        },
    )?;
    Ok(())
}

pub fn write_account_re(
    svm: &mut LiteSVM,
    pubkey: Pubkey,
    owner: Pubkey,
    data: Vec<u8>,
) -> Result<()> {
    svm.set_account(
        pubkey,
        Account {
            lamports: svm.minimum_balance_for_rent_exemption(data.len()),
            data,
            owner,
            ..Account::default()
        },
    )?;
    Ok(())
}

pub fn write_account_anchor_re<T: AnchorSerialize>(
    svm: &mut LiteSVM,
    pubkey: Pubkey,
    owner: Pubkey,
    data: T,
    discriminator: &[u8],
) -> Result<()> {
    let mut data_vec = discriminator.to_vec();
    data_vec.extend(data.try_to_vec()?);

    svm.set_account(
        pubkey,
        Account {
            lamports: svm.minimum_balance_for_rent_exemption(data_vec.len()),
            data: data_vec,
            owner,
            ..Account::default()
        },
    )?;
    Ok(())
}

pub fn read_account_lamports(svm: &LiteSVM, pubkey: &Pubkey) -> Result<u64> {
    let account = svm
        .get_account(pubkey)
        .ok_or(anyhow::anyhow!("Account not found"))?;
    Ok(account.lamports)
}

pub fn read_account_lamports_re(svm: &LiteSVM, pubkey: &Pubkey) -> Result<u64> {
    let account = svm
        .get_account(pubkey)
        .ok_or(anyhow::anyhow!("Account not found"))?;
    Ok(account.lamports - svm.minimum_balance_for_rent_exemption(account.data.len()))
}

pub fn read_account_anchor<T: AnchorDeserialize + Discriminator>(
    svm: &LiteSVM,
    pubkey: &Pubkey,
) -> Result<T> {
    let account = svm
        .get_account(pubkey)
        .ok_or(anyhow::anyhow!("Account not found"))?;
    let data = account.data.as_slice();
    #[allow(deprecated)]
    Ok(anchor_lang::solana_program::borsh0_10::try_from_slice_unchecked(&data[8..])?)
}

pub fn generate_salt() -> [u8; 32] {
    let mut salt = [0u8; 32];
    salt.copy_from_slice(&rand::rng().random::<[u8; 32]>());
    salt
}

pub fn now_ts(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<Clock>().unix_timestamp
}
