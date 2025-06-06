use std::u64;

use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::{
    associated_token::{
        get_associated_token_address_with_program_id,
        spl_associated_token_account::instruction::create_associated_token_account,
    },
    token::spl_token,
};
use derive_more::{Deref, DerefMut};
use eco_routes::{
    encoding,
    state::{Call, Intent, Reward, Route, TokenAmount, MAX_REWARD_TOKENS},
};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use solana_sdk::{
    clock::Clock, instruction::Instruction, message::Message, program_pack::Pack, pubkey::Pubkey,
    rent::Rent, signature::Keypair, signer::Signer, system_program, transaction::Transaction,
};

const ECO_ROUTES_BIN: &[u8] = include_bytes!("../../../../target/deploy/eco_routes.so");

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

impl Context {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(eco_routes::ID, &ECO_ROUTES_BIN);

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
        let account = self.get_account(pubkey).unwrap();

        spl_token::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    pub fn account<T: anchor_lang::AnchorDeserialize + anchor_lang::Discriminator>(
        &self,
        pubkey: &Pubkey,
    ) -> T {
        let account = self.get_account(pubkey).unwrap();

        anchor_lang::AnchorDeserialize::deserialize(&mut &account.data.as_slice()[8..]).unwrap()
    }

    pub fn rand_intent(&mut self) -> Intent {
        let salt = rand_bytes32();
        let destination_domain_id = 2;
        let inbox = [4; 32];
        let route_tokens = vec![
            TokenAmount {
                token: rand_bytes32(),
                amount: token_amount(100.0),
            },
            TokenAmount {
                token: rand_bytes32(),
                amount: token_amount(100.0),
            },
            TokenAmount {
                token: rand_bytes32(),
                amount: token_amount(100.0),
            },
        ];
        let calls = vec![
            Call {
                destination: rand_bytes32(),
                calldata: rand_bytes32().to_vec(),
            },
            Call {
                destination: rand_bytes32(),
                calldata: rand_bytes32().to_vec(),
            },
            Call {
                destination: rand_bytes32(),
                calldata: rand_bytes32().to_vec(),
            },
        ];
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

    pub fn publish_intent(
        &mut self,
        intent: &Intent,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
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

    pub fn fund_intent_native(
        &mut self,
        intent_hash: [u8; 32],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
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

    pub fn fund_intent_spl(
        &mut self,
        intent_hash: [u8; 32],
        mint: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
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

    fn send_transaction(
        &mut self,
        transaction: Transaction,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let result = self.svm.send_transaction(transaction);
        self.expire_blockhash();
        let slot = self.get_sysvar::<Clock>().slot;
        self.warp_to_slot(slot + 1);

        result
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
        let create_token_account_instruction = create_associated_token_account(
            &self.mint_authority.pubkey(),
            recipient,
            mint,
            &spl_token::ID,
        );
        let mint_instruction = spl_token::instruction::mint_to(
            &spl_token::ID,
            mint,
            &recipient_token,
            &self.mint_authority.pubkey(),
            &[],
            amount,
        )
        .unwrap();
        let transaction = Transaction::new(
            &[&self.mint_authority],
            Message::new(
                &[create_token_account_instruction, mint_instruction],
                Some(&self.mint_authority.pubkey()),
            ),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction).unwrap();
    }
}

pub fn sol_amount(amount: f64) -> u64 {
    (amount * 1_000_000_000.0) as u64
}

pub fn token_amount(amount: f64) -> u64 {
    (amount * 1_000_000.0) as u64
}

fn rand_bytes32() -> [u8; 32] {
    rand::random()
}
