use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::{event_authority_pda, Bytes32};
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::{abi_encode_bytes, INTENT_FULFILLED_FROM_SOURCE_SELECTOR};
use polymer_prover::polymer;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult};

/// Polymer's `load_proof` chunk size recommendation.
const LOAD_PROOF_CHUNK: usize = 800;

#[derive(Deref, DerefMut)]
pub struct PolymerProver<'a>(&'a mut Context);

impl Context {
    pub fn polymer_prover(&mut self) -> PolymerProver<'_> {
        PolymerProver(self)
    }
}

/// Builds the result Polymer would write for an `IntentFulfilledFromSource`
/// event emitted by `emitter` on EVM chain `chain_id`, with `source` in the
/// indexed topic and `encoded_proofs` ABI-encoded as the `bytes` argument.
pub fn intent_fulfilled_result(
    emitter: [u8; 20],
    source: u64,
    chain_id: u32,
    encoded_proofs: Vec<u8>,
) -> ValidationResultAccount {
    let mut topics = INTENT_FULFILLED_FROM_SOURCE_SELECTOR.to_vec();
    topics.extend_from_slice(&[0u8; 24]);
    topics.extend_from_slice(&source.to_be_bytes());

    ValidationResultAccount {
        is_valid: true,
        error_message: String::new(),
        chain_id,
        emitting_contract: emitter,
        topics,
        unindexed_data: abi_encode_bytes(&encoded_proofs),
    }
}

impl PolymerProver<'_> {
    /// Compute limit for `validate`: Polymer's real `validate_event` needs
    /// close to the 1.4M transaction maximum.
    pub const VALIDATE_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

    pub fn init(
        &mut self,
        whitelisted_emitters: Vec<Bytes32>,
        config: Pubkey,
    ) -> TransactionResult {
        let instruction = Instruction {
            program_id: polymer_prover::ID,
            accounts: polymer_prover::accounts::Init {
                config,
                payer: self.payer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: polymer_prover::instruction::Init {
                args: polymer_prover::instructions::InitArgs {
                    whitelisted_emitters,
                },
            }
            .data(),
        };
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// `validate` for the proof loaded under `authority`; `proof_accounts` are
    /// the Proof PDAs in payload order.
    pub fn validate(
        &mut self,
        authority: &Keypair,
        proof_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let accounts = polymer_prover::accounts::Validate {
            authority: authority.pubkey(),
            config: polymer_prover::state::Config::pda().0,
            cache_account: polymer::cache_pda(&authority.pubkey()).0,
            result_account: polymer::result_pda(&authority.pubkey()).0,
            internal: polymer::internal_pda().0,
            polymer_prover_program: polymer::POLYMER_PROVER_ID,
            system_program: anchor_lang::system_program::ID,
            event_authority: event_authority_pda(&polymer_prover::ID).0,
            program: polymer_prover::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(proof_accounts)
        .collect();
        let instruction = Instruction {
            program_id: polymer_prover::ID,
            accounts,
            data: polymer_prover::instruction::Validate {}.data(),
        };
        let transaction = Transaction::new(
            &[authority],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(
                        Self::VALIDATE_COMPUTE_UNIT_LIMIT,
                    ),
                    instruction,
                ],
                Some(&authority.pubkey()),
            ),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// Polymer's `create_accounts` for `authority`; funds the authority first.
    pub fn polymer_create_accounts(&mut self, authority: &Keypair) -> TransactionResult {
        if self.balance(&authority.pubkey()) == 0 {
            self.airdrop(&authority.pubkey(), super::sol_amount(5.0))
                .unwrap();
        }
        let instruction = Instruction {
            program_id: polymer::POLYMER_PROVER_ID,
            accounts: mock_polymer_prover::accounts::CreateAccounts {
                authority: authority.pubkey(),
                cache_account: polymer::cache_pda(&authority.pubkey()).0,
                result_account: polymer::result_pda(&authority.pubkey()).0,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: mock_polymer_prover::instruction::CreateAccounts {}.data(),
        };
        let transaction = Transaction::new(
            &[authority],
            Message::new(&[instruction], Some(&authority.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// Loads `result` into the mock's cache in 800-byte chunks, one
    /// transaction each, exactly as a relayer loads a real proof.
    pub fn polymer_load_result(
        &mut self,
        authority: &Keypair,
        result: &ValidationResultAccount,
    ) -> TransactionResult {
        let body = borsh::to_vec(result).unwrap();
        let mut last = None;
        for chunk in body.chunks(LOAD_PROOF_CHUNK) {
            let instruction = Instruction {
                program_id: polymer::POLYMER_PROVER_ID,
                accounts: mock_polymer_prover::accounts::LoadProof {
                    authority: authority.pubkey(),
                    cache_account: polymer::cache_pda(&authority.pubkey()).0,
                }
                .to_account_metas(None),
                data: mock_polymer_prover::instruction::LoadProof {
                    proof_chunk: chunk.to_vec(),
                }
                .data(),
            };
            let transaction = Transaction::new(
                &[authority],
                Message::new(&[instruction], Some(&authority.pubkey())),
                self.latest_blockhash(),
            );
            last = Some(self.send_transaction(transaction)?);
        }

        Ok(last.expect("result body is never empty"))
    }
}
