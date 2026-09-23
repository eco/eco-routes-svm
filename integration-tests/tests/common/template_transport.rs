//! SBF-tested generic staging, not a claim that the solver already submits this way.
use anchor_lang::prelude::AccountMeta;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use super::{Context, TransactionResult};

const BUFFER_ID: Pubkey = Pubkey::new_from_array([0xb5; 32]);
const BUFFER_BIN: &[u8] = include_bytes!("../../../target/deploy/template_buffer.so");
pub const PACKET_BYTES: usize = 1232;

pub fn transaction_size(tx: &Transaction) -> usize {
    // Solana transaction wire encoding: compact signature count, signatures,
    // followed by the SDK's serialized legacy message. These tests have <=2 signers.
    assert!(tx.signatures.len() < 128);
    1 + 64 * tx.signatures.len() + tx.message.serialize().len()
}

impl Context {
    /// Stages a single instruction only when its serialized transaction is too
    /// large. Every actual submitted transaction is checked against 1232 bytes.
    pub fn send_template_instruction(&mut self, instruction: Instruction) -> TransactionResult {
        let payer = self.payer.insecure_clone();
        let budget = ComputeBudgetInstruction::set_compute_unit_limit(self.compute_limit);
        let direct = Transaction::new(
            &[&payer],
            Message::new(
                &[budget.clone(), instruction.clone()],
                Some(&payer.pubkey()),
            ),
            self.latest_blockhash(),
        );
        let direct_size = transaction_size(&direct);
        if direct_size <= PACKET_BYTES {
            return self.send_transaction(direct);
        }
        let (execute, largest_write) = self.stage_template_instruction(instruction);
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[budget, execute], Some(&payer.pubkey())),
            self.latest_blockhash(),
        );
        let execute_size = transaction_size(&tx);
        assert!(execute_size <= PACKET_BYTES);
        let result = self.send_transaction(tx);
        if let Ok(meta) = &result {
            let logs: usize = meta.logs.iter().map(String::len).sum();
            println!("template transport: direct={direct_size}, largest_write={largest_write}, execute={execute_size}, CU={}, logs={logs}", meta.compute_units_consumed);
            for line in meta
                .logs
                .iter()
                .filter(|line| line.contains("chainer heap"))
            {
                println!("{line}");
            }
        }
        result
    }

    /// Returns the small execute instruction so tests can combine/sequence it
    /// with other real instructions without ever submitting an oversized packet.
    pub fn stage_template_instruction(&mut self, instruction: Instruction) -> (Instruction, usize) {
        self.add_program(BUFFER_ID, BUFFER_BIN).unwrap();
        let buffer = Keypair::new();
        let payer = self.payer.insecure_clone();
        let size = instruction.data.len();
        let create = solana_system_interface::instruction::create_account(
            &payer.pubkey(),
            &buffer.pubkey(),
            self.get_sysvar::<solana_sdk::rent::Rent>()
                .minimum_balance(size),
            size as u64,
            &BUFFER_ID,
        );
        let tx = Transaction::new(
            &[&payer, &buffer],
            Message::new(&[create], Some(&payer.pubkey())),
            self.latest_blockhash(),
        );
        assert!(transaction_size(&tx) <= PACKET_BYTES);
        self.send_transaction(tx).unwrap();
        let mut largest_write = 0;
        for (index, chunk) in instruction.data.chunks(800).enumerate() {
            let mut data = ((index * 800) as u32).to_le_bytes().to_vec();
            data.extend_from_slice(chunk);
            let write = Instruction {
                program_id: BUFFER_ID,
                accounts: vec![AccountMeta::new(buffer.pubkey(), true)],
                data,
            };
            let tx = Transaction::new(
                &[&payer, &buffer],
                Message::new(&[write], Some(&payer.pubkey())),
                self.latest_blockhash(),
            );
            largest_write = largest_write.max(transaction_size(&tx));
            assert!(transaction_size(&tx) <= PACKET_BYTES);
            self.send_transaction(tx).unwrap();
        }
        let accounts = [
            vec![
                AccountMeta::new_readonly(buffer.pubkey(), false),
                AccountMeta::new_readonly(instruction.program_id, false),
            ],
            instruction.accounts,
        ]
        .concat();
        (
            Instruction {
                program_id: BUFFER_ID,
                accounts,
                data: vec![],
            },
            largest_write,
        )
    }
}
