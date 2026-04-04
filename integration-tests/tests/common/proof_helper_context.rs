use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use litesvm::LiteSVM;
use proof_helper::igp;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult, COMPUTE_UNIT_LIMIT};
use solana_sdk::compute_budget::ComputeBudgetInstruction;

const PROOF_HELPER_BIN: &[u8] = include_bytes!("../../../target/deploy/proof_helper.so");
const MOCK_IGP_BIN: &[u8] = include_bytes!("../../../target/deploy/mock_igp.so");

pub fn add_proof_helper_programs(svm: &mut LiteSVM) {
    svm.add_program(proof_helper::ID, PROOF_HELPER_BIN);
    svm.add_program(igp::IGP_PROGRAM_ID, MOCK_IGP_BIN);
}

/// Derive the IGP program data PDA (seeds from Hyperlane IGP).
pub fn igp_program_data_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[b"hyperlane_igp", b"-", b"program_data"],
        &igp::IGP_PROGRAM_ID,
    )
    .0
}

/// Derive the gas payment PDA from a unique gas payment pubkey.
pub fn gas_payment_pda(unique_gas_payment: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"hyperlane_igp",
            b"-",
            b"gas_payment",
            b"-",
            unique_gas_payment.as_ref(),
        ],
        &igp::IGP_PROGRAM_ID,
    )
    .0
}

/// Derive the IGP account PDA from a salt.
pub fn igp_account_pda(salt: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(
        &[b"hyperlane_igp", b"-", b"igp", b"-", salt.as_ref()],
        &igp::IGP_PROGRAM_ID,
    )
    .0
}

#[derive(Deref, DerefMut)]
pub struct ProofHelper<'a>(&'a mut Context);

impl Context {
    pub fn proof_helper(&mut self) -> ProofHelper {
        ProofHelper(self)
    }
}

impl ProofHelper<'_> {
    pub fn pay_for_gas(
        &mut self,
        dispatched_message: Pubkey,
        destination_domain: u32,
        gas_amount: u64,
    ) -> TransactionResult {
        let unique_gas_payment = Keypair::new();
        self.pay_for_gas_with_keypair(
            dispatched_message,
            destination_domain,
            gas_amount,
            &unique_gas_payment,
        )
    }

    pub fn pay_for_gas_with_keypair(
        &mut self,
        dispatched_message: Pubkey,
        destination_domain: u32,
        gas_amount: u64,
        unique_gas_payment: &Keypair,
    ) -> TransactionResult {
        let salt = [0u8; 32];
        let args = proof_helper::instructions::PayForGasArgs {
            destination_domain,
            gas_amount,
        };
        let instruction = proof_helper::instruction::PayForGas { args };
        let accounts: Vec<_> = proof_helper::accounts::PayForGas {
            dispatched_message,
            payer: self.payer.pubkey(),
            igp_program_data: igp_program_data_pda(),
            unique_gas_payment: unique_gas_payment.pubkey(),
            gas_payment_pda: gas_payment_pda(&unique_gas_payment.pubkey()),
            igp_account: igp_account_pda(&salt),
            overhead_igp: None,
            system_program: anchor_lang::system_program::ID,
            igp_program: igp::IGP_PROGRAM_ID,
        }
        .to_account_metas(None);

        let instruction = Instruction {
            program_id: proof_helper::ID,
            accounts,
            data: instruction.data(),
        };
        let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT);

        let transaction = Transaction::new(
            &[&self.payer, unique_gas_payment],
            Message::new(&[compute_budget, instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
