use anchor_lang::prelude::borsh::BorshDeserialize;
use anchor_lang::prelude::AccountMeta;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::ProveArgs;
use eco_svm_std::{Bytes32, SerializableAccountMeta};
use hyper_prover::hyperlane;
use hyper_prover::state::dispatcher_pda;
// import hyper_prover
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{sol_amount, Context, TransactionResult};

#[derive(Deref, DerefMut)]
pub struct HyperProver<'a>(&'a mut Context);

impl Context {
    pub fn hyper_prover(&mut self) -> HyperProver {
        HyperProver(self)
    }
}

impl HyperProver<'_> {
    pub fn init(&mut self, whitelisted_senders: Vec<Bytes32>, config: Pubkey) -> TransactionResult {
        let args = hyper_prover::instructions::InitArgs {
            whitelisted_senders,
        };
        let instruction = hyper_prover::instruction::Init { args };
        let accounts: Vec<_> = hyper_prover::accounts::Init {
            config,
            payer: self.payer.pubkey(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None);
        let instruction = Instruction {
            program_id: hyper_prover::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn ism_account_metas(&mut self) -> Vec<AccountMeta> {
        let ism_account_metas_pda = Pubkey::find_program_address(
            &[
                b"hyperlane_message_recipient",
                b"-",
                b"interchain_security_module",
                b"-",
                b"account_metas",
            ],
            &hyper_prover::ID,
        )
        .0;

        let instruction = hyper_prover::instruction::IsmAccountMetas {};
        let accounts = hyper_prover::accounts::IsmAccountMetas {
            ism_account_metas: ism_account_metas_pda,
        };

        let instruction = Instruction {
            program_id: hyper_prover::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };

        let payer = Keypair::new();
        self.airdrop(&payer.pubkey(), sol_amount(1.0)).unwrap();

        let transaction = Transaction::new(
            &[&payer],
            Message::new(&[instruction], Some(&payer.pubkey())),
            self.latest_blockhash(),
        );

        let result = self.send_transaction(transaction).unwrap();

        let serializable_metas: Vec<SerializableAccountMeta> =
            BorshDeserialize::try_from_slice(&result.return_data.data).unwrap();

        serializable_metas
            .into_iter()
            .map(|meta| AccountMeta {
                pubkey: meta.pubkey,
                is_signer: meta.is_signer,
                is_writable: meta.is_writable,
            })
            .collect()
    }

    pub fn handle_account_metas(
        &mut self,
        origin: u32,
        sender: [u8; 32],
        payload: Vec<u8>,
    ) -> Vec<AccountMeta> {
        let handle_account_metas_pda = Pubkey::find_program_address(
            &[
                b"hyperlane_message_recipient",
                b"-",
                b"handle",
                b"-",
                b"account_metas",
            ],
            &hyper_prover::ID,
        )
        .0;
        let instruction = hyper_prover::instruction::HandleAccountMetas {
            origin,
            sender,
            payload,
        };
        let accounts = hyper_prover::accounts::HandleAccountMetas {
            handle_account_metas: handle_account_metas_pda,
        };
        let instruction = Instruction {
            program_id: hyper_prover::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        let result = self.send_transaction(transaction).unwrap();

        let serializable_metas: Vec<SerializableAccountMeta> =
            BorshDeserialize::try_from_slice(&result.return_data.data).unwrap();

        serializable_metas
            .into_iter()
            .map(|meta| AccountMeta {
                pubkey: meta.pubkey,
                is_signer: meta.is_signer,
                is_writable: meta.is_writable,
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prove(
        &mut self,
        portal_dispatcher: &Keypair,
        source: u64,
        intent_hash: Bytes32,
        data: Vec<u8>,
        claimant: Bytes32,
        outbox_pda: Pubkey,
        unique_message: &Keypair,
        dispatched_message_pda: Pubkey,
    ) -> TransactionResult {
        let args = ProveArgs {
            source,
            intent_hash,
            data,
            claimant,
        };
        let instruction = hyper_prover::instruction::Prove { args };
        let accounts = hyper_prover::accounts::Prove {
            portal_dispatcher: portal_dispatcher.pubkey(),
            dispatcher: dispatcher_pda().0,
            payer: self.payer.pubkey(),
            outbox_pda,
            spl_noop_program: spl_noop::ID,
            unique_message: unique_message.pubkey(),
            dispatched_message_pda,
            system_program: anchor_lang::system_program::ID,
            mailbox_program: hyperlane::MAILBOX_ID,
        };
        let instruction = Instruction {
            program_id: hyper_prover::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, portal_dispatcher, unique_message],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn close_proof(
        &mut self,
        portal_proof_closer: &Keypair,
        proof: Pubkey,
    ) -> TransactionResult {
        let instruction = hyper_prover::instruction::CloseProof {};
        let accounts = hyper_prover::accounts::CloseProof {
            portal_proof_closer: portal_proof_closer.pubkey(),
            proof,
            pda_payer: hyper_prover::state::pda_payer_pda().0,
        };
        let instruction = Instruction {
            program_id: hyper_prover::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, portal_proof_closer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
