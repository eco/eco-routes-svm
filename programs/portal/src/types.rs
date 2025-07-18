use std::collections::BTreeMap;

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token};
use anchor_spl::token_2022::{self, Token2022};
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount};
use eco_svm_std::{Bytes32, SerializableAccountMeta};
use itertools::Itertools;
use tiny_keccak::{Hasher, Keccak};

use crate::instructions::PortalError;

pub const VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE: usize = 3;

pub struct VecTokenTransferAccounts<'info>(Vec<TokenTransferAccounts<'info>>);

impl<'info> TryFrom<&[AccountInfo<'info>]> for VecTokenTransferAccounts<'info> {
    type Error = anchor_lang::error::Error;

    fn try_from(accounts: &[AccountInfo<'info>]) -> Result<Self> {
        accounts
            .iter()
            .chunks(VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE)
            .into_iter()
            .map(|chunk| chunk.collect::<Vec<_>>().try_into())
            .collect::<Result<Vec<TokenTransferAccounts>>>()
            .map(Self)
    }
}

impl<'info> VecTokenTransferAccounts<'info> {
    pub fn into_inner(self) -> Vec<TokenTransferAccounts<'info>> {
        self.0
    }
}

pub struct TokenTransferAccounts<'info> {
    pub from: AccountInfo<'info>,
    pub to: AccountInfo<'info>,
    pub mint: AccountInfo<'info>,
}

impl<'info> TryFrom<Vec<&AccountInfo<'info>>> for TokenTransferAccounts<'info> {
    type Error = anchor_lang::error::Error;

    fn try_from(accounts: Vec<&AccountInfo<'info>>) -> Result<Self> {
        match accounts.as_slice() {
            [from, to, mint] => {
                // validate that the accounts are all owned by the same token program
                let token_program = mint.owner;
                require!(
                    token_program == from.owner,
                    PortalError::InvalidTokenTransferAccounts
                );
                require!(
                    to.data_is_empty() || token_program == to.owner,
                    PortalError::InvalidTokenTransferAccounts
                );

                Ok(Self {
                    from: from.to_account_info(),
                    to: to.to_account_info(),
                    mint: mint.to_account_info(),
                })
            }
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
        match amount {
            0 => Ok(()),
            amount => transfer_checked(
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
            ),
        }
    }

    pub fn transfer_with_signer(
        &self,
        token_program: &AccountInfo<'info>,
        authority: &AccountInfo<'info>,
        signer_seeds: &[&[&[u8]]],
        amount: u64,
    ) -> Result<()> {
        match amount {
            0 => Ok(()),
            amount => transfer_checked(
                CpiContext::new_with_signer(
                    token_program.to_account_info(),
                    anchor_spl::token_interface::TransferChecked {
                        from: self.from.to_account_info(),
                        to: self.to.to_account_info(),
                        mint: self.mint.to_account_info(),
                        authority: authority.to_account_info(),
                    },
                    signer_seeds,
                ),
                amount,
                self.mint_data()?.decimals,
            ),
        }
    }

    pub fn token_program(
        &self,
        token_program: &Program<'info, Token>,
        token_2022_program: &Program<'info, Token2022>,
    ) -> Result<AccountInfo<'info>> {
        let token_program_id = self.token_program_id();

        if *token_program_id == token::ID {
            Ok(token_program.to_account_info())
        } else if *token_program_id == token_2022::ID {
            Ok(token_2022_program.to_account_info())
        } else {
            Err(PortalError::InvalidTokenProgram.into())
        }
    }

    pub fn token_program_id(&self) -> &Pubkey {
        self.mint.owner
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

/// Represents minimal calldata that can fit within Solana's 1024-byte instruction limit.
/// This is provided as part of the fulfill instruction on the destination chain.
///
/// # Background
/// Cross-chain intents need to include calldata for execution on the destination chain.
/// However, Solana has a strict 1024-byte limit per instruction, making it infeasible to
/// include full calldata with all account metadata in a single fulfill instruction.
///
/// # Solution
/// The source chain submits the Route with full `CallDataWithAccounts`, but on the destination
/// chain we provide only the minimal `Calldata` in the fulfill instruction. The account information is
/// provided separately via the transaction accounts to reconstruct the complete calldata.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Calldata {
    pub data: Vec<u8>,
    pub account_count: u8,
}

/// Complete calldata including both instruction data and account metadata.
/// This is submitted on the source chain and reconstructed on the destination chain during fulfillment.
///
/// # Workflow
/// 1. **Source Chain**: Intent submitted with Route containing `CallDataWithAccounts` (complete data)
/// 2. **Destination Chain**: Fulfill instruction provides minimal `Calldata` + accounts in transaction
/// 3. **Reconstruction**: Accounts from transaction combined with `Calldata` to build `CallDataWithAccounts`
/// 4. **Execution**: Complete calldata is used to calculate intent hash and mark as fulfilled
///
/// This approach allows complex cross-chain calls while respecting Solana's instruction size limits.
#[derive(AnchorSerialize, AnchorDeserialize, Debug)]
pub struct CalldataWithAccounts {
    pub calldata: Calldata,
    pub accounts: Vec<SerializableAccountMeta>,
}

impl CalldataWithAccounts {
    pub fn new<T>(calldata: Calldata, accounts: Vec<T>) -> Result<Self>
    where
        T: Into<SerializableAccountMeta>,
    {
        require!(
            accounts.len() == calldata.account_count as usize,
            PortalError::InvalidCalldata,
        );

        Ok(Self {
            calldata,
            accounts: accounts.into_iter().map(Into::into).collect(),
        })
    }
}

pub fn intent_hash(destination: u64, route_hash: &Bytes32, reward_hash: &Bytes32) -> Bytes32 {
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];

    hasher.update(destination.to_be_bytes().as_slice());
    hasher.update(route_hash.as_ref());
    hasher.update(reward_hash.as_ref());

    hasher.finalize(&mut hash);

    hash.into()
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Intent {
    pub destination: u64,
    pub route: Route,
    pub reward: Reward,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Route {
    pub salt: Bytes32,
    pub deadline: u64,
    pub portal: Bytes32,
    pub tokens: Vec<TokenAmount>,
    pub calls: Vec<Call>,
}

impl Route {
    pub fn hash(&self) -> Bytes32 {
        let encoded = self.try_to_vec().expect("Failed to serialize Route");
        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];

        hasher.update(&encoded);
        hasher.finalize(&mut hash);

        hash.into()
    }

    pub fn token_amounts(&self) -> Result<BTreeMap<Pubkey, u64>> {
        token_amounts(&self.tokens)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Reward {
    pub deadline: u64,
    pub creator: Pubkey,
    pub prover: Pubkey,
    pub native_amount: u64,
    pub tokens: Vec<TokenAmount>,
}

impl Reward {
    pub fn hash(&self) -> Bytes32 {
        let encoded = self.try_to_vec().expect("Failed to serialize Reward");
        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];

        hasher.update(&encoded);
        hasher.finalize(&mut hash);

        hash.into()
    }

    pub fn token_amounts(&self) -> Result<BTreeMap<Pubkey, u64>> {
        token_amounts(&self.tokens)
    }
}

fn token_amounts(tokens: &[TokenAmount]) -> Result<BTreeMap<Pubkey, u64>> {
    tokens
        .iter()
        .try_fold(BTreeMap::<Pubkey, u64>::new(), |mut result, token| {
            let entry = result.entry(token.token).or_default();
            *entry = entry
                .checked_add(token.amount)
                .ok_or(PortalError::TokenAmountOverflow)?;

            Ok(result)
        })
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct TokenAmount {
    pub token: Pubkey,
    pub amount: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Call {
    pub target: Bytes32,
    pub data: Vec<u8>,
    pub value: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_hash_deterministic() {
        let destination = 1000;
        let route_hash = [6u8; 32].into();
        let reward = Reward {
            deadline: 1500000,
            creator: Pubkey::default(),
            prover: Pubkey::new_from_array([7u8; 32]),
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

        let hash_1 = intent_hash(destination, &route_hash, &reward.hash());
        let hash_2 = intent_hash(destination, &route_hash, &reward.hash());

        assert_eq!(hash_1, hash_2);
        goldie::assert_json!(hash_1.as_ref());
    }

    #[test]
    fn reward_token_amounts() {
        let reward = Reward {
            deadline: 1640995200,
            creator: Pubkey::new_from_array([1u8; 32]),
            prover: Pubkey::new_from_array([2u8; 32]),
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

    #[test]
    fn vec_token_transfer_accounts_empty_slice() {
        let accounts: &[AccountInfo] = &[];

        let result = VecTokenTransferAccounts::try_from(accounts);
        assert_eq!(result.unwrap().into_inner().len(), 0);
    }

    #[test]
    fn vec_token_transfer_accounts_valid_chunks() {
        let token_program = anchor_spl::token::ID;
        let from_key = Pubkey::new_unique();
        let to_key = Pubkey::new_unique();
        let mint_key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut lamports_3 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![];
        let mut data_3 = vec![];

        let from_account = AccountInfo::new(
            &from_key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_program,
            false,
            0,
        );
        let to_account = AccountInfo::new(
            &to_key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &token_program,
            false,
            0,
        );
        let mint_account = AccountInfo::new(
            &mint_key,
            false,
            false,
            &mut lamports_3,
            &mut data_3,
            &token_program,
            false,
            0,
        );

        let accounts: &[AccountInfo] = &[from_account, to_account, mint_account];
        let result = VecTokenTransferAccounts::try_from(accounts);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_inner().len(), 1);
    }

    #[test]
    fn vec_token_transfer_accounts_invalid_chunk_size() {
        let token_program = anchor_spl::token::ID;
        let key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![];

        let account_1 = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_program,
            false,
            0,
        );
        let account_2 = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &token_program,
            false,
            0,
        );

        let accounts: &[AccountInfo] = &[account_1, account_2];
        let result = VecTokenTransferAccounts::try_from(accounts);
        assert!(result.is_err());
    }

    #[test]
    fn token_transfer_accounts_wrong_number_of_accounts() {
        let token_program = anchor_spl::token::ID;
        let key = Pubkey::new_unique();
        let mut lamports = 0;
        let mut data = vec![];

        let account = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut data,
            &token_program,
            false,
            0,
        );

        let two_accounts = vec![&account, &account];
        let result = TokenTransferAccounts::try_from(two_accounts);
        assert!(result.is_err());

        let four_accounts = vec![&account, &account, &account, &account];
        let result = TokenTransferAccounts::try_from(four_accounts);
        assert!(result.is_err());
    }

    #[test]
    fn token_transfer_accounts_mismatched_owners() {
        let token_program = anchor_spl::token::ID;
        let different_program = anchor_spl::token_2022::ID;
        let from_key = Pubkey::new_unique();
        let to_key = Pubkey::new_unique();
        let mint_key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut lamports_3 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![1, 2, 3]; // non-empty data
        let mut data_3 = vec![];

        let from_account = AccountInfo::new(
            &from_key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_program,
            false,
            0,
        );
        let to_account = AccountInfo::new(
            &to_key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &different_program,
            false,
            0,
        );
        let mint_account = AccountInfo::new(
            &mint_key,
            false,
            false,
            &mut lamports_3,
            &mut data_3,
            &token_program,
            false,
            0,
        );

        let accounts = vec![&from_account, &to_account, &mint_account];
        let result = TokenTransferAccounts::try_from(accounts);
        assert!(result.is_err());
    }

    #[test]
    fn token_transfer_accounts_valid() {
        let token_program = anchor_spl::token::ID;
        let from_key = Pubkey::new_unique();
        let to_key = Pubkey::new_unique();
        let mint_key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut lamports_3 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![];
        let mut data_3 = vec![];

        let from_account = AccountInfo::new(
            &from_key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_program,
            false,
            0,
        );
        let to_account = AccountInfo::new(
            &to_key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &token_program,
            false,
            0,
        );
        let mint_account = AccountInfo::new(
            &mint_key,
            false,
            false,
            &mut lamports_3,
            &mut data_3,
            &token_program,
            false,
            0,
        );

        let accounts = vec![&from_account, &to_account, &mint_account];
        let result = TokenTransferAccounts::try_from(accounts);
        assert!(result.is_ok());
    }

    #[test]
    fn token_transfer_accounts_empty_to_account() {
        let token_program = anchor_spl::token::ID;
        let system_program = Pubkey::default();
        let from_key = Pubkey::new_unique();
        let to_key = Pubkey::new_unique();
        let mint_key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut lamports_3 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![];
        let mut data_3 = vec![];

        let from_account = AccountInfo::new(
            &from_key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_program,
            false,
            0,
        );
        let empty_to_account = AccountInfo::new(
            &to_key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &system_program,
            false,
            0,
        );
        let mint_account = AccountInfo::new(
            &mint_key,
            false,
            false,
            &mut lamports_3,
            &mut data_3,
            &token_program,
            false,
            0,
        );

        let accounts = vec![&from_account, &empty_to_account, &mint_account];
        let result = TokenTransferAccounts::try_from(accounts);
        assert!(result.is_ok());
    }

    #[test]
    fn token_transfer_accounts_token_program_id() {
        let token_program = anchor_spl::token::ID;
        let from_key = Pubkey::new_unique();
        let to_key = Pubkey::new_unique();
        let mint_key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut lamports_3 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![];
        let mut data_3 = vec![];

        let from_account = AccountInfo::new(
            &from_key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_program,
            false,
            0,
        );
        let to_account = AccountInfo::new(
            &to_key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &token_program,
            false,
            0,
        );
        let mint_account = AccountInfo::new(
            &mint_key,
            false,
            false,
            &mut lamports_3,
            &mut data_3,
            &token_program,
            false,
            0,
        );

        let accounts = vec![&from_account, &to_account, &mint_account];
        let transfer_accounts = TokenTransferAccounts::try_from(accounts).unwrap();

        goldie::assert_debug!(transfer_accounts.token_program_id());
    }

    #[test]
    fn token_transfer_accounts_token_program_id_2022() {
        let token_2022_program = anchor_spl::token_2022::ID;
        let from_key = Pubkey::new_unique();
        let to_key = Pubkey::new_unique();
        let mint_key = Pubkey::new_unique();
        let mut lamports_1 = 0;
        let mut lamports_2 = 0;
        let mut lamports_3 = 0;
        let mut data_1 = vec![];
        let mut data_2 = vec![];
        let mut data_3 = vec![];

        let from_account = AccountInfo::new(
            &from_key,
            false,
            false,
            &mut lamports_1,
            &mut data_1,
            &token_2022_program,
            false,
            0,
        );
        let to_account = AccountInfo::new(
            &to_key,
            false,
            false,
            &mut lamports_2,
            &mut data_2,
            &token_2022_program,
            false,
            0,
        );
        let mint_account = AccountInfo::new(
            &mint_key,
            false,
            false,
            &mut lamports_3,
            &mut data_3,
            &token_2022_program,
            false,
            0,
        );

        let accounts = vec![&from_account, &to_account, &mint_account];
        let transfer_accounts = TokenTransferAccounts::try_from(accounts).unwrap();

        goldie::assert_debug!(transfer_accounts.token_program_id());
    }

    #[test]
    fn route_hash_deterministic() {
        let route = Route {
            deadline: 1700000000,
            salt: [1u8; 32].into(),
            portal: [2u8; 32].into(),
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 100,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([4u8; 32]),
                    amount: 200,
                },
            ],
            calls: vec![
                Call {
                    target: [5u8; 32].into(),
                    data: vec![1, 2, 3],
                    value: 0,
                },
                Call {
                    target: [6u8; 32].into(),
                    data: vec![4, 5, 6],
                    value: 1000,
                },
            ],
        };

        goldie::assert_json!(route.hash().as_ref());
    }

    #[test]
    fn route_token_amounts_deterministic() {
        let route = Route {
            deadline: 1700000000,
            salt: [1u8; 32].into(),
            portal: [2u8; 32].into(),
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
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 50,
                },
            ],
            calls: vec![],
        };

        goldie::assert_debug!(route.token_amounts());
    }

    #[test]
    fn calldata_with_accounts_success() {
        let calldata = Calldata {
            data: vec![1, 2, 3, 4, 5],
            account_count: 3,
        };
        let accounts = vec![
            SerializableAccountMeta {
                pubkey: Pubkey::new_from_array([1u8; 32]),
                is_signer: true,
                is_writable: false,
            },
            SerializableAccountMeta {
                pubkey: Pubkey::new_from_array([2u8; 32]),
                is_signer: false,
                is_writable: true,
            },
            SerializableAccountMeta {
                pubkey: Pubkey::new_from_array([3u8; 32]),
                is_signer: false,
                is_writable: false,
            },
        ];

        let result = CalldataWithAccounts::new(calldata, accounts);
        goldie::assert_debug!(result);
    }

    #[test]
    fn calldata_with_accounts_invalid_count_fail() {
        let calldata = Calldata {
            data: vec![1, 2, 3, 4, 5],
            account_count: 3,
        };
        let accounts = vec![
            SerializableAccountMeta {
                pubkey: Pubkey::new_from_array([1u8; 32]),
                is_signer: true,
                is_writable: false,
            },
            SerializableAccountMeta {
                pubkey: Pubkey::new_from_array([2u8; 32]),
                is_signer: false,
                is_writable: true,
            },
        ];

        let result = CalldataWithAccounts::new(calldata, accounts);
        assert!(result.is_err());
    }
}
