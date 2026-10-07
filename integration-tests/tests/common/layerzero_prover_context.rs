use std::iter;

use anchor_lang::{system_program, AnchorDeserialize, InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::{GetProofArgs, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::{Bytes32, CHAIN_ID};
use layerzero_prover::instructions::{
    required_alt_addresses, InitArgs, PathConfig, QuoteMessageArgs, ADDRESS_LOOKUP_TABLE_PROGRAM_ID,
};
use layerzero_prover::layerzero::{
    self, AccountMetaRef, AddressLocator, ExecutorConfig, LzReceiveParams, MessagingFee, UlnConfig,
    DEVNET_SOLANA_EID, ENDPOINT_ID, NIL_DVN_COUNT, ULN_ID,
};
use layerzero_prover::state::{pda_payer_pda, LzReceiveTypesAccount, Peer, PendingSend, Store};
use portal::state::FulfillMarker;
use portal::types::Reward;
use solana_address_lookup_table_interface::state::{AddressLookupTable, LookupTableMeta};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::account::Account;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{
    cleanup_recipient, program_data_address, sol_amount, Context, TransactionResult,
};

const LAYERZERO_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/layerzero_prover.so");

pub const BASE_EID: u32 = 30184;
pub const BASE_CHAIN_ID: u64 = 8453;
pub const OP_EID: u32 = 30111;
pub const OP_CHAIN_ID: u64 = 10;
pub const COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
/// Receives the mock endpoint's flat fee in `send_message` tests.
pub const TREASURY: Pubkey = Pubkey::new_from_array([0x7e; 32]);

/// An EVM peer whose address is a left-padded 20-byte address of `byte`s.
pub fn evm_peer(eid: u32, chain_id: u64, byte: u8) -> Peer {
    let mut address = [0u8; 32];
    address[12..].fill(byte);

    Peer {
        eid,
        address: address.into(),
        chain_id,
    }
}

pub fn peers() -> Vec<Peer> {
    vec![
        evm_peer(BASE_EID, BASE_CHAIN_ID, 0xba),
        evm_peer(OP_EID, OP_CHAIN_ID, 0x0b),
    ]
}

#[derive(Deref, DerefMut)]
pub struct LayerZeroProver<'a>(&'a mut Context);

impl Context {
    pub fn layerzero_prover(&mut self) -> LayerZeroProver<'_> {
        LayerZeroProver(self)
    }
}

impl LayerZeroProver<'_> {
    /// Adds the program with `authority` as upgrade authority, initializes the
    /// mock endpoint's settings account and funds `pda_payer`.
    pub fn install(&mut self, authority: Pubkey) {
        self.add_program(layerzero_prover::ID, LAYERZERO_PROVER_BIN)
            .unwrap();
        self.set_upgrade_authority(&layerzero_prover::ID, Some(authority));

        let payer = self.payer.pubkey();
        let instruction = Instruction {
            program_id: layerzero::ENDPOINT_ID,
            accounts: mock_layerzero_endpoint::accounts::MockInit {
                payer,
                endpoint: layerzero::endpoint_settings_pda().0,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mock_layerzero_endpoint::instruction::MockInit {
                eid: DEVNET_SOLANA_EID,
            }
            .data(),
        };
        self.send(vec![instruction], &[]).unwrap();

        self.airdrop(&pda_payer_pda().0, sol_amount(10.0)).unwrap();
    }

    pub fn program_data(&self) -> Pubkey {
        program_data_address(&layerzero_prover::ID)
    }

    /// Simulates `solana program set-upgrade-authority --final`.
    pub fn finalize(&mut self) {
        self.set_upgrade_authority(&layerzero_prover::ID, None);
    }

    /// Sends `instructions` after a 1.4M CU limit, paid by the context payer
    /// plus any extra `signers`.
    pub fn send(
        &mut self,
        instructions: Vec<Instruction>,
        signers: &[&Keypair],
    ) -> TransactionResult {
        let payer = self.payer.insecure_clone();

        self.send_as(&payer, instructions, signers)
    }

    /// [`Self::send`] with `payer` paying the fee instead of the context payer.
    pub fn send_as(
        &mut self,
        payer: &Keypair,
        instructions: Vec<Instruction>,
        signers: &[&Keypair],
    ) -> TransactionResult {
        let instructions: Vec<_> = iter::once(ComputeBudgetInstruction::set_compute_unit_limit(
            COMPUTE_UNIT_LIMIT,
        ))
        .chain(instructions)
        .collect();
        let signers: Vec<&Keypair> = iter::once(payer).chain(signers.iter().copied()).collect();
        let transaction = Transaction::new(
            &signers,
            Message::new(&instructions, Some(&payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// A funded, native-only intent on this (source) chain naming `prover`.
    pub fn funded_native_intent(
        &mut self,
        destination: u64,
        prover: Pubkey,
    ) -> (Reward, Bytes32, Bytes32) {
        let (_, _, mut reward) = self.rand_intent();
        reward.prover = prover;
        reward.tokens.clear();
        let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
        let hash = portal::types::intent_hash(destination, &route_hash, &reward.hash());
        let vault = portal::state::vault_pda(&hash).0;
        let funder = self.funder.pubkey();
        self.airdrop(&funder, reward.native_amount).unwrap();
        self.portal()
            .fund_intent(
                destination,
                reward.clone(),
                vault,
                route_hash,
                false,
                Vec::<AccountMeta>::new(),
            )
            .unwrap();

        (reward, route_hash, hash)
    }
}

/// A fully pinned path config: two required DVNs (sorted, as ULN302 requires),
/// explicit confirmations and executor.
pub fn path_config() -> PathConfig {
    let mut dvns = vec![
        Pubkey::new_from_array([1; 32]),
        Pubkey::new_from_array([2; 32]),
    ];
    dvns.sort();
    let uln = UlnConfig {
        confirmations: 15,
        required_dvn_count: 2,
        optional_dvn_count: NIL_DVN_COUNT,
        optional_dvn_threshold: 0,
        required_dvns: dvns,
        optional_dvns: vec![],
    };

    PathConfig {
        send_uln: uln.clone(),
        receive_uln: uln,
        executor: ExecutorConfig {
            max_message_size: 10_000,
            executor: Pubkey::new_from_array([3; 32]),
        },
    }
}

impl LayerZeroProver<'_> {
    pub fn init(&mut self, authority: &Keypair, peers: Vec<Peer>) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::Init {
                payer: self.payer.pubkey(),
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                lz_receive_types: LzReceiveTypesAccount::pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                endpoint_event_authority: layerzero::endpoint_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::Init {
                args: InitArgs { peers },
            }
            .data(),
        };

        self.send(vec![instruction], &[authority])
    }

    pub fn init_path(&mut self, authority: &Keypair, peer: &Peer) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::InitPath {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                pda_payer: pda_payer_pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                nonce: layerzero::nonce_pda(&store, peer.eid, &peer.address).0,
                pending_nonce: layerzero::pending_nonce_pda(&store, peer.eid, &peer.address).0,
                send_library_config: layerzero::send_library_config_pda(&store, peer.eid).0,
                receive_library_config: layerzero::receive_library_config_pda(&store, peer.eid).0,
                message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0)
                    .0,
                endpoint_event_authority: layerzero::endpoint_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::InitPath { eid: peer.eid }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    pub fn set_path_config(
        &mut self,
        authority: &Keypair,
        eid: u32,
        config: PathConfig,
    ) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::SetPathConfig {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                pda_payer: pda_payer_pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0)
                    .0,
                uln_settings: layerzero::uln_settings_pda().0,
                uln_program: ULN_ID,
                uln_send_config: layerzero::uln_send_config_pda(eid, &store).0,
                uln_receive_config: layerzero::uln_receive_config_pda(eid, &store).0,
                uln_default_send_config: layerzero::uln_default_send_config_pda(eid).0,
                uln_default_receive_config: layerzero::uln_default_receive_config_pda(eid).0,
                uln_event_authority: layerzero::uln_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::SetPathConfig { eid, config }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    /// Stages a frozen, active lookup table holding `required_alt_addresses`
    /// for the `Store` that `init` created: the table `set_alt` accepts.
    pub fn create_alt(&mut self) -> Pubkey {
        let store = self.account::<Store>(&Store::pda().0).unwrap();
        self.create_alt_with(None, u64::MAX, required_alt_addresses(&store))
    }

    /// Stages a lookup-table account with the given meta and addresses, in
    /// the program's bincode `ProgramState::LookupTable` layout.
    pub fn create_alt_with(
        &mut self,
        authority: Option<Pubkey>,
        deactivation_slot: u64,
        addresses: Vec<Pubkey>,
    ) -> Pubkey {
        let alt = Pubkey::new_unique();
        let data = AddressLookupTable {
            meta: LookupTableMeta {
                deactivation_slot,
                authority,
                ..LookupTableMeta::default()
            },
            addresses: addresses.into(),
        }
        .serialize_for_tests()
        .unwrap();
        self.set_account(
            alt,
            Account {
                lamports: 1_000_000_000,
                data,
                owner: ADDRESS_LOOKUP_TABLE_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        alt
    }

    pub fn set_alt(&mut self, authority: &Keypair, alt: Pubkey) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::SetAlt {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store: Store::pda().0,
                alt,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::SetAlt {}.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    /// install + init + every peer's path and config + ALT.
    pub fn setup(&mut self) -> Keypair {
        let authority = Keypair::new();
        self.install(authority.pubkey());
        self.init(&authority, peers()).unwrap();
        peers().iter().for_each(|peer| {
            self.init_path(&authority, peer).unwrap();
            self.set_path_config(&authority, peer.eid, path_config())
                .unwrap();
        });
        let alt = self.create_alt();
        self.set_alt(&authority, alt).unwrap();

        authority
    }
}

impl LayerZeroProver<'_> {
    /// The bytes portal hands `prove` for these fulfilled intents.
    pub fn payload(&self, intent_hashes: &[Bytes32]) -> Vec<u8> {
        let pairs = intent_hashes
            .iter()
            .map(|hash| {
                let marker = self
                    .account::<FulfillMarker>(&FulfillMarker::pda(hash).0)
                    .unwrap();
                IntentHashClaimant::new(*hash, marker.claimant)
            })
            .collect();

        ProofData::new(CHAIN_ID, pairs).to_bytes()
    }

    pub fn pending_send_for(
        &self,
        dst_eid: u32,
        receiver: &Bytes32,
        intent_hashes: &[Bytes32],
    ) -> Pubkey {
        PendingSend::pda(dst_eid, receiver, &self.payload(intent_hashes)).0
    }

    /// `portal::prove` targeting this prover. `data` is normally the 32-byte
    /// EVM receiver; tests pass malformed data on purpose.
    pub fn prove(
        &mut self,
        intent_hashes: Vec<Bytes32>,
        dst_eid: u64,
        data: Vec<u8>,
    ) -> TransactionResult {
        let receiver: Bytes32 = <[u8; 32]>::try_from(data.as_slice())
            .unwrap_or([0; 32])
            .into();
        let pending = self.pending_send_for(dst_eid as u32, &receiver, &intent_hashes);
        let instruction =
            build_prove_instruction(self.payer.pubkey(), intent_hashes, dst_eid, data, pending);

        self.send(vec![instruction], &[])
    }
}

/// `portal::prove` targeting this prover, with the `PendingSend` it commits to.
/// Free function so the batch-size tests can build it without a context.
pub fn build_prove_instruction(
    payer: Pubkey,
    intent_hashes: Vec<Bytes32>,
    dst_eid: u64,
    data: Vec<u8>,
    pending: Pubkey,
) -> Instruction {
    let fulfill_markers = intent_hashes.iter().map(|hash| FulfillMarker::pda(hash).0);
    let accounts = portal::accounts::Prove {
        prover: layerzero_prover::ID,
        dispatcher: portal::state::dispatcher_pda(&layerzero_prover::ID).0,
    }
    .to_account_metas(None)
    .into_iter()
    .chain(fulfill_markers.map(|marker| AccountMeta::new_readonly(marker, false)))
    .chain([
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(Store::pda().0, false),
        AccountMeta::new(pending, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ])
    .collect();

    Instruction {
        program_id: portal::ID,
        accounts,
        data: portal::instruction::Prove {
            args: portal::instructions::ProveArgs {
                prover: layerzero_prover::ID,
                source_chain_domain_id: dst_eid,
                intent_hashes,
                data,
            },
        }
        .data(),
    }
}

/// The read-only head `send` and `quote` share: ULN302, the path's send
/// libraries, the library record and the endpoint settings.
fn library_head(store: Pubkey, dst_eid: u32) -> [AccountMeta; 5] {
    [
        AccountMeta::new_readonly(ULN_ID, false),
        AccountMeta::new_readonly(layerzero::send_library_config_pda(&store, dst_eid).0, false),
        AccountMeta::new_readonly(layerzero::default_send_library_config_pda(dst_eid).0, false),
        AccountMeta::new_readonly(
            layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
            false,
        ),
        AccountMeta::new_readonly(layerzero::endpoint_settings_pda().0, false),
    ]
}

/// Endpoint `send` accounts after `[program, sender]`, then the ULN302 send
/// tail. Worker (executor/DVN) accounts are omitted: the mock ignores them.
pub fn send_accounts(
    store: Pubkey,
    payer: Pubkey,
    dst_eid: u32,
    receiver: &Bytes32,
) -> Vec<AccountMeta> {
    library_head(store, dst_eid)
        .into_iter()
        .chain([
            AccountMeta::new(layerzero::nonce_pda(&store, dst_eid, receiver).0, false),
            AccountMeta::new_readonly(layerzero::endpoint_event_authority().0, false),
            AccountMeta::new_readonly(ENDPOINT_ID, false),
            AccountMeta::new_readonly(layerzero::uln_settings_pda().0, false),
            AccountMeta::new_readonly(layerzero::uln_send_config_pda(dst_eid, &store).0, false),
            AccountMeta::new_readonly(layerzero::uln_default_send_config_pda(dst_eid).0, false),
            AccountMeta::new(payer, true),
            AccountMeta::new(TREASURY, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(layerzero::uln_event_authority().0, false),
            AccountMeta::new_readonly(ULN_ID, false),
        ])
        .collect()
}

/// Endpoint `quote` accounts plus the ULN302 quote head, all read-only.
pub fn quote_accounts(store: Pubkey, dst_eid: u32, receiver: &Bytes32) -> Vec<AccountMeta> {
    library_head(store, dst_eid)
        .into_iter()
        .chain([
            AccountMeta::new_readonly(layerzero::nonce_pda(&store, dst_eid, receiver).0, false),
            AccountMeta::new_readonly(layerzero::uln_settings_pda().0, false),
            AccountMeta::new_readonly(layerzero::uln_send_config_pda(dst_eid, &store).0, false),
            AccountMeta::new_readonly(layerzero::uln_default_send_config_pda(dst_eid).0, false),
        ])
        .collect()
}

/// Free function (no context needed) so the batch-size tests can build it too.
pub fn build_send_message_instruction(
    pending_send: Pubkey,
    rent_payer: Pubkey,
    fee_payer: Pubkey,
    dst_eid: u32,
    receiver: &Bytes32,
    max_native_fee: u64,
) -> Instruction {
    let store = Store::pda().0;
    let accounts = layerzero_prover::accounts::SendMessage {
        payer: fee_payer,
        store,
        pending_send,
        rent_payer,
        endpoint_program: ENDPOINT_ID,
    }
    .to_account_metas(None)
    .into_iter()
    .chain(send_accounts(store, fee_payer, dst_eid, receiver))
    .collect();

    Instruction {
        program_id: layerzero_prover::ID,
        accounts,
        data: layerzero_prover::instruction::SendMessage { max_native_fee }.data(),
    }
}

impl LayerZeroProver<'_> {
    pub fn send_message(
        &mut self,
        pending_send: Pubkey,
        rent_payer: Pubkey,
        fee_payer: &Keypair,
        dst_eid: u32,
        receiver: &Bytes32,
        max_native_fee: u64,
    ) -> TransactionResult {
        let instruction = build_send_message_instruction(
            pending_send,
            rent_payer,
            fee_payer.pubkey(),
            dst_eid,
            receiver,
            max_native_fee,
        );

        self.send_as(fee_payer, vec![instruction], &[])
    }

    pub fn quote_message(
        &mut self,
        args: QuoteMessageArgs,
    ) -> Result<MessagingFee, Box<litesvm::types::FailedTransactionMetadata>> {
        let store = Store::pda().0;
        let accounts = layerzero_prover::accounts::QuoteMessage {
            store,
            endpoint_program: ENDPOINT_ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(quote_accounts(store, args.dst_eid, &args.receiver))
        .collect();
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts,
            data: layerzero_prover::instruction::QuoteMessage { args }.data(),
        };
        let result = self.send(vec![instruction], &[])?;

        Ok(MessagingFee::try_from_slice(&result.return_data.data).unwrap())
    }
}

/// A delivery of `proof_data` from `peer` at `nonce` (guid derived from nonce).
pub fn receive_params(peer: &Peer, nonce: u64, proof_data: ProofData) -> LzReceiveParams {
    LzReceiveParams {
        src_eid: peer.eid,
        sender: peer.address.into(),
        nonce,
        guid: [nonce as u8; 32],
        message: proof_data.to_bytes(),
        extra_data: vec![],
    }
}

impl LayerZeroProver<'_> {
    pub fn lz_receive_types_info(&mut self, params: LzReceiveParams) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::LzReceiveTypesInfo {
                store: Store::pda().0,
                lz_receive_types: LzReceiveTypesAccount::pda().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::LzReceiveTypesInfo { params }.data(),
        };
        self.send(vec![instruction], &[])
    }

    pub fn lz_receive_types_v2(&mut self, params: LzReceiveParams) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::LzReceiveTypesV2 {
                store: Store::pda().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::LzReceiveTypesV2 { params }.data(),
        };
        self.send(vec![instruction], &[])
    }
}

/// `lz_receive` as the executor would build it from `accounts` (all
/// `AddressLocator::Address`). Free function so the batch-size tests can use it.
pub fn build_lz_receive_instruction(
    params: &LzReceiveParams,
    accounts: Vec<AccountMetaRef>,
) -> Instruction {
    let accounts = accounts
        .into_iter()
        .map(|meta| match meta.pubkey {
            AddressLocator::Address(pubkey) => AccountMeta {
                pubkey,
                is_signer: false,
                is_writable: meta.is_writable,
            },
            other => panic!("unexpected locator {other:?}"),
        })
        .collect();

    Instruction {
        program_id: layerzero_prover::ID,
        accounts,
        data: layerzero_prover::instruction::LzReceive {
            params: params.clone(),
        }
        .data(),
    }
}

/// The hash the endpoint stores for a verified message: `keccak(guid || message)`.
pub fn payload_hash(params: &LzReceiveParams) -> [u8; 32] {
    let mut hasher = tiny_keccak::Keccak::v256();
    tiny_keccak::Hasher::update(&mut hasher, &params.guid);
    tiny_keccak::Hasher::update(&mut hasher, &params.message);
    let mut hash = [0u8; 32];
    tiny_keccak::Hasher::finalize(hasher, &mut hash);

    hash
}

impl LayerZeroProver<'_> {
    /// Stands in for DVN verification: writes the PayloadHash and advances the
    /// path's inbound nonce on the mock endpoint.
    pub fn verify(&mut self, params: &LzReceiveParams) -> TransactionResult {
        let payload_hash = payload_hash(params);
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: ENDPOINT_ID,
            accounts: mock_layerzero_endpoint::accounts::MockVerify {
                payer: self.payer.pubkey(),
                nonce: layerzero::nonce_pda(&store, params.src_eid, &params.sender).0,
                payload_hash: layerzero::payload_hash_pda(
                    &store,
                    params.src_eid,
                    &params.sender,
                    params.nonce,
                )
                .0,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mock_layerzero_endpoint::instruction::MockVerify {
                params: mock_layerzero_endpoint::MockVerifyParams {
                    receiver: store,
                    src_eid: params.src_eid,
                    sender: params.sender,
                    nonce: params.nonce,
                    payload_hash,
                },
            }
            .data(),
        };

        self.send(vec![instruction], &[])
    }

    pub fn lz_receive_instruction(
        &self,
        params: &LzReceiveParams,
        accounts: Vec<AccountMetaRef>,
    ) -> Instruction {
        build_lz_receive_instruction(params, accounts)
    }

    pub fn lz_receive(&mut self, params: &LzReceiveParams) -> TransactionResult {
        self.lz_receive_with(params, |_| {})
    }

    /// Verifies `params`, then sends `lz_receive` with the canonical account
    /// list after `mutate` tampered with it.
    pub fn deliver_with(
        &mut self,
        params: &LzReceiveParams,
        mutate: impl FnOnce(&mut Vec<AccountMetaRef>),
    ) -> TransactionResult {
        self.verify(params)?;
        self.lz_receive_with(params, mutate)
    }

    fn lz_receive_with(
        &mut self,
        params: &LzReceiveParams,
        mutate: impl FnOnce(&mut Vec<AccountMetaRef>),
    ) -> TransactionResult {
        let proof_data = ProofData::from_bytes(&params.message).unwrap();
        let mut accounts = layerzero_prover::instructions::lz_receive_accounts(params, &proof_data);
        mutate(&mut accounts);
        let instruction = self.lz_receive_instruction(params, accounts);
        self.send(vec![instruction], &[])
    }

    pub fn deliver(&mut self, params: &LzReceiveParams) -> TransactionResult {
        self.verify(params)?;
        self.lz_receive(params)
    }

    /// Creates an endpoint Nonce for a path our `init_path` never opened, to
    /// prove our own peer check holds even if the endpoint had one.
    pub fn force_nonce(&mut self, src_eid: u32, sender: [u8; 32]) {
        let store = Store::pda().0;
        let (address, bump) = layerzero::nonce_pda(&store, src_eid, &sender);
        self.set_anchor_account(
            address,
            ENDPOINT_ID,
            &mock_layerzero_endpoint::Nonce {
                bump,
                outbound_nonce: 0,
                inbound_nonce: 0,
            },
        );
    }
}

/// Portal `close_proof` cleanup tail for our proof of `intent_hash`:
/// `[proof, pda_payer]`, the rent going back to the reserve that paid it.
pub fn cleanup_tail(intent_hash: &Bytes32) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(Proof::pda(intent_hash, &layerzero_prover::ID).0, false),
        cleanup_recipient(&layerzero_prover::ID, Pubkey::default()),
    ]
}

impl LayerZeroProver<'_> {
    /// Calls our `get_proof` directly (query tail `[proof]`) and decodes the
    /// `Option<Proof>` it returns.
    pub fn get_proof(&mut self, intent_hash: Bytes32, destination: u64) -> Option<Proof> {
        let result = self
            .send_instruction(Instruction {
                program_id: layerzero_prover::ID,
                accounts: layerzero_prover::accounts::GetProof {
                    proof: Proof::pda(&intent_hash, &layerzero_prover::ID).0,
                }
                .to_account_metas(None),
                data: layerzero_prover::instruction::GetProof {
                    args: GetProofArgs::new(intent_hash, destination, vec![]),
                }
                .data(),
            })
            .unwrap();
        assert_eq!(result.return_data.program_id, layerzero_prover::ID);

        Option::<Proof>::try_from_slice(&result.return_data.data).unwrap()
    }
}
