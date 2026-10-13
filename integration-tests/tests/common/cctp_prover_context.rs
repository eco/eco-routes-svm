use std::iter;

use anchor_lang::{system_program, InstructionData, ToAccountMetas};
use cctp_prover::instructions::InitArgs;
use cctp_prover::state::{pda_payer_pda, Config, Peer};
use derive_more::{Deref, DerefMut};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{program_data_address, sol_amount, Context, TransactionResult};

pub const ETHEREUM_DOMAIN: u32 = 0;
pub const ETHEREUM_CHAIN_ID: u64 = 1;
pub const OP_DOMAIN: u32 = 2;
pub const BASE_DOMAIN: u32 = 6;
pub const BASE_CHAIN_ID: u64 = 8453;
pub const COMPUTE_UNIT_LIMIT: u32 = 400_000;

/// An EVM peer whose sender is a left-padded 20-byte address of `byte`s.
pub fn evm_peer(domain: u32, chain_id: u64, byte: u8) -> Peer {
    let mut sender = [0u8; 32];
    sender[12..].fill(byte);

    Peer {
        domain,
        chain_id,
        sender: sender.into(),
    }
}

/// Ethereum (domain 0, the case MessageBridgeProver's base rejects) and Base.
pub fn peers() -> Vec<Peer> {
    vec![
        evm_peer(ETHEREUM_DOMAIN, ETHEREUM_CHAIN_ID, 0xee),
        evm_peer(BASE_DOMAIN, BASE_CHAIN_ID, 0xba),
    ]
}

pub fn build_init_instruction(payer: &Pubkey, authority: &Pubkey, peers: Vec<Peer>) -> Instruction {
    Instruction {
        program_id: cctp_prover::ID,
        accounts: cctp_prover::accounts::Init {
            payer: *payer,
            authority: *authority,
            program: cctp_prover::ID,
            program_data: program_data_address(&cctp_prover::ID),
            config: Config::pda().0,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: cctp_prover::instruction::Init {
            args: InitArgs { peers },
        }
        .data(),
    }
}

#[derive(Deref, DerefMut)]
pub struct CctpProver<'a>(&'a mut Context);

impl Context {
    pub fn cctp_prover(&mut self) -> CctpProver<'_> {
        CctpProver(self)
    }
}

impl CctpProver<'_> {
    /// Makes `authority` the program's upgrade authority and funds `pda_payer`.
    pub fn install(&mut self, authority: Pubkey) {
        self.set_upgrade_authority(&cctp_prover::ID, Some(authority));
        self.airdrop(&pda_payer_pda().0, sol_amount(10.0)).unwrap();
    }

    /// Simulates `solana program set-upgrade-authority --final`.
    pub fn finalize(&mut self) {
        self.set_upgrade_authority(&cctp_prover::ID, None);
    }

    pub fn init(&mut self, authority: &Keypair, peers: Vec<Peer>) -> TransactionResult {
        let instruction = build_init_instruction(&self.payer.pubkey(), &authority.pubkey(), peers);

        self.send(vec![instruction], &[authority])
    }

    /// Sends `instructions` after a `COMPUTE_UNIT_LIMIT`, paid by the context
    /// payer plus any extra `signers`.
    pub fn send(
        &mut self,
        instructions: Vec<Instruction>,
        signers: &[&Keypair],
    ) -> TransactionResult {
        let payer = self.payer.insecure_clone();
        let instructions: Vec<_> = iter::once(ComputeBudgetInstruction::set_compute_unit_limit(
            COMPUTE_UNIT_LIMIT,
        ))
        .chain(instructions)
        .collect();
        let signers: Vec<&Keypair> = iter::once(&payer).chain(signers.iter().copied()).collect();
        let transaction = Transaction::new(
            &signers,
            Message::new(&instructions, Some(&payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
