use std::ops::Deref;

use anchor_lang::error::ERROR_CODE_OFFSET;
use anchor_lang::{Event, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account;
use anchor_spl::token::spl_token;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use derive_more::{Deref, DerefMut};
use eco_routes::encoding;
use eco_routes::error::EcoRoutesError;
use eco_routes::state::{
    Call, Intent, Reward, Route, TokenAmount, MAX_CALLS, MAX_REWARD_TOKENS, MAX_ROUTE_TOKENS,
};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_sdk::clock::Clock;
use solana_sdk::instruction::{Instruction, InstructionError};
use solana_sdk::message::Message;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_program;
use solana_sdk::transaction::{Transaction, TransactionError};

const ECO_ROUTES_BIN: &[u8] = include_bytes!("../../../../target/deploy/eco_routes.so");

type TransactionResult = Result<TransactionMetadata, Box<FailedTransactionMetadata>>;

#[derive(Deref, DerefMut)]
pub struct Context {
    #[deref]
    #[deref_mut]
    svm: LiteSVM,
    mint_authority: Keypair,
    pub creator: Keypair,
    pub payer: Keypair,
    pub funder: Keypair,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(eco_routes::ID, ECO_ROUTES_BIN);

        let mint_authority = Keypair::new();
        let creator = Keypair::new();
        let payer = Keypair::new();
        let funder = Keypair::new();

        svm.airdrop(&mint_authority.pubkey(), sol_amount(100.0))
            .unwrap();
        svm.airdrop(&creator.pubkey(), sol_amount(10.0)).unwrap();
        svm.airdrop(&payer.pubkey(), sol_amount(10.0)).unwrap();
        svm.airdrop(&funder.pubkey(), sol_amount(10.0)).unwrap();

        Self {
            svm,
            mint_authority,
            creator,
            payer,
            funder,
        }
    }

    pub fn now(&self) -> i64 {
        self.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn balance(&self, pubkey: &Pubkey) -> u64 {
        self.get_account(pubkey)
            .map(|account| account.lamports)
            .unwrap_or_default()
    }

    pub fn token_balance(&self, pubkey: &Pubkey) -> u64 {
        self.get_account(pubkey)
            .and_then(|account| spl_token::state::Account::unpack(&account.data).ok())
            .map(|account| account.amount)
            .unwrap_or_default()
    }

    pub fn account<T: anchor_lang::AnchorDeserialize + anchor_lang::Discriminator>(
        &self,
        pubkey: &Pubkey,
    ) -> Option<T> {
        self.get_account(pubkey).map(|account| {
            anchor_lang::AnchorDeserialize::deserialize(&mut &account.data.as_slice()[8..]).unwrap()
        })
    }

    pub fn rand_intent(&mut self) -> Intent {
        let salt = rand::random();
        let destination_domain_id = 2;
        let inbox = [4; 32];
        let route_tokens = (0..MAX_ROUTE_TOKENS)
            .map(|_| TokenAmount {
                token: rand::random(),
                amount: token_amount(100.0),
            })
            .collect::<Vec<_>>();
        let calls = (0..MAX_CALLS)
            .map(|_| Call {
                destination: rand::random(),
                calldata: rand::random::<[u8; 32]>().to_vec(),
            })
            .collect::<Vec<_>>();
        let reward_tokens: Vec<_> = (0..MAX_REWARD_TOKENS)
            .map(|_| Keypair::new().pubkey())
            .map(|mint| TokenAmount {
                token: mint.to_bytes(),
                amount: token_amount(5.0),
            })
            .collect();
        let native_reward = sol_amount(1.0);
        let deadline = self.now() + 3600;
        let route = Route::new(
            salt,
            destination_domain_id,
            inbox,
            route_tokens.clone(),
            calls.clone(),
        )
        .unwrap();
        let reward = Reward::new(
            reward_tokens.clone(),
            self.creator.pubkey(),
            native_reward,
            deadline,
            self.get_sysvar(),
        )
        .unwrap();
        let intent_hash = encoding::intent_hash(&route, &reward);
        let bump = Intent::pda(intent_hash).1;

        reward_tokens.iter().for_each(|token| {
            self.set_mint_account(&Pubkey::new_from_array(token.token));
        });

        Intent::new(intent_hash, route, reward, bump).unwrap()
    }

    pub fn publish_intent(&mut self, intent: &Intent) -> TransactionResult {
        let args = eco_routes::instructions::PublishIntentArgs {
            salt: intent.route.salt,
            intent_hash: intent.intent_hash,
            destination_domain_id: intent.route.destination_domain_id,
            inbox: intent.route.inbox,
            route_tokens: intent.route.tokens.clone(),
            calls: intent.route.calls.clone(),
            reward_tokens: intent.reward.tokens.clone(),
            native_reward: intent.reward.native_amount,
            deadline: intent.reward.deadline,
        };
        let instruction = eco_routes::instruction::PublishIntent { args };
        let account_metas = eco_routes::accounts::PublishIntent {
            intent: Intent::pda(intent.intent_hash).0,
            creator: self.creator.pubkey(),
            payer: self.payer.pubkey(),
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: eco_routes::ID,
            accounts: account_metas,
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, &self.creator],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn fund_intent_native(&mut self, intent_hash: [u8; 32]) -> TransactionResult {
        let args = eco_routes::instructions::FundIntentNativeArgs { intent_hash };
        let instruction = eco_routes::instruction::FundIntentNative { args };
        let account_metas = eco_routes::accounts::FundIntentNative {
            intent: Intent::pda(intent_hash).0,
            funder: self.funder.pubkey(),
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: eco_routes::ID,
            accounts: account_metas,
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, &self.funder],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn fund_intent_spl(&mut self, intent_hash: [u8; 32], mint: &Pubkey) -> TransactionResult {
        let args = eco_routes::instructions::FundIntentSplArgs { intent_hash };
        let instruction = eco_routes::instruction::FundIntentSpl { args };

        let funder_token = get_associated_token_address_with_program_id(
            &self.funder.pubkey(),
            mint,
            &spl_token::ID,
        );

        let vault_seeds = &[b"reward", intent_hash.as_ref(), mint.as_ref()];
        let vault_pda = Pubkey::find_program_address(vault_seeds, &eco_routes::ID).0;

        let account_metas = eco_routes::accounts::FundIntentSpl {
            intent: Intent::pda(intent_hash).0,
            funder_token,
            vault: vault_pda,
            mint: *mint,
            funder: self.funder.pubkey(),
            payer: self.payer.pubkey(),
            system_program: system_program::ID,
            token_program: spl_token::ID,
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: eco_routes::ID,
            accounts: account_metas,
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, &self.funder],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn refund_intent_native(&mut self, intent_hash: [u8; 32]) -> TransactionResult {
        let args = eco_routes::instructions::RefundIntentNativeArgs { intent_hash };
        let instruction = eco_routes::instruction::RefundIntentNative { args };
        let account_metas = eco_routes::accounts::RefundIntentNative {
            intent: Intent::pda(intent_hash).0,
            refundee: self.creator.pubkey(),
            payer: self.payer.pubkey(),
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: eco_routes::ID,
            accounts: account_metas,
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, &self.creator],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn refund_intent_spl(&mut self, intent_hash: [u8; 32], mint: &Pubkey) -> TransactionResult {
        let args = eco_routes::instructions::RefundIntentSplArgs { intent_hash };
        let instruction = eco_routes::instruction::RefundIntentSpl { args };

        let vault_seeds = &[b"reward", intent_hash.as_ref(), mint.as_ref()];
        let vault_pda = Pubkey::find_program_address(vault_seeds, &eco_routes::ID).0;

        let refundee_token = get_associated_token_address_with_program_id(
            &self.creator.pubkey(),
            mint,
            &spl_token::ID,
        );
        self.airdrop_token(mint, &self.creator.pubkey(), 0);

        let account_metas = eco_routes::accounts::RefundIntentSpl {
            intent: Intent::pda(intent_hash).0,
            vault: vault_pda,
            refundee_token,
            mint: *mint,
            refundee: self.creator.pubkey(),
            payer: self.payer.pubkey(),
            system_program: system_program::ID,
            token_program: spl_token::ID,
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: eco_routes::ID,
            accounts: account_metas,
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, &self.creator],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    fn send_transaction(&mut self, transaction: Transaction) -> TransactionResult {
        let result = self.svm.send_transaction(transaction);
        self.expire_blockhash();
        let slot = self.get_sysvar::<Clock>().slot;
        self.warp_to_slot(slot + 1);

        result.map_err(Box::new)
    }

    pub fn set_mint_account(&mut self, mint: &Pubkey) {
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

        let mint_account = solana_sdk::account::Account {
            lamports: self
                .get_sysvar::<Rent>()
                .minimum_balance(spl_token::state::Mint::LEN),
            data: mint_data.to_vec(),
            owner: spl_token::ID,
            executable: false,
            rent_epoch: 0,
        };
        self.set_account(*mint, mint_account).unwrap();
    }

    pub fn airdrop_token(&mut self, mint: &Pubkey, recipient: &Pubkey, amount: u64) {
        let recipient_token =
            get_associated_token_address_with_program_id(recipient, mint, &spl_token::ID);

        let mut instructions = if self.get_account(&recipient_token).is_none() {
            vec![create_associated_token_account(
                &self.mint_authority.pubkey(),
                recipient,
                mint,
                &spl_token::ID,
            )]
        } else {
            vec![]
        };
        instructions.push(
            spl_token::instruction::mint_to(
                &spl_token::ID,
                mint,
                &recipient_token,
                &self.mint_authority.pubkey(),
                &[],
                amount,
            )
            .unwrap(),
        );

        let transaction = Transaction::new(
            &[&self.mint_authority],
            Message::new(&instructions, Some(&self.mint_authority.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction).unwrap();
    }

    pub fn expire_intent(&mut self, intent_hash: [u8; 32]) {
        let intent_pda = Intent::pda(intent_hash).0;
        let intent: Intent = self.account(&intent_pda).unwrap();

        self.warp_to_timestamp(intent.reward.deadline + 1);
    }

    fn warp_to_timestamp(&mut self, unix_timestamp: i64) {
        let mut clock = self.get_sysvar::<Clock>();
        clock.unix_timestamp = unix_timestamp;

        self.set_sysvar(&clock);
    }
}

pub fn assert_contains_event<E>(tx: TransactionMetadata, expected: E)
where
    E: Event,
{
    let expected = STANDARD.encode(expected.data());

    assert!(tx
        .logs
        .iter()
        .any(|log| { log.contains(format!("Program data: {}", expected).as_str()) }))
}

pub fn sol_amount(amount: f64) -> u64 {
    (amount * 1_000_000_000.0) as u64
}

pub fn token_amount(amount: f64) -> u64 {
    (amount * 1_000_000.0) as u64
}

pub fn is_eco_routes_error<T>(expected: EcoRoutesError) -> impl Fn(T) -> bool
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
