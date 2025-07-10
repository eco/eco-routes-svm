use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::{Proof, ProveArgs};
use eco_svm_std::{event_authority_pda, Bytes32};
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult};

#[derive(Deref, DerefMut)]
pub struct LocalProver<'a>(&'a mut Context);

impl Context {
    pub fn local_prover(&mut self) -> LocalProver {
        LocalProver(self)
    }
}

impl LocalProver<'_> {
    pub fn prove(
        &mut self,
        portal_dispatcher: &Keypair,
        source_chain: u64,
        intent_hash: Bytes32,
        data: Vec<u8>,
        claimant: Bytes32,
    ) -> TransactionResult {
        let args = ProveArgs {
            source_chain,
            intent_hash,
            data,
            claimant,
        };
        let instruction = local_prover::instruction::Prove { args };
        let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
        let accounts = local_prover::accounts::Prove {
            portal_dispatcher: portal_dispatcher.pubkey(),
            proof,
            payer: self.payer.pubkey(),
            system_program: anchor_lang::system_program::ID,
            event_authority: event_authority_pda(&local_prover::ID).0,
            program: local_prover::ID,
        };
        let instruction = Instruction {
            program_id: local_prover::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, portal_dispatcher],
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
        let instruction = local_prover::instruction::CloseProof {};
        let accounts = local_prover::accounts::CloseProof {
            portal_proof_closer: portal_proof_closer.pubkey(),
            proof,
            payer: self.payer.pubkey(),
        };
        let instruction = Instruction {
            program_id: local_prover::ID,
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
