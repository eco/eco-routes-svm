use std::ops::Deref;

use anchor_lang::error::ERROR_CODE_OFFSET;
use anchor_lang::prelude::AccountMeta;
use anchor_lang::{Event, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account;
use anchor_spl::token::{self, spl_token};
use anchor_spl::token_2022::{self, spl_token_2022};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use derive_more::{Deref, DerefMut};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use portal::instructions::PortalError;
use portal::types::{Bytes32, Call, Intent, Reward, Route, TokenAmount};
use rand::random;
use solana_sdk::clock::Clock;
use solana_sdk::instruction::{Instruction, InstructionError};
use solana_sdk::message::Message;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::{Transaction, TransactionError};

const PORTAL_BIN: &[u8] = include_bytes!("../../../../target/deploy/portal.so");

type TransactionResult = Result<TransactionMetadata, Box<FailedTransactionMetadata>>;

#[derive(Deref, DerefMut)]
pub struct Context {
    #[deref]
    #[deref_mut]
    svm: LiteSVM,
    mint_authority: Keypair,
    pub token_program: Pubkey,
    pub creator: Keypair,
    pub payer: Keypair,
    pub funder: Keypair,
}

impl Default for Context {
    fn default() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(portal::ID, PORTAL_BIN);

        let mint_authority = Keypair::new();
        let creator = Keypair::new();
        let payer = Keypair::new();
        let funder = Keypair::new();

        svm.airdrop(&mint_authority.pubkey(), sol_amount(100.0))
            .unwrap();
        svm.airdrop(&payer.pubkey(), sol_amount(10.0)).unwrap();

        Self {
            svm,
            mint_authority,
            token_program: token::ID,
            creator,
            payer,
            funder,
        }
    }
}

impl Context {
    pub fn new_with_token_2022() -> Self {
        Self {
            token_program: token_2022::ID,
            ..Default::default()
        }
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn rand_intent(&mut self) -> Intent {
        let reward_tokens: Vec<_> = (0..2)
            .map(|_| TokenAmount {
                token: Pubkey::new_unique(),
                amount: random(),
            })
            .collect();

        reward_tokens.iter().for_each(|token| {
            self.set_mint_account(&token.token);
        });

        Intent {
            destination_chain: random(),
            route: Route {
                salt: random(),
                destination_chain_portal: random(),
                tokens: (0..3)
                    .map(|_| TokenAmount {
                        token: Pubkey::new_unique(),
                        amount: random(),
                    })
                    .collect(),
                calls: (0..3)
                    .map(|_| Call {
                        target: random(),
                        data: random::<Bytes32>().to_vec(),
                    })
                    .collect(),
            },
            reward: Reward {
                deadline: self.now() + 3600,
                creator: self.creator.pubkey(),
                prover: random(),
                native_amount: sol_amount(1.0),
                tokens: reward_tokens,
            },
        }
    }

    pub fn set_mint_account(&mut self, mint: &Pubkey) {
        let mint_account = if self.token_program == token::ID {
            let mut mint_data = [0u8; spl_token::state::Mint::LEN];
            spl_token::state::Mint::pack(
                spl_token::state::Mint {
                    decimals: 6,
                    is_initialized: true,
                    mint_authority: Some(self.mint_authority.pubkey()).into(),
                    supply: 0,
                    freeze_authority: None.into(),
                },
                &mut mint_data,
            )
            .unwrap();

            solana_sdk::account::Account {
                lamports: self
                    .get_sysvar::<Rent>()
                    .minimum_balance(spl_token::state::Mint::LEN),
                data: mint_data.to_vec(),
                owner: self.token_program,
                executable: false,
                rent_epoch: 0,
            }
        } else {
            let mut mint_data = [0u8; spl_token_2022::state::Mint::LEN];
            spl_token_2022::state::Mint::pack(
                spl_token_2022::state::Mint {
                    decimals: 6,
                    is_initialized: true,
                    mint_authority: Some(self.mint_authority.pubkey()).into(),
                    supply: 0,
                    freeze_authority: None.into(),
                },
                &mut mint_data,
            )
            .unwrap();

            solana_sdk::account::Account {
                lamports: self
                    .get_sysvar::<Rent>()
                    .minimum_balance(spl_token_2022::state::Mint::LEN),
                data: mint_data.to_vec(),
                owner: self.token_program,
                executable: false,
                rent_epoch: 0,
            }
        };

        self.set_account(*mint, mint_account).unwrap();
    }

    pub fn airdrop_token_ata(&mut self, mint: &Pubkey, recipient: &Pubkey, amount: u64) {
        let recipient_token =
            get_associated_token_address_with_program_id(recipient, mint, &self.token_program);

        let mut instructions = if self.get_account(&recipient_token).is_none() {
            vec![create_associated_token_account(
                &self.mint_authority.pubkey(),
                recipient,
                mint,
                &self.token_program,
            )]
        } else {
            vec![]
        };

        match self.token_program {
            token::ID => {
                instructions.push(
                    spl_token::instruction::mint_to(
                        &self.token_program,
                        mint,
                        &recipient_token,
                        &self.mint_authority.pubkey(),
                        &[],
                        amount,
                    )
                    .unwrap(),
                );
            }
            token_2022::ID => {
                instructions.push(
                    spl_token_2022::instruction::mint_to(
                        &self.token_program,
                        mint,
                        &recipient_token,
                        &self.mint_authority.pubkey(),
                        &[],
                        amount,
                    )
                    .unwrap(),
                );
            }
            _ => panic!("unsupported token program"),
        }

        let transaction = Transaction::new(
            &[&self.mint_authority],
            Message::new(&instructions, Some(&self.mint_authority.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction).unwrap();
    }

    pub fn publish_intent(&mut self, intent: &Intent, route_hash: Bytes32) -> TransactionResult {
        let args = portal::instructions::PublishArgs {
            intent: intent.clone(),
            route_hash,
        };
        let instruction = portal::instruction::Publish { args };
        let accounts: Vec<_> = portal::accounts::Publish {}.to_account_metas(None);
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn fund_intent(
        &mut self,
        intent: &Intent,
        vault: Pubkey,
        route_hash: Bytes32,
        allow_partial: bool,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::FundArgs {
            destination_chain: intent.destination_chain,
            route_hash,
            reward: intent.reward.clone(),
            allow_partial,
        };
        let instruction = portal::instruction::Fund { args };
        let accounts: Vec<_> = portal::accounts::Fund {
            payer: self.payer.pubkey(),
            funder: self.funder.pubkey(),
            vault,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, &self.funder],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn balance(&self, pubkey: &Pubkey) -> u64 {
        self.svm.get_balance(pubkey).unwrap_or_default()
    }

    pub fn token_balance(&self, pubkey: &Pubkey) -> u64 {
        self.get_account(pubkey)
            .and_then(|account| {
                if self.token_program == token::ID {
                    spl_token::state::Account::unpack(&account.data)
                        .ok()
                        .map(|acc| acc.amount)
                } else if self.token_program == token_2022::ID {
                    spl_token_2022::extension::StateWithExtensions::<spl_token_2022::state::Account>::unpack(&account.data)
                        .ok()
                        .map(|state| state.base.amount)
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn token_balance_ata(&self, mint: &Pubkey, pubkey: &Pubkey) -> u64 {
        self.token_balance(&get_associated_token_address_with_program_id(
            pubkey,
            mint,
            &self.token_program,
        ))
    }

    pub fn account<T: anchor_lang::AccountDeserialize>(&self, pubkey: &Pubkey) -> Option<T> {
        self.svm
            .get_account(pubkey)
            .and_then(|account| T::try_deserialize(&mut account.data.as_slice()).ok())
    }

    fn send_transaction(&mut self, transaction: Transaction) -> TransactionResult {
        let result = self.svm.send_transaction(transaction);
        self.expire_blockhash();
        let slot = self.svm.get_sysvar::<Clock>().slot;
        self.svm.warp_to_slot(slot + 1);

        result.map_err(Box::new)
    }
}

pub fn sol_amount(amount: f64) -> u64 {
    (amount * 1_000_000_000.0) as u64
}

pub fn contains_event<E>(expected: E) -> impl Fn(TransactionMetadata) -> bool
where
    E: Event,
{
    let expected = STANDARD.encode(expected.data());

    move |actual: TransactionMetadata| {
        actual
            .logs
            .iter()
            .any(|log| log.contains(format!("Program data: {}", expected).as_str()))
    }
}

pub fn is_portal_error<T>(expected: PortalError) -> impl Fn(T) -> bool
where
    T: Deref<Target = FailedTransactionMetadata>,
{
    move |actual: T| match actual.err {
        TransactionError::InstructionError(_, InstructionError::Custom(error_code)) => {
            error_code == ERROR_CODE_OFFSET + expected as u32
        }
        _ => false,
    }
}
