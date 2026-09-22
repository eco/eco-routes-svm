use anchor_lang::prelude::borsh;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::{abi_encode_bytes, INTENT_FULFILLED_FROM_SOURCE_SELECTOR};
use polymer_prover::polymer;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
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
