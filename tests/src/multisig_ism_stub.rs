use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use litesvm::LiteSVM;
use solana_sdk::{account::Account, pubkey::Pubkey};

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Default)]
pub struct TestIsmStorage {
    pub accept: bool,
}

pub fn domain_data_pda(domain: u32, ism_program_id: &Pubkey) -> (Pubkey, u8) {
    let domain_be = domain.to_be_bytes();
    Pubkey::find_program_address(&[b"test_ism", b"-", b"storage"], ism_program_id)
}

pub fn write_domain_data(svm: &mut LiteSVM, ism_program_id: Pubkey) -> Result<()> {
    let (pda, bump) = domain_data_pda(0, &ism_program_id);

    let domain_data = TestIsmStorage { accept: true };

    let data = borsh::to_vec(&domain_data)?;

    let mut account = Account {
        lamports: svm.minimum_balance_for_rent_exemption(data.len()),
        data: data.to_vec(),
        owner: ism_program_id,
        executable: false,
        rent_epoch: 0,
    };

    svm.set_account(pda, account)?;

    Ok(())
}
