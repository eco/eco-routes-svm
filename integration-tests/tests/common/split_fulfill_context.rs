use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::{AnchorDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::Proof;
use eco_svm_std::{event_authority_pda, Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::ProveAndWithdrawArgs;
use flash_fulfiller::state::prove_authority_pda;
use portal::instructions::FulfillArgs;
use portal::state::{executor_pda, proof_closer_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{intent_hash, Calldata, Reward, Route};
use solana_address_lookup_table_interface::state::{AddressLookupTable, LookupTableMeta};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::clock::Clock;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;

use super::Context;

impl Context {
    /// Model a pre-existing lookup table so the signed split transaction really
    /// fits the packet limit. LiteSVM also exercises sysvar lookup resolution.
    pub fn build_split_fulfill_transaction(
        &mut self,
        instructions: &[Instruction],
    ) -> VersionedTransaction {
        let all: Vec<_> = [
            ComputeBudgetInstruction::request_heap_frame(256 * 1024),
            ComputeBudgetInstruction::set_compute_unit_limit(1_000_000),
        ]
        .into_iter()
        .chain(instructions.iter().cloned())
        .collect();
        let table = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            addresses: Message::new(&all, Some(&self.payer.pubkey())).account_keys,
        };
        let data = AddressLookupTable {
            meta: LookupTableMeta::default(),
            addresses: std::borrow::Cow::Borrowed(&table.addresses),
        }
        .serialize_for_tests()
        .unwrap();
        let lamports = self.get_sysvar::<Rent>().minimum_balance(data.len());
        self.set_account(
            table.key,
            solana_sdk::account::Account {
                lamports,
                data,
                owner: solana_address_lookup_table_interface::program::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        let slot = self.get_sysvar::<Clock>().slot;
        self.warp_to_slot(slot.max(1));
        let message = v0::Message::try_compile(
            &self.payer.pubkey(),
            &all,
            &[table],
            self.latest_blockhash(),
        )
        .unwrap();
        let tx = VersionedTransaction::try_new(
            VersionedMessage::V0(message),
            &[&self.payer, &self.solver],
        )
        .unwrap();
        // Two signatures: their short-vec length is exactly one byte.
        assert_eq!(tx.signatures.len(), 2);
        let wire_size = 1 + 64 * tx.signatures.len() + tx.message.serialize().len();
        assert!(wire_size <= 1232, "split transaction is {wire_size} bytes");
        tx
    }

    pub fn build_prove_and_withdraw_instruction(
        &self,
        route_hash: Bytes32,
        reward: &Reward,
    ) -> Instruction {
        let hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
        let vault = vault_pda(&hash).0;
        let mut accounts = flash_fulfiller::accounts::ProveAndWithdraw {
            payer: self.payer.pubkey(),
            solver: self.solver.pubkey(),
            instructions: solana_instructions_sysvar::ID,
            proof: Proof::pda(&hash, &reward.prover).0,
            intent_vault: vault,
            withdrawn_marker: WithdrawnMarker::pda(&hash).0,
            proof_closer: proof_closer_pda(&reward.prover).0,
            portal_program: portal::ID,
            local_prover_program: reward.prover,
            prove_authority: prove_authority_pda(&reward.prover).0,
            local_prover_event_authority: event_authority_pda(&reward.prover).0,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None);
        accounts.extend(
            reward
                .token_amounts()
                .unwrap()
                .keys()
                .flat_map(|mint| self.token_transfer_metas(vault, self.solver.pubkey(), *mint)),
        );
        Instruction {
            program_id: flash_fulfiller::ID,
            accounts,
            data: flash_fulfiller::instruction::ProveAndWithdraw {
                args: ProveAndWithdrawArgs {
                    route_hash,
                    reward: reward.clone(),
                },
            }
            .data(),
        }
    }

    /// `route` contains the committed CalldataWithAccounts. Send only Calldata
    /// to portal, which reconstructs the account metadata before hashing.
    pub fn build_paired_fulfill_instruction(
        &self,
        route: &Route,
        reward: &Reward,
        call_accounts: Vec<AccountMeta>,
    ) -> Instruction {
        let hash = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
        let mut accounts = portal::accounts::Fulfill {
            payer: self.payer.pubkey(),
            solver: self.solver.pubkey(),
            executor: executor_pda().0,
            fulfill_marker: FulfillMarker::pda(&hash).0,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None);
        // The native funding CPI writes the executor's lamports. Portal's
        // generated fixed accounts do not mark it writable for the client.
        accounts[2].is_writable = route.native_amount > 0;
        accounts.extend(route.tokens.iter().flat_map(|token| {
            self.token_transfer_metas(self.solver.pubkey(), executor_pda().0, token.token)
        }));
        accounts.extend(call_accounts);
        let mut route = route.clone();
        for call in &mut route.calls {
            let calldata = Calldata::deserialize(&mut call.data.as_slice()).unwrap();
            call.data = borsh::to_vec(&calldata).unwrap();
        }
        Instruction {
            program_id: portal::ID,
            accounts,
            data: portal::instruction::Fulfill {
                args: FulfillArgs {
                    intent_hash: hash,
                    route,
                    reward_hash: reward.hash(),
                    claimant: self.solver.pubkey().to_bytes().into(),
                },
            }
            .data(),
        }
    }

    pub fn token_transfer_metas(&self, from: Pubkey, to: Pubkey, mint: Pubkey) -> [AccountMeta; 3] {
        [
            AccountMeta::new(
                get_associated_token_address_with_program_id(&from, &mint, &self.token_program),
                false,
            ),
            AccountMeta::new(
                get_associated_token_address_with_program_id(&to, &mint, &self.token_program),
                false,
            ),
            AccountMeta::new_readonly(mint, false),
        ]
    }

    /// Keep every other LiteSVM feature setting, but enforce the pre-SIMD-0268
    /// depth limit (5). No custom ComputeBudget override can mask this check.
    pub fn with_five_frame_limit(mut self) -> Self {
        let gate = solana_sdk::pubkey!("6TkHkRmP7JZy1fdM6fg5uXn76wChQBWGokHBJzrLB3mj");
        let mut features = litesvm::LiteSVM::mainnet_feature_set();
        features.deactivate(&gate);
        assert!(!features.is_active(&gate));
        self.svm = self.svm.with_feature_set(features);
        self
    }
}
