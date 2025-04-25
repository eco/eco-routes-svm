use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use litesvm::LiteSVM;
use solana_sdk::{account::Account, pubkey::Pubkey};

pub const DOMAIN_DATA_SEED: &[u8] = b"domain_data";

#[derive(Debug, BorshSerialize, BorshDeserialize, Clone, PartialEq, Eq)]
pub struct ValidatorsAndThreshold {
    pub validators: Vec<[u8; 20]>,
    pub threshold: u8,
}

#[derive(Debug, BorshSerialize, BorshDeserialize, Clone, PartialEq, Eq)]
pub struct DomainData {
    pub bump_seed: u8,
    pub validators_and_threshold: ValidatorsAndThreshold,
}

pub fn domain_data_pda(domain: u32, ism_program_id: &Pubkey) -> (Pubkey, u8) {
    let domain_be = domain.to_be_bytes();
    Pubkey::find_program_address(&[DOMAIN_DATA_SEED, &domain_be], ism_program_id)
}

pub fn write_domain_data(
    svm: &mut LiteSVM,
    ism_program_id: Pubkey,
    origin_domain: u32,
    validators_and_threshold: ValidatorsAndThreshold,
) -> Result<()> {
    let (pda, bump) = domain_data_pda(origin_domain, &ism_program_id);

    let domain_data = DomainData {
        bump_seed: bump,
        validators_and_threshold,
    };

    let data = borsh::to_vec(&domain_data)?;

    let mut account = Account::new(
        svm.minimum_balance_for_rent_exemption(data.len()),
        data.len(),
        &ism_program_id,
    );
    account.data = data.to_vec();
    svm.set_account(pda, account)?;

    Ok(())
}
