use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;

use crate::chain::{self, Chain};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    New,
    Partial,
    Live,
    Foreign { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramState {
    pub address: Pubkey,
    pub status: Status,
    pub authority: Option<Pubkey>,
    pub data_hash: Option<[u8; 32]>,
}

pub fn classify(
    chain: &impl Chain,
    address: &Pubkey,
    release_so: &[u8],
    deployer: &Pubkey,
) -> Result<ProgramState, Error> {
    let Some(program) = chain.account(address)? else {
        return Ok(state(address, Status::New, None, None));
    };
    let Some(programdata_address) = programdata_address(&program) else {
        return Ok(foreign(address, "not a program"));
    };
    let Some(programdata) = chain.account(&programdata_address)? else {
        return Ok(foreign(address, "programdata account missing"));
    };
    let Some((authority, elf)) = split_programdata(&programdata) else {
        return Ok(foreign(address, "malformed programdata"));
    };
    let data_hash = executable_hash(elf);
    let status = status(
        authority,
        data_hash == executable_hash(release_so),
        deployer,
    );

    Ok(state(address, status, authority, Some(data_hash)))
}

/// SHA-256 of `program_data` without trailing zero bytes, as `solana-verify` computes it.
pub fn executable_hash(program_data: &[u8]) -> [u8; 32] {
    let end = program_data
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |index| index + 1);

    solana_sha256_hasher::hash(&program_data[..end]).to_bytes()
}

fn status(authority: Option<Pubkey>, hash_matches: bool, deployer: &Pubkey) -> Status {
    match authority {
        _ if !hash_matches => Status::Foreign {
            reason: "hash differs".to_owned(),
        },
        None => Status::Live,
        Some(authority) if authority == *deployer => Status::Partial,
        Some(authority) => Status::Foreign {
            reason: format!("authority {authority}"),
        },
    }
}

fn programdata_address(program: &Account) -> Option<Pubkey> {
    if program.owner != solana_sdk_ids::bpf_loader_upgradeable::id() {
        return None;
    }

    match bincode::deserialize(&program.data).ok()? {
        UpgradeableLoaderState::Program {
            programdata_address,
        } => Some(programdata_address),
        _ => None,
    }
}

fn split_programdata(programdata: &Account) -> Option<(Option<Pubkey>, &[u8])> {
    let elf = programdata
        .data
        .get(UpgradeableLoaderState::size_of_programdata_metadata()..)?;

    match bincode::deserialize(&programdata.data).ok()? {
        UpgradeableLoaderState::ProgramData {
            upgrade_authority_address,
            ..
        } => Some((upgrade_authority_address, elf)),
        _ => None,
    }
}

fn state(
    address: &Pubkey,
    status: Status,
    authority: Option<Pubkey>,
    data_hash: Option<[u8; 32]>,
) -> ProgramState {
    ProgramState {
        address: *address,
        status,
        authority,
        data_hash,
    }
}

fn foreign(address: &Pubkey, reason: &str) -> ProgramState {
    let reason = reason.to_owned();

    state(address, Status::Foreign { reason }, None, None)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use solana_sdk::account::Account;
    use solana_sdk::hash::Hash;
    use solana_sdk::instruction::Instruction;
    use solana_sdk::signature::{Keypair, Signature};

    use super::*;

    const RELEASE_SO: &[u8] = include_bytes!("../tests/fixtures/mock_igp.so");
    const RELEASE_SO_EXECUTABLE_HASH: &str =
        "ec38dbedf54cf0c0348a788d297830366326a5216d995d697b9c81965df18651";

    #[derive(Default)]
    struct FakeChain {
        accounts: HashMap<Pubkey, Account>,
    }

    impl Chain for FakeChain {
        fn account(&self, address: &Pubkey) -> Result<Option<Account>, chain::Error> {
            Ok(self.accounts.get(address).cloned())
        }

        fn send(
            &mut self,
            _instructions: &[Instruction],
            _signers: &[&Keypair],
        ) -> Result<Signature, chain::Error> {
            Ok(Signature::default())
        }

        fn slot(&self) -> Result<u64, chain::Error> {
            Ok(0)
        }

        fn genesis_hash(&self) -> Result<Hash, chain::Error> {
            unimplemented!("classification never reads the genesis hash")
        }
    }

    fn address() -> Pubkey {
        Pubkey::new_from_array([1; 32])
    }

    fn deployer() -> Pubkey {
        Pubkey::new_from_array([2; 32])
    }

    fn programdata_address() -> Pubkey {
        Pubkey::new_from_array([3; 32])
    }

    fn account(owner: Pubkey, data: Vec<u8>) -> Account {
        Account {
            lamports: 1,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        }
    }

    fn deployed(elf: &[u8], authority: Option<Pubkey>) -> FakeChain {
        let program = bincode::serialize(&UpgradeableLoaderState::Program {
            programdata_address: programdata_address(),
        })
        .unwrap();
        let mut programdata = bincode::serialize(&UpgradeableLoaderState::ProgramData {
            slot: 7,
            upgrade_authority_address: authority,
        })
        .unwrap();
        programdata.resize(UpgradeableLoaderState::size_of_programdata_metadata(), 0);
        programdata.extend_from_slice(elf);
        programdata.extend_from_slice(&[0; 16]);

        FakeChain {
            accounts: HashMap::from([
                (
                    address(),
                    account(solana_sdk_ids::bpf_loader_upgradeable::id(), program),
                ),
                (
                    programdata_address(),
                    account(solana_sdk_ids::bpf_loader_upgradeable::id(), programdata),
                ),
            ]),
        }
    }

    fn classified(chain: &FakeChain) -> ProgramState {
        classify(chain, &address(), RELEASE_SO, &deployer()).unwrap()
    }

    #[test]
    fn new_when_no_account() {
        let state = classified(&FakeChain::default());

        assert_eq!(state.status, Status::New);
        assert_eq!(state.authority, None);
        assert_eq!(state.data_hash, None);
    }

    #[test]
    fn live_when_hash_matches_and_immutable() {
        let state = classified(&deployed(RELEASE_SO, None));

        assert_eq!(state.status, Status::Live);
        assert_eq!(state.authority, None);
        assert_eq!(state.data_hash, Some(executable_hash(RELEASE_SO)));
    }

    #[test]
    fn partial_when_hash_matches_and_authority_is_deployer() {
        let state = classified(&deployed(RELEASE_SO, Some(deployer())));

        assert_eq!(state.status, Status::Partial);
        assert_eq!(state.authority, Some(deployer()));
    }

    #[test]
    fn foreign_when_hash_differs() {
        let state = classified(&deployed(&[1, 2, 3], Some(deployer())));

        assert_eq!(
            state.status,
            Status::Foreign {
                reason: "hash differs".to_owned()
            }
        );
    }

    #[test]
    fn foreign_when_authority_is_someone_else() {
        let other = Pubkey::new_from_array([9; 32]);
        let state = classified(&deployed(RELEASE_SO, Some(other)));

        assert_eq!(
            state.status,
            Status::Foreign {
                reason: format!("authority {other}")
            }
        );
        assert_eq!(state.authority, Some(other));
    }

    #[test]
    fn foreign_when_not_a_program() {
        let chain = FakeChain {
            accounts: HashMap::from([(
                address(),
                account(solana_sdk_ids::system_program::id(), vec![]),
            )]),
        };

        assert_eq!(
            classified(&chain).status,
            Status::Foreign {
                reason: "not a program".to_owned()
            }
        );
    }

    #[test]
    fn foreign_when_a_loader_account_is_a_buffer() {
        let buffer = bincode::serialize(&UpgradeableLoaderState::Buffer {
            authority_address: Some(deployer()),
        })
        .unwrap();
        let chain = FakeChain {
            accounts: HashMap::from([(
                address(),
                account(solana_sdk_ids::bpf_loader_upgradeable::id(), buffer),
            )]),
        };

        assert_eq!(
            classified(&chain).status,
            Status::Foreign {
                reason: "not a program".to_owned()
            }
        );
    }

    #[test]
    fn foreign_when_programdata_is_missing() {
        let mut chain = deployed(RELEASE_SO, Some(deployer()));
        chain.accounts.remove(&programdata_address());

        assert_eq!(
            classified(&chain).status,
            Status::Foreign {
                reason: "programdata account missing".to_owned()
            }
        );
    }

    #[test]
    fn foreign_when_programdata_is_malformed() {
        let not_programdata = bincode::serialize(&UpgradeableLoaderState::Program {
            programdata_address: programdata_address(),
        })
        .unwrap();
        let truncated = vec![3, 0, 0, 0];

        [not_programdata, truncated].into_iter().for_each(|data| {
            let mut chain = deployed(RELEASE_SO, Some(deployer()));
            chain.accounts.insert(
                programdata_address(),
                account(solana_sdk_ids::bpf_loader_upgradeable::id(), data),
            );

            assert_eq!(
                classified(&chain).status,
                Status::Foreign {
                    reason: "malformed programdata".to_owned()
                }
            );
        });
    }

    #[test]
    fn executable_hash_trims_trailing_zeros() {
        assert_eq!(executable_hash(&[1, 2, 0, 0]), executable_hash(&[1, 2]));
    }

    #[test]
    fn executable_hash_matches_solana_verify() {
        assert_eq!(
            hex::encode(executable_hash(RELEASE_SO)),
            RELEASE_SO_EXECUTABLE_HASH
        );
    }
}
