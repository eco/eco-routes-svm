use std::ops::Deref;

use anchor_lang::{AnchorSerialize, Discriminator, Event, Space};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account;
use anchor_spl::token::{self, spl_token};
use anchor_spl::token_2022::{self, spl_token_2022};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::Proof;
use hyper_prover::state::ProofAccount;
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use portal::state::WithdrawnMarker;
use portal::types::{Call, Reward, Route, TokenAmount};
use rand::random;
use solana_sdk::clock::Clock;
use solana_sdk::instruction::InstructionError;
use solana_sdk::message::Message;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::{Transaction, TransactionError};

mod hyper_prover_context;
mod hyperlane_context;
mod local_prover_context;
mod portal_context;

const COMPUTE_UNIT_LIMIT: u32 = 400_000;
const PORTAL_BIN: &[u8] = include_bytes!("../../../target/deploy/portal.so");
const HYPER_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/hyper_prover.so");
const LOCAL_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/local_prover.so");

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
    pub solver: Keypair,
    pub sender: Keypair,
}

impl Default for Context {
    fn default() -> Self {
        let mut svm = LiteSVM::new();

        svm.add_program(portal::ID, PORTAL_BIN);
        svm.add_program(hyper_prover::ID, HYPER_PROVER_BIN);
        svm.add_program(local_prover::ID, LOCAL_PROVER_BIN);

        hyperlane_context::add_hyperlane_programs(&mut svm);
        hyperlane_context::init_hyperlane(&mut svm);

        let mint_authority = Keypair::new();
        let creator = Keypair::new();
        let payer = Keypair::new();
        let funder = Keypair::new();
        let solver = Keypair::new();
        let sender = Keypair::new();

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
            solver,
            sender,
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

    pub fn now(&self) -> u64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp as u64
    }

    pub fn rand_intent(&mut self) -> (u64, Route, Reward) {
        let route_tokens: Vec<_> = (0..2)
            .map(|i| TokenAmount {
                token: Pubkey::new_unique(),
                amount: (i + 1) * 1_000_000,
            })
            .collect();
        let reward_tokens: Vec<_> = (0..2)
            .map(|i| TokenAmount {
                token: Pubkey::new_unique(),
                amount: (i + 1) * 1_000_000,
            })
            .collect();

        reward_tokens.iter().for_each(|token| {
            self.set_mint_account(&token.token);
        });
        route_tokens.iter().for_each(|token| {
            self.set_mint_account(&token.token);
        });

        let calls: Vec<_> = (0..3)
            .map(|_| Call {
                target: random::<[u8; 32]>().into(),
                data: random::<[u8; 32]>().to_vec(),
            })
            .collect();

        (
            random::<u32>().into(),
            Route {
                deadline: self.now() + 1800,
                salt: random::<[u8; 32]>().into(),
                portal: portal::ID.to_bytes().into(),
                native_amount: sol_amount(1.0),
                tokens: route_tokens,
                calls,
            },
            Reward {
                deadline: self.now() + 3600,
                creator: self.creator.pubkey(),
                prover: hyper_prover::ID,
                native_amount: sol_amount(1.0),
                tokens: reward_tokens,
            },
        )
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

    pub fn set_proof(&mut self, proof_pda: Pubkey, proof: Proof, owner: Pubkey) {
        let mut data = Vec::new();
        data.extend_from_slice(ProofAccount::DISCRIMINATOR);
        proof.serialize(&mut data).unwrap();

        let account = solana_sdk::account::Account {
            lamports: self.get_sysvar::<Rent>().minimum_balance(data.len()),
            data,
            owner,
            executable: false,
            rent_epoch: self
                .get_sysvar::<Rent>()
                .minimum_balance(8 + ProofAccount::INIT_SPACE),
        };

        self.set_account(proof_pda, account).unwrap();
    }

    pub fn set_withdrawn_marker(&mut self, withdrawn_marker_pda: Pubkey) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 8]);

        let account = solana_sdk::account::Account {
            lamports: WithdrawnMarker::min_balance(self.get_sysvar()),
            data,
            owner: portal::ID,
            executable: false,
            rent_epoch: 0,
        };

        self.set_account(withdrawn_marker_pda, account).unwrap();
    }

    pub fn warp_to_timestamp(&mut self, unix_timestamp: i64) {
        let mut clock = self.get_sysvar::<Clock>();
        clock.unix_timestamp = unix_timestamp;

        self.set_sysvar(&clock);
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

pub fn contains_cpi_event<E>(expected: E) -> impl Fn(TransactionMetadata) -> bool
where
    E: Event,
{
    let expected = expected.data();

    move |actual: TransactionMetadata| {
        actual
            .inner_instructions
            .iter()
            .flat_map(|inner_ix_list| inner_ix_list.iter())
            .any(
                |inner_instruction| match inner_instruction.instruction.data.get(8..) {
                    Some(data) => data == expected,
                    None => false,
                },
            )
    }
}

pub fn contains_event_and_msg<E, M>(expected: E, msg: M) -> impl Fn(TransactionMetadata) -> bool
where
    E: Event,
    M: ToString,
{
    let expected = STANDARD.encode(expected.data());

    move |actual: TransactionMetadata| {
        actual
            .logs
            .iter()
            .any(|log| log.contains(format!("Program data: {}", expected).as_str()))
            && actual
                .logs
                .iter()
                .any(|log| log.contains(msg.to_string().as_str()))
    }
}

pub fn is_error<T, Err>(expected: Err) -> impl Fn(T) -> bool
where
    T: Deref<Target = FailedTransactionMetadata>,
    Err: Into<u32>,
{
    let expected = expected.into();

    move |actual: T| match actual.err {
        TransactionError::InstructionError(_, InstructionError::Custom(error_code)) => {
            error_code == expected
        }
        _ => false,
    }
}
