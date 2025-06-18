use std::collections::BTreeMap;

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount};
use tiny_keccak::{Hasher, Keccak};

use crate::instructions::PortalError;

pub type Bytes32 = [u8; 32];

pub struct TokenTransferAccounts<'info> {
    pub from: AccountInfo<'info>,
    pub to: AccountInfo<'info>,
    pub mint: AccountInfo<'info>,
}

impl<'info> TryFrom<Vec<&AccountInfo<'info>>> for TokenTransferAccounts<'info> {
    type Error = anchor_lang::error::Error;

    fn try_from(accounts: Vec<&AccountInfo<'info>>) -> Result<Self> {
        match accounts.as_slice() {
            [from, to, mint] => Ok(Self {
                from: from.to_account_info(),
                to: to.to_account_info(),
                mint: mint.to_account_info(),
            }),
            _ => Err(PortalError::InvalidTokenTransferAccounts.into()),
        }
    }
}

impl<'info> TokenTransferAccounts<'info> {
    pub fn transfer(
        &self,
        token_program: &AccountInfo<'info>,
        authority: &AccountInfo<'info>,
        amount: u64,
    ) -> Result<()> {
        transfer_checked(
            CpiContext::new(
                token_program.to_account_info(),
                anchor_spl::token_interface::TransferChecked {
                    from: self.from.to_account_info(),
                    to: self.to.to_account_info(),
                    mint: self.mint.to_account_info(),
                    authority: authority.to_account_info(),
                },
            ),
            amount,
            self.mint_data()?.decimals,
        )
    }

    pub fn program_id(&self) -> &Pubkey {
        self.from.owner
    }

    pub fn mint_data(&self) -> Result<Mint> {
        Mint::try_deserialize(&mut &self.mint.try_borrow_data()?[..])
    }

    pub fn from_data(&self) -> Result<TokenAccount> {
        TokenAccount::try_deserialize(&mut &self.from.try_borrow_data()?[..])
    }

    pub fn to_data(&self) -> Result<TokenAccount> {
        TokenAccount::try_deserialize(&mut &self.to.try_borrow_data()?[..])
    }
}

pub fn intent_hash(route_chain: Bytes32, route_hash: Bytes32, reward: &Reward) -> Bytes32 {
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];

    hasher.update(&route_chain);
    hasher.update(&route_hash);
    hasher.update(&reward.hash());

    hasher.finalize(&mut hash);

    hash
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Intent {
    pub route_chain: Bytes32,
    pub route: Route,
    pub reward: Reward,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Route {
    pub salt: Bytes32,
    pub route_chain_portal: Bytes32,
    pub tokens: Vec<TokenAmount>,
    pub calls: Vec<Call>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Reward {
    pub deadline: i64,
    pub creator: Pubkey,
    pub prover: Bytes32,
    pub native_amount: u64,
    pub tokens: Vec<TokenAmount>,
}

impl Reward {
    fn hash(&self) -> Bytes32 {
        let encoded = self.try_to_vec().expect("Failed to serialize Reward");
        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];

        hasher.update(&encoded);
        hasher.finalize(&mut hash);

        hash
    }

    pub fn token_amounts(&self) -> Result<BTreeMap<Pubkey, u64>> {
        self.tokens
            .iter()
            .try_fold(BTreeMap::<Pubkey, u64>::new(), |mut result, token| {
                let entry = result.entry(token.token).or_default();
                *entry = entry
                    .checked_add(token.amount)
                    .ok_or(PortalError::RewardAmountOverflow)?;

                Ok(result)
            })
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TokenAmount {
    pub token: Pubkey,
    pub amount: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Call {
    pub target: Bytes32,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_hash_deterministic() {
        let route_chain = [5u8; 32];
        let route_hash = [6u8; 32];
        let reward = Reward {
            deadline: 1500000,
            creator: Pubkey::default(),
            prover: [7u8; 32],
            native_amount: 250,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([40u8; 32]),
                    amount: 1000,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([50u8; 32]),
                    amount: 2000,
                },
            ],
        };

        let hash_1 = intent_hash(route_chain, route_hash, &reward);
        let hash_2 = intent_hash(route_chain, route_hash, &reward);

        assert_eq!(hash_1, hash_2);
        goldie::assert_json!(hash_1);
    }

    #[test]
    fn reward_token_amounts() {
        let reward = Reward {
            deadline: 1640995200,
            creator: Pubkey::new_from_array([1u8; 32]),
            prover: [2u8; 32],
            native_amount: 1_000_000_000,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 100,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([4u8; 32]),
                    amount: 200,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([5u8; 32]),
                    amount: 300,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 500,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([4u8; 32]),
                    amount: 0,
                },
            ],
        };

        goldie::assert_debug!(reward.token_amounts());
    }
}
