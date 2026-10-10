use solana_sdk::instruction::AccountMeta;

#[derive(Clone)]
pub struct ProverQuery {
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

impl<T: IntoIterator<Item = AccountMeta>> From<T> for ProverQuery {
    fn from(accounts: T) -> Self {
        Self {
            accounts: accounts.into_iter().collect(),
            data: vec![],
        }
    }
}
