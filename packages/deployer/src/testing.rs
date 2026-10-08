use std::collections::HashMap;

use anchor_lang::{AccountSerialize, AnchorSerialize};
use solana_sdk::account::Account;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk_ids::bpf_loader_upgradeable;

use crate::chain::{self, Chain};
use crate::classify::{ProgramState, Status};
use crate::config::Configs;
use crate::inputs::{Inputs, RawInputs};
use crate::layerzero_state;
use crate::plan::{
    Cluster, Plan, Release, ReleaseProgram, AGGREGATOR_MEMBERS, AGGREGATOR_PROVER, HYPER_PROVER,
    LAYERZERO_PROVER, LOCAL_PROVER, POLYMER_PROVER,
};

pub const SENDER: &str = "0xAbCdEf0123456789aBcDeF0123456789abcdef01";
pub const DVN: &str = "11111111111111111111111111111112";
pub const EXECUTOR: &str = "11111111111111111111111111111113";

pub struct RecordingChain {
    pub accounts: HashMap<Pubkey, Account>,
    pub sent: Vec<Instruction>,
    pub genesis_hash: Hash,
}

impl Default for RecordingChain {
    fn default() -> Self {
        Self {
            accounts: HashMap::default(),
            sent: Vec::default(),
            genesis_hash: Cluster::Devnet.genesis_hash(),
        }
    }
}

impl Chain for RecordingChain {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>, chain::Error> {
        Ok(self.accounts.get(address).cloned())
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        _signers: &[&Keypair],
    ) -> Result<Signature, chain::Error> {
        self.sent.extend_from_slice(instructions);

        Ok(Signature::default())
    }

    fn slot(&self) -> Result<u64, chain::Error> {
        Ok(0)
    }

    fn genesis_hash(&self) -> Result<Hash, chain::Error> {
        Ok(self.genesis_hash)
    }
}

pub fn address(index: u8) -> Pubkey {
    Pubkey::new_from_array([index; 32])
}

/// The aggregator depends on its members; the fixture's other programs on nothing.
pub fn dependencies(program: &str) -> Vec<String> {
    match program {
        AGGREGATOR_PROVER => AGGREGATOR_MEMBERS.map(Into::into).to_vec(),
        _ => vec![],
    }
}

pub fn release_address(program: &str) -> Pubkey {
    match program {
        HYPER_PROVER => address(201),
        LOCAL_PROVER => address(202),
        POLYMER_PROVER => address(203),
        AGGREGATOR_PROVER => address(204),
        _ => layerzero_prover::ID,
    }
}

pub fn plan(reserve_lamports: u64) -> Plan {
    let names = [
        HYPER_PROVER,
        LOCAL_PROVER,
        POLYMER_PROVER,
        AGGREGATOR_PROVER,
        LAYERZERO_PROVER,
    ];
    let release = Release {
        version: "0.0.1".into(),
        cluster: Cluster::Devnet,
        programs: names
            .iter()
            .map(|name| {
                let program = ReleaseProgram {
                    address: release_address(name),
                    so_sha256: [0; 32],
                    dependencies: dependencies(name),
                };

                ((*name).into(), program)
            })
            .collect(),
    };
    let states = names
        .iter()
        .map(|name| {
            let state = ProgramState {
                address: release_address(name),
                status: Status::Partial,
                authority: Some(address(1)),
                data_hash: None,
            };

            ((*name).into(), state)
        })
        .collect();
    let uln = format!(
        r#"{{"confirmations":15,"required_dvn_count":1,"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":["{DVN}"],"optional_dvns":[]}}"#
    );
    let inputs = Inputs::parse(RawInputs {
        hyper_senders: SENDER.into(),
        polymer_emitters: SENDER.into(),
        layerzero: format!(
            r#"{{"peers":[{{"eid":1,"address":"{SENDER}","chain_id":1,"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}]}}"#
        ),
        hyper_reserve_lamports: reserve_lamports.to_string(),
        layerzero_reserve_lamports: reserve_lamports.to_string(),
        compute_unit_price: "0".into(),
    })
    .unwrap();

    Plan::build(
        release,
        Cluster::Devnet.genesis_hash(),
        address(1),
        states,
        Configs::default(),
        inputs,
    )
    .unwrap()
}

pub fn chain_with_deployed_members() -> RecordingChain {
    let executable = Account {
        executable: true,
        ..Account::default()
    };

    RecordingChain {
        accounts: AGGREGATOR_MEMBERS
            .iter()
            .map(|member| (release_address(member), executable.clone()))
            .collect(),
        ..RecordingChain::default()
    }
}

pub fn pda(seed: &[u8], program: &str) -> Pubkey {
    Pubkey::find_program_address(&[seed], &release_address(program)).0
}

pub fn program_data(program: &str) -> Pubkey {
    Pubkey::find_program_address(
        &[release_address(program).as_ref()],
        &bpf_loader_upgradeable::id(),
    )
    .0
}

pub fn layerzero_store(plan: &Plan) -> layerzero_prover::state::Store {
    layerzero_prover::state::Store {
        peers: plan.expected_configs.layerzero_peers.clone().unwrap(),
        alt: Pubkey::default(),
    }
}

pub fn chain_with_layerzero_store(plan: &Plan) -> RecordingChain {
    let program = release_address(LAYERZERO_PROVER);
    let mut data = Vec::new();
    layerzero_store(plan).try_serialize(&mut data).unwrap();
    let mut chain = chain_with_deployed_members();
    chain.accounts.insert(
        layerzero_state::store_address(&program),
        Account {
            owner: program,
            data,
            ..Account::default()
        },
    );

    chain
}

pub fn keys(instruction: &Instruction) -> Vec<Pubkey> {
    instruction
        .accounts
        .iter()
        .map(|account| account.pubkey)
        .collect()
}

pub struct FullSetup {
    pub chain: RecordingChain,
    pub store: Pubkey,
    pub nonce: Pubkey,
    pub send_config: Pubkey,
    pub receive_config: Pubkey,
    pub alt: Pubkey,
}

/// Every config, path account and the lookup table `apply` leaves behind, at the release
/// addresses.
pub fn full_setup(plan: &Plan) -> FullSetup {
    let program = release_address(LAYERZERO_PROVER);
    let alt = address(99);
    let store = layerzero_prover::state::Store {
        alt,
        ..layerzero_store(plan)
    };
    let peer = store.peers[0];
    let accounts = layerzero_state::path_accounts(&program, &peer);
    let expected = &plan.expected_configs;
    let path = expected.layerzero_paths[0].clone();
    let send = path.send.unwrap();
    let receive = path.receive.unwrap();
    let mut chain = chain_with_deployed_members();
    let configs = [
        (
            HYPER_PROVER,
            hyper_prover::state::CONFIG_SEED,
            serialize(&hyper_prover::state::Config {
                whitelisted_senders: expected.hyper_senders.clone().unwrap(),
            }),
        ),
        (
            POLYMER_PROVER,
            polymer_prover::state::CONFIG_SEED,
            serialize(&polymer_prover::state::Config {
                whitelisted_emitters: expected.polymer_emitters.clone().unwrap(),
            }),
        ),
        (
            AGGREGATOR_PROVER,
            aggregator_prover::state::CONFIG_SEED,
            serialize(&aggregator_prover::state::Config {
                provers: expected.aggregator_provers.clone().unwrap(),
            }),
        ),
        (
            LAYERZERO_PROVER,
            layerzero_prover::state::STORE_SEED,
            serialize(&store),
        ),
    ];
    configs.into_iter().for_each(|(name, seed, data)| {
        let owner = release_address(name);
        chain.accounts.insert(
            Pubkey::find_program_address(&[seed], &owner).0,
            Account {
                owner,
                data,
                ..Account::default()
            },
        );
    });
    [
        (
            accounts.nonce,
            layerzero_prover::layerzero::ENDPOINT_ID,
            vec![1],
        ),
        (
            accounts.send_config,
            layerzero_prover::layerzero::ULN_ID,
            uln_account_data("SendConfig", &(255u8, send.uln, send.executor)),
        ),
        (
            accounts.receive_config,
            layerzero_prover::layerzero::ULN_ID,
            uln_account_data("ReceiveConfig", &(255u8, receive.uln)),
        ),
        (
            alt,
            solana_sdk_ids::address_lookup_table::id(),
            frozen_table(&store),
        ),
    ]
    .into_iter()
    .for_each(|(address, owner, data)| {
        chain.accounts.insert(
            address,
            Account {
                owner,
                data,
                ..Account::default()
            },
        );
    });

    FullSetup {
        chain,
        store: layerzero_state::store_address(&program),
        nonce: accounts.nonce,
        send_config: accounts.send_config,
        receive_config: accounts.receive_config,
        alt,
    }
}

/// What ULN302 stores: the Anchor discriminator, then the Borsh account.
pub fn uln_account_data(name: &str, account: &impl AnchorSerialize) -> Vec<u8> {
    let mut data =
        solana_sha256_hasher::hash(format!("account:{name}").as_bytes()).to_bytes()[..8].to_vec();
    account.serialize(&mut data).unwrap();

    data
}

/// A lookup table in the program's layout: tag 1, never deactivated, no authority.
pub fn frozen_table(store: &layerzero_prover::state::Store) -> Vec<u8> {
    let mut data = vec![0u8; 56];
    data[..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..12].copy_from_slice(&u64::MAX.to_le_bytes());
    layerzero_prover::instructions::required_alt_addresses(store)
        .iter()
        .for_each(|address| data.extend_from_slice(address.as_ref()));

    data
}

fn serialize(account: &impl AccountSerialize) -> Vec<u8> {
    let mut data = Vec::new();
    account.try_serialize(&mut data).unwrap();

    data
}
