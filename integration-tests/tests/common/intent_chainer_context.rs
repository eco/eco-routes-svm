use anchor_lang::prelude::AccountMeta;
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use derive_more::{Deref, DerefMut};
use eco_svm_std::{Bytes32, CHAIN_ID};
use intent_chainer::state::escrow_authority_pda;
use intent_chainer::types::{Order, Slot, WAD};
use portal::state::{vault_pda, WithdrawnMarker};
use portal::types::{Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult, COMPUTE_UNIT_LIMIT};

/// The SPL `transfer_checked` instruction discriminator.
const TRANSFER_CHECKED_DISCRIMINATOR: u8 = 12;

/// Sentinel the SDK-side segment cutter splits on.
///
/// The production SDK encodes the route with this in every runtime position and
/// splits the blob on it, rather than computing write offsets — which is the
/// whole point of the segments representation, and is why the cutter here works
/// the same way. A hand-computed offset table would make the test agree with the
/// program by construction while both disagreed with reality.
const SENTINEL: u64 = 0xDEAD_BEEF_DEAD_BEEF;

#[derive(Deref, DerefMut)]
pub struct IntentChainer<'a>(&'a mut Context);

impl Context {
    pub fn intent_chainer(&mut self) -> IntentChainer<'_> {
        IntentChainer(self)
    }
}

/// A chained pair ready to drive: intent2's committed order plus the addresses it
/// resolves to for one particular measured amount.
pub struct ChainedIntent {
    pub order: Order,
    pub order_commitment: Bytes32,
    pub escrow_authority: Pubkey,
    pub escrow_ata: Pubkey,
    /// Intent2's typed route for the amount this was resolved at. Only meaningful
    /// for a Solana destination, where the route bytes *are* Borsh.
    pub route: Route,
    pub reward: Reward,
    pub intent_hash: Bytes32,
    pub vault: Pubkey,
    pub vault_ata: Pubkey,
    pub amount_out: u128,
}

impl IntentChainer<'_> {
    /// Builds a same-chain (SVM → SVM) order whose route is a Borsh
    /// [`portal::types::Route`] carrying the destination amount **twice** — once
    /// as the token leg and once inside an SPL `transfer_checked` call — which is
    /// the shape `DepositAddress_USDCTransfer_Solana` emits and the shape the
    /// segments mechanism exists to handle.
    ///
    /// `recipient_ata` receives the tokens on the destination side.
    pub fn svm_order(
        &mut self,
        base_mint: Pubkey,
        recipient_ata: Pubkey,
        prover: Pubkey,
        scale: u128,
        min_amount_in: u64,
    ) -> Order {
        let creator = self.creator.pubkey();
        let deadline = self.now() + 3600;
        let token_program = self.token_program;
        let (segments, slots) = self.cut_svm_route(base_mint, recipient_ata, token_program);

        Order {
            base_mint,
            destination: CHAIN_ID,
            segments,
            slots,
            reward: Reward {
                deadline,
                creator,
                prover,
                native_amount: 0,
                // Authored at zero: the program requires it, which is what makes
                // the commitment preimage canonical.
                tokens: vec![TokenAmount {
                    token: base_mint,
                    amount: 0,
                }],
            },
            scale,
            min_amount_in,
        }
    }

    /// Encodes intent2's route with [`SENTINEL`] in both amount positions and
    /// splits on it, exactly as the SDK does.
    fn cut_svm_route(
        &mut self,
        base_mint: Pubkey,
        recipient_ata: Pubkey,
        token_program: Pubkey,
    ) -> (Vec<Vec<u8>>, Vec<Slot>) {
        let route = self.svm_route(base_mint, recipient_ata, token_program, SENTINEL);
        let blob = borsh::to_vec(&route).unwrap();
        let needle = SENTINEL.to_le_bytes();

        let segments: Vec<Vec<u8>> = blob
            .windows(needle.len())
            .enumerate()
            .filter(|(_, window)| *window == needle)
            .map(|(index, _)| index)
            .fold((Vec::new(), 0usize), |(mut segments, cursor), position| {
                // Overlapping matches cannot happen for a sentinel this sparse,
                // but a match that starts inside the previous one would be a
                // cutter bug, so skip rather than emit a negative-length slice.
                if position < cursor {
                    return (segments, cursor);
                }
                segments.push(blob[cursor..position].to_vec());
                (segments, position + needle.len())
            })
            .0
            .into_iter()
            .chain(std::iter::once({
                let last = blob
                    .windows(needle.len())
                    .enumerate()
                    .filter(|(_, window)| *window == needle)
                    .map(|(index, _)| index)
                    .next_back()
                    .expect("sentinel must appear in the route");
                blob[last + needle.len()..].to_vec()
            }))
            .collect();

        let slots = (0..segments.len() - 1)
            .map(|_| Slot {
                width: 8,
                little_endian: true,
            })
            .collect();

        (segments, slots)
    }

    /// Intent2's route for a concrete amount — the reference the segments must
    /// reproduce byte for byte.
    pub fn svm_route(
        &mut self,
        base_mint: Pubkey,
        recipient_ata: Pubkey,
        token_program: Pubkey,
        amount: u64,
    ) -> Route {
        let executor_ata = get_associated_token_address_with_program_id(
            &portal::state::executor_pda().0,
            &base_mint,
            &token_program,
        );

        let mut data = vec![TRANSFER_CHECKED_DISCRIMINATOR];
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(6); // decimals, matching `set_mint_account`

        let calldata = Calldata {
            data,
            account_count: 4,
        };
        let accounts = vec![
            AccountMeta::new(executor_ata, false),
            AccountMeta::new_readonly(base_mint, false),
            AccountMeta::new(recipient_ata, false),
            AccountMeta::new_readonly(portal::state::executor_pda().0, false),
        ];

        Route {
            deadline: self.now() + 1800,
            salt: [7u8; 32].into(),
            portal: portal::ID.to_bytes().into(),
            native_amount: 0,
            tokens: vec![TokenAmount {
                token: base_mint,
                amount,
            }],
            calls: vec![Call {
                target: token_program.to_bytes().into(),
                data: borsh::to_vec(&CalldataWithAccounts::new(calldata, accounts).unwrap())
                    .unwrap(),
            }],
        }
    }

    /// Resolves an order against a measured amount the same way the program will,
    /// producing every address the `chain` transaction has to name up front.
    ///
    /// This models exactly what an off-chain caller must do: read the escrow
    /// balance, derive intent2's vault from it, and submit. If the balance moves
    /// between the read and the transaction, the program rejects the mismatch.
    pub fn resolve(
        &mut self,
        order: &Order,
        amount_in: u64,
        recipient_ata: Pubkey,
    ) -> ChainedIntent {
        let token_program = self.token_program;
        let order_commitment = order.hash();
        let (escrow_authority, _) = escrow_authority_pda(&order_commitment);
        let escrow_ata = get_associated_token_address_with_program_id(
            &escrow_authority,
            &order.base_mint,
            &token_program,
        );

        let amount_out = intent_chainer::types::scale_amount(amount_in, order.scale).unwrap();
        let route_bytes = order.build_route(amount_out).unwrap();
        let route: Route = borsh::from_slice(&route_bytes).expect("SVM route must deserialize");
        let _ = recipient_ata;

        let mut reward = order.reward.clone();
        reward.tokens[0].amount = amount_in;

        let route_hash = keccak(&route_bytes);
        let intent_hash =
            portal::types::intent_hash(order.destination, &route_hash, &reward.hash());
        let (vault, _) = vault_pda(&intent_hash);
        let vault_ata =
            get_associated_token_address_with_program_id(&vault, &order.base_mint, &token_program);

        ChainedIntent {
            order: order.clone(),
            order_commitment,
            escrow_authority,
            escrow_ata,
            route,
            reward,
            intent_hash,
            vault,
            vault_ata,
            amount_out,
        }
    }

    /// Creates the escrow ATA and mints `amount` into it, standing in for
    /// intent1's swap output landing there.
    pub fn seed_escrow(&mut self, chained: &ChainedIntent, amount: u64) {
        let mint = chained.order.base_mint;
        let authority = chained.escrow_authority;

        self.airdrop_token_ata(&mint, &authority, amount);
    }

    /// Drives `intent_chainer::chain`.
    pub fn chain(&mut self, chained: &ChainedIntent, publish: bool) -> TransactionResult {
        self.chain_with_accounts(
            chained,
            publish,
            chained.escrow_authority,
            chained.escrow_ata,
            chained.vault,
            chained.vault_ata,
            WithdrawnMarker::pda(&chained.intent_hash).0,
            chained.order.base_mint,
        )
    }

    /// Drives `chain` with every address the handler validates left open, so a
    /// test can substitute a single wrong account and pin which check catches it.
    #[allow(clippy::too_many_arguments)]
    pub fn chain_with_accounts(
        &mut self,
        chained: &ChainedIntent,
        publish: bool,
        escrow_authority: Pubkey,
        escrow_ata: Pubkey,
        vault: Pubkey,
        vault_ata: Pubkey,
        withdrawn_marker: Pubkey,
        base_mint: Pubkey,
    ) -> TransactionResult {
        let args = intent_chainer::instructions::ChainArgs {
            order: chained.order.clone(),
            publish,
        };
        let accounts = intent_chainer::accounts::Chain {
            payer: self.payer.pubkey(),
            escrow_authority,
            escrow_ata,
            base_mint,
            vault,
            vault_ata,
            withdrawn_marker,
            portal_program: portal::ID,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        };
        let instruction = Instruction {
            program_id: intent_chainer::ID,
            accounts: accounts.to_account_metas(None),
            data: intent_chainer::instruction::Chain { args }.data(),
        };

        let payer = self.payer.insecure_clone();
        let transaction = Transaction::new(
            &[&payer],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&payer.pubkey()),
            ),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// A route call on intent1 that forwards the executor's balance of `mint`
    /// into the chainer's escrow — the stand-in for "the swap named the escrow as
    /// its recipient".
    ///
    /// Returns both encodings portal needs: the minimal `Calldata` form that
    /// `fulfill` is given, and the full `CalldataWithAccounts` form the intent
    /// hash is computed over. Portal reconstructs the second from the first plus
    /// the transaction's accounts, so a test that used one for both would fail
    /// with a bare `BorshIoError`.
    pub fn deliver_to_escrow_call(
        &mut self,
        mint: Pubkey,
        escrow_ata: Pubkey,
        amount: u64,
    ) -> (Call, Call, Vec<AccountMeta>) {
        let token_program = self.token_program;
        let executor = portal::state::executor_pda().0;
        let executor_ata =
            get_associated_token_address_with_program_id(&executor, &mint, &token_program);

        let mut data = vec![TRANSFER_CHECKED_DISCRIMINATOR];
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(6);

        let accounts = vec![
            AccountMeta::new(executor_ata, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new(escrow_ata, false),
            AccountMeta::new_readonly(executor, false),
        ];
        let calldata = Calldata {
            data,
            account_count: 4,
        };

        let minimal = Call {
            target: token_program.to_bytes().into(),
            data: borsh::to_vec(&calldata).unwrap(),
        };
        let with_accounts = Call {
            target: token_program.to_bytes().into(),
            data: borsh::to_vec(&CalldataWithAccounts::new(calldata, accounts.clone()).unwrap())
                .unwrap(),
        };

        (minimal, with_accounts, accounts)
    }
}

pub fn keccak(bytes: &[u8]) -> Bytes32 {
    use tiny_keccak::{Hasher, Keccak};

    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(bytes);
    hasher.finalize(&mut hash);

    hash.into()
}

/// Convenience for the common same-unit, no-spread lane.
pub const IDENTITY_SCALE: u128 = WAD;
