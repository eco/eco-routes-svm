use anchor_lang::{system_program, InstructionData, ToAccountMetas};
use layerzero_prover::instructions::{required_alt_addresses, InitArgs, PathConfig};
use layerzero_prover::layerzero::{self, ENDPOINT_ID, LZ_RECEIVE_TYPES_SEED, ULN_ID};
use layerzero_prover::state::{Peer, PDA_PAYER_SEED};
use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, extend_lookup_table, freeze_lookup_table,
};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::chain::{self, Chain};
use crate::config::{self, Configs};
use crate::inputs::LayerZeroPeer;
use crate::layerzero_state::{self, Path};
use crate::plan::{Plan, LAYERZERO_PROVER};

const ALT_EXTEND_CHUNK: usize = 20;
/// The transaction maximum: `init_path` and `set_path_config` nest several endpoint and ULN302
/// CPIs whose cost on the real programs is unmeasured, and the default 200k must not decide it.
const COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
const PACKET_DATA_SIZE: usize = 1232;
/// Any price: the instruction's size does not depend on it.
const SIZING_PRICE_MICRO_LAMPORTS: u64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error(transparent)]
    Mismatch(#[from] config::Mismatch),
    #[error("{LAYERZERO_PROVER} is not in the plan")]
    UnplannedProgram,
    #[error("{LAYERZERO_PROVER}: eid {eid} has only one of its ULN configs, and init_config cannot run twice; close it (and the aggregator) with close.yml, raise its salt in scripts/program-salts.json and release again")]
    PartialPathConfig { eid: u32 },
    #[error("{LAYERZERO_PROVER} {transaction} needs {size} bytes, over the {limit}-byte transaction limit; use fewer DVNs")]
    TransactionTooLarge {
        transaction: String,
        size: usize,
        limit: usize,
    },
    #[error("{LAYERZERO_PROVER} store {address} does not exist after init")]
    StoreMissing { address: Pubkey },
}

/// One LayerZero setup action; `signature` is `None` when it was already done.
#[derive(Debug, Clone)]
pub struct Action {
    pub name: &'static str,
    pub signature: Option<Signature>,
}

/// Rejects a setup whose `init` or any `set_path_config` would not fit one legacy transaction
/// with its compute-unit limit and the price `apply --compute-unit-price` prepends: deployer as
/// sole signer and fee payer, built with the real instructions at `program`.
pub fn check_transaction_sizes(program: &Pubkey, peers: &[LayerZeroPeer]) -> Result<(), Error> {
    let deployer = Pubkey::new_from_array([1; 32]);
    let init = (
        "init".to_owned(),
        init_instruction(program, &deployer, peers.iter().map(Into::into).collect()),
    );
    let set_path_configs = peers.iter().map(|peer| {
        (
            format!("set_path_config for eid {}", peer.eid),
            set_path_config_instruction(program, &deployer, peer.eid, &peer.path),
        )
    });

    std::iter::once(init)
        .chain(set_path_configs)
        .try_for_each(|(transaction, instruction)| {
            let size = priced_transaction_size(&deployer, instruction);

            match size <= PACKET_DATA_SIZE {
                true => Ok(()),
                false => Err(Error::TransactionTooLarge {
                    transaction,
                    size,
                    limit: PACKET_DATA_SIZE,
                }),
            }
        })
}

fn priced_transaction_size(deployer: &Pubkey, instruction: Instruction) -> usize {
    let price = ComputeBudgetInstruction::set_compute_unit_price(SIZING_PRICE_MICRO_LAMPORTS);
    let instructions: Vec<Instruction> = [price].into_iter().chain(limited(instruction)).collect();
    let message = Message::new_with_blockhash(&instructions, Some(deployer), &Hash::default());

    bincode::serialize(&Transaction::new_unsigned(message))
        .expect("a transaction must serialize")
        .len()
}

/// Init, paths and lookup table in spec order, calling `observe` with each action as soon as it
/// has landed; every action reads before it writes.
pub fn apply<C: Chain>(
    chain: &mut C,
    plan: &Plan,
    deployer: &Keypair,
    observe: &mut impl FnMut(&Action),
) -> Result<(), Error> {
    let program = plan
        .programs
        .get(LAYERZERO_PROVER)
        .map(|planned| planned.state.address)
        .ok_or(Error::UnplannedProgram)?;
    let peers: Vec<(Peer, PathConfig)> = plan
        .inputs
        .layerzero_peers
        .iter()
        .map(|peer| (peer.into(), peer.path.clone()))
        .collect();
    let mut landed = |name, signature| observe(&Action { name, signature });

    landed("init", init(chain, plan, &program, deployer, &peers)?);
    peers.iter().try_for_each(|(peer, _)| {
        landed("init_path", init_path(chain, &program, deployer, peer)?);

        Ok::<_, Error>(())
    })?;
    peers.iter().try_for_each(|(peer, path)| {
        let signature = set_path_config(chain, &program, deployer, peer, path)?;
        landed("set_path_config", signature);

        Ok::<_, Error>(())
    })?;
    landed("set_alt", set_alt(chain, &program, deployer)?);

    Ok(())
}

fn init(
    chain: &mut impl Chain,
    plan: &Plan,
    program: &Pubkey,
    deployer: &Keypair,
    peers: &[(Peer, PathConfig)],
) -> Result<Option<Signature>, Error> {
    let live = Configs {
        layerzero_peers: config::layerzero_store(chain, program)?.map(|store| store.peers),
        ..Configs::default()
    };
    live.conflict(&plan.expected_configs)?;
    if !live.is_absent() {
        return Ok(None);
    }
    let peers = peers.iter().map(|(peer, _)| *peer).collect();

    send(
        chain,
        deployer,
        &limited(init_instruction(program, &deployer.pubkey(), peers)),
    )
}

fn init_path(
    chain: &mut impl Chain,
    program: &Pubkey,
    deployer: &Keypair,
    peer: &Peer,
) -> Result<Option<Signature>, Error> {
    if layerzero_state::read_path(chain, program, peer)?.nonce {
        return Ok(None);
    }

    send(
        chain,
        deployer,
        &limited(init_path_instruction(program, &deployer.pubkey(), peer)),
    )
}

fn set_path_config(
    chain: &mut impl Chain,
    program: &Pubkey,
    deployer: &Keypair,
    peer: &Peer,
    path: &PathConfig,
) -> Result<Option<Signature>, Error> {
    let live = layerzero_state::read_path(chain, program, peer)?;
    live.conflict(&Path::expected(program, peer, path))?;
    match (live.send.is_some(), live.receive.is_some()) {
        (true, true) => return Ok(None),
        (false, false) => {}
        _ => return Err(Error::PartialPathConfig { eid: peer.eid }),
    }
    let instruction = set_path_config_instruction(program, &deployer.pubkey(), peer.eid, path);

    send(chain, deployer, &limited(instruction))
}

/// Creates, fills and freezes a lookup table, then records it. Freezing and recording share a
/// transaction so a frozen table is never left unrecorded. A run that dies before that
/// transaction leaves an unfrozen table the next run cannot find, since only the `Store` points
/// at it; the next run creates a new one and the orphan only wastes rent.
fn set_alt(
    chain: &mut impl Chain,
    program: &Pubkey,
    deployer: &Keypair,
) -> Result<Option<Signature>, Error> {
    let address = layerzero_state::store_address(program);
    let store = config::layerzero_store(chain, program)?.ok_or(Error::StoreMissing { address })?;
    if store.alt != Pubkey::default() {
        return Ok(None);
    }
    let authority = deployer.pubkey();
    let (create, alt) = create_lookup_table(authority, authority, chain.slot()?);
    let required = required_alt_addresses(&store);
    let mut extensions = required
        .chunks(ALT_EXTEND_CHUNK)
        .map(|addresses| extend_lookup_table(alt, authority, Some(authority), addresses.to_vec()));
    let first = vec![
        create,
        extensions
            .next()
            .expect("required lookup table addresses must not be empty"),
    ];
    let last = vec![
        freeze_lookup_table(alt, authority),
        set_alt_instruction(program, &authority, &alt),
    ];

    std::iter::once(first)
        .chain(extensions.map(|extension| vec![extension]))
        .chain([last])
        .try_fold(None, |_, instructions| send(chain, deployer, &instructions))
}

fn limited(instruction: Instruction) -> [Instruction; 2] {
    [
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        instruction,
    ]
}

fn send(
    chain: &mut impl Chain,
    deployer: &Keypair,
    instructions: &[Instruction],
) -> Result<Option<Signature>, Error> {
    chain
        .send(instructions, &[deployer])
        .map(Some)
        .map_err(Into::into)
}

fn init_instruction(program: &Pubkey, deployer: &Pubkey, peers: Vec<Peer>) -> Instruction {
    let store = layerzero_state::store_address(program);
    let accounts = layerzero_prover::accounts::Init {
        payer: *deployer,
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store,
        lz_receive_types: Pubkey::find_program_address(
            &[LZ_RECEIVE_TYPES_SEED, store.as_ref()],
            program,
        )
        .0,
        system_program: system_program::ID,
        endpoint_program: ENDPOINT_ID,
        oapp_registry: layerzero::oapp_registry_pda(&store).0,
        endpoint_event_authority: layerzero::endpoint_event_authority().0,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::Init {
            args: InitArgs { peers },
        }
        .data(),
    }
}

fn init_path_instruction(program: &Pubkey, deployer: &Pubkey, peer: &Peer) -> Instruction {
    let store = layerzero_state::store_address(program);
    let accounts = layerzero_prover::accounts::InitPath {
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store,
        pda_payer: pda_payer_address(program),
        system_program: system_program::ID,
        endpoint_program: ENDPOINT_ID,
        oapp_registry: layerzero::oapp_registry_pda(&store).0,
        nonce: layerzero::nonce_pda(&store, peer.eid, &peer.address).0,
        pending_nonce: layerzero::pending_nonce_pda(&store, peer.eid, &peer.address).0,
        send_library_config: layerzero::send_library_config_pda(&store, peer.eid).0,
        receive_library_config: layerzero::receive_library_config_pda(&store, peer.eid).0,
        message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
        endpoint_event_authority: layerzero::endpoint_event_authority().0,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::InitPath { eid: peer.eid }.data(),
    }
}

fn set_path_config_instruction(
    program: &Pubkey,
    deployer: &Pubkey,
    eid: u32,
    config: &PathConfig,
) -> Instruction {
    let store = layerzero_state::store_address(program);
    let accounts = layerzero_prover::accounts::SetPathConfig {
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store,
        pda_payer: pda_payer_address(program),
        system_program: system_program::ID,
        endpoint_program: ENDPOINT_ID,
        oapp_registry: layerzero::oapp_registry_pda(&store).0,
        message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
        uln_settings: layerzero::uln_settings_pda().0,
        uln_program: ULN_ID,
        uln_send_config: layerzero::uln_send_config_pda(eid, &store).0,
        uln_receive_config: layerzero::uln_receive_config_pda(eid, &store).0,
        uln_default_send_config: layerzero::uln_default_send_config_pda(eid).0,
        uln_default_receive_config: layerzero::uln_default_receive_config_pda(eid).0,
        uln_event_authority: layerzero::uln_event_authority().0,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::SetPathConfig {
            eid,
            config: config.clone(),
        }
        .data(),
    }
}

fn set_alt_instruction(program: &Pubkey, deployer: &Pubkey, alt: &Pubkey) -> Instruction {
    let accounts = layerzero_prover::accounts::SetAlt {
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store: layerzero_state::store_address(program),
        alt: *alt,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::SetAlt {}.data(),
    }
}

fn pda_payer_address(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PDA_PAYER_SEED], program).0
}

#[cfg(test)]
mod tests {
    use layerzero_prover::layerzero::{ExecutorConfig, UlnConfig};
    use layerzero_prover::state::MAX_PEERS;
    use solana_sdk::account::Account;
    use solana_sdk::hash::Hash;
    use solana_sdk::message::Message;
    use solana_sdk::transaction::Transaction;

    use super::*;
    use crate::chain::Priced;
    use crate::testing::{full_setup, plan, uln_account_data, FullSetup};

    const PACKET_DATA_SIZE: usize = 1232;
    /// The most required DVNs per ULN (same on send and receive) whose `set_path_config` still fits
    /// a packet with a compute-unit price prepended; `Plan::build` rejects more.
    const MAX_DVNS: usize = 7;

    /// Refuses any transaction that would not fit a packet.
    struct SizingChain;

    impl Chain for SizingChain {
        fn account(&self, _address: &Pubkey) -> Result<Option<Account>, chain::Error> {
            Ok(None)
        }

        fn send(
            &mut self,
            instructions: &[Instruction],
            signers: &[&Keypair],
        ) -> Result<Signature, chain::Error> {
            let payer = signers[0].pubkey();
            let transaction = Transaction::new_unsigned(Message::new(instructions, Some(&payer)));
            let size = bincode::serialize(&transaction).unwrap().len();

            match size <= PACKET_DATA_SIZE {
                true => Ok(Signature::default()),
                false => Err(chain::Error::SendFailed {
                    reason: format!("{size} bytes exceeds {PACKET_DATA_SIZE}"),
                }),
            }
        }

        fn slot(&self) -> Result<u64, chain::Error> {
            Ok(0)
        }

        fn genesis_hash(&self) -> Result<Hash, chain::Error> {
            unimplemented!("applying a plan never reads the genesis hash")
        }
    }

    fn applied(
        chain: &mut impl Chain,
        plan: &Plan,
        deployer: &Keypair,
    ) -> Result<Vec<Action>, Error> {
        let mut actions = Vec::new();
        apply(chain, plan, deployer, &mut |action| {
            actions.push(action.clone())
        })?;

        Ok(actions)
    }

    fn max_uln() -> UlnConfig {
        UlnConfig {
            confirmations: 15,
            required_dvn_count: MAX_DVNS as u8,
            optional_dvn_count: 255,
            optional_dvn_threshold: 0,
            required_dvns: (0..MAX_DVNS).map(|_| Pubkey::new_unique()).collect(),
            optional_dvns: vec![],
        }
    }

    fn with_other_send_executor(setup: &mut FullSetup, plan: &Plan) {
        let send = plan.expected_configs.layerzero_paths[0]
            .send
            .clone()
            .unwrap();
        let mut executor = send.executor;
        executor.max_message_size += 1;
        setup.chain.accounts.insert(
            setup.send_config,
            Account {
                owner: ULN_ID,
                data: uln_account_data("SendConfig", &(255u8, send.uln, executor)),
                ..Account::default()
            },
        );
    }

    #[test]
    fn complete_setup_sends_nothing() {
        let plan = plan(5_000_000);
        let mut setup = full_setup(&plan);

        let actions = applied(&mut setup.chain, &plan, &Keypair::new()).unwrap();

        assert!(actions.iter().all(|action| action.signature.is_none()));
        assert!(setup.chain.sent.is_empty());
    }

    #[test]
    fn existing_path_config_that_differs_is_refused_without_a_write() {
        let plan = plan(5_000_000);
        let mut setup = full_setup(&plan);
        with_other_send_executor(&mut setup, &plan);

        let result = applied(&mut setup.chain, &plan, &Keypair::new());

        assert!(matches!(
            result,
            Err(Error::Mismatch(mismatch)) if mismatch.program == LAYERZERO_PROVER
        ));
        assert!(setup.chain.sent.is_empty());
    }

    #[test]
    fn path_with_only_one_uln_config_is_refused_without_a_write() {
        let plan = plan(5_000_000);
        let mut setup = full_setup(&plan);
        setup.chain.accounts.remove(&setup.receive_config);

        let result = applied(&mut setup.chain, &plan, &Keypair::new());

        assert!(matches!(result, Err(Error::PartialPathConfig { eid: 1 })));
        assert!(setup.chain.sent.is_empty());
    }

    #[test]
    fn largest_transactions_fit_a_packet_with_a_compute_unit_price() {
        let program = Pubkey::new_unique();
        let deployer = Keypair::new();
        let peers: Vec<Peer> = (1..=MAX_PEERS as u32)
            .map(|eid| Peer {
                eid,
                address: [0; 32].into(),
                chain_id: eid.into(),
            })
            .collect();
        let path = PathConfig {
            send_uln: max_uln(),
            receive_uln: max_uln(),
            executor: ExecutorConfig {
                max_message_size: 10_000,
                executor: Pubkey::new_unique(),
            },
        };
        let mut chain = Priced::new(SizingChain, 1_000_000);

        send(
            &mut chain,
            &deployer,
            &[init_instruction(&program, &deployer.pubkey(), peers)],
        )
        .unwrap();
        send(
            &mut chain,
            &deployer,
            &[set_path_config_instruction(
                &program,
                &deployer.pubkey(),
                1,
                &path,
            )],
        )
        .unwrap();
    }
}
