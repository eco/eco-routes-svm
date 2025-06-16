use std::ops::Deref;

use anchor_lang::error::ERROR_CODE_OFFSET;
use anchor_lang::{Event, InstructionData, ToAccountMetas};
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
use solana_sdk::pubkey::Pubkey;
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
    pub creator: Keypair,
    pub payer: Keypair,
}

impl Default for Context {
    fn default() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(portal::ID, PORTAL_BIN);

        let creator = Keypair::new();
        let payer = Keypair::new();

        svm.airdrop(&creator.pubkey(), sol_amount(10.0)).unwrap();
        svm.airdrop(&payer.pubkey(), sol_amount(10.0)).unwrap();

        Self {
            svm,
            creator,
            payer,
        }
    }
}

impl Context {
    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn rand_intent(&mut self) -> Intent {
        Intent {
            route_chain: random(),
            route: Route {
                salt: random(),
                route_chain_portal: random(),
                tokens: (0..3)
                    .map(|_| TokenAmount {
                        token: random(),
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
                tokens: (0..3)
                    .map(|_| TokenAmount {
                        token: random(),
                        amount: random(),
                    })
                    .collect(),
            },
        }
    }

    pub fn publish_intent(&mut self, intent: &Intent, route_hash: Bytes32) -> TransactionResult {
        let args = portal::instructions::PublishArgs {
            intent: intent.clone(),
            route_hash,
        };
        let instruction = portal::instruction::Publish { args };
        let accounts: Vec<_> = portal::accounts::Publish {
            creator: self.creator.pubkey(),
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer, &self.creator],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn balance(&self, pubkey: &Pubkey) -> u64 {
        self.svm.get_balance(pubkey).unwrap_or_default()
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
