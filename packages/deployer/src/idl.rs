//! The release IDLs as each program's canonical Program Metadata account, where Anchor 1.x and
//! explorers read them. Only the program's upgrade authority can create or change that account,
//! so the upload runs in the deploy job, signed by the deployer, and must land before finalize.
//!
//! The upload is a state machine over the one account at the canonical PDA, re-read before every
//! transaction: absent → allocate (rent transfer + `allocate`) → buffer → `write` each chunk that
//! differs → `initialize` → metadata, which is done when its inflated data equals the file. A
//! metadata account holding anything else is closed and rebuilt. Every step is checked on-chain
//! against the state it expects, so a stale read only costs a failed transaction, and a run that
//! stops anywhere resumes from what the chain holds: the official client sends chunk writes in
//! parallel and cannot continue a half-written buffer at the canonical address.

use std::io::{Read, Write as _};
use std::time::Duration;
use std::{fmt, thread};

use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use solana_sdk::account::Account;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;

use crate::chain::{self, Chain};

pub const PROGRAM_METADATA_ID: Pubkey =
    Pubkey::from_str_const("ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S");
/// `"idl"` zero-padded to the program's 16-byte seed; the canonical PDA has no authority seed.
pub const SEED: [u8; SEED_LEN] = *b"idl\0\0\0\0\0\0\0\0\0\0\0\0\0";
/// Bytes of IDL each `write` carries; a write with a rent top-up stays within a v1 transaction.
pub const WRITE_CHUNK: usize = 3_600;
/// Consecutive failed transactions before the upload gives up; a rerun resumes.
const MAX_FAILURES: u32 = 5;
const SEED_LEN: usize = 16;
/// Both account kinds start with a 96-byte header; the data follows it.
const HEADER_LEN: usize = 96;

// Account discriminators.
const BUFFER: u8 = 1;
const METADATA: u8 = 2;
// Instruction discriminators.
const WRITE: u8 = 0;
const INITIALIZE: u8 = 1;
const CLOSE: u8 = 6;
const ALLOCATE: u8 = 7;
// Header values `initialize` records: UTF-8 JSON, zlib-compressed, stored in the account.
const ENCODING_UTF8: u8 = 1;
const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_GZIP: u8 = 1;
const COMPRESSION_ZLIB: u8 = 2;
const FORMAT_JSON: u8 = 1;
const DATA_SOURCE_DIRECT: u8 = 0;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error("{program}: canonical IDL account {address}: {reason}")]
    Refused {
        program: String,
        address: Pubkey,
        reason: String,
    },
    #[error("{program}: {failures} transactions in a row failed; rerun the step, it resumes from the chain: {source}")]
    GaveUp {
        program: String,
        failures: u32,
        #[source]
        source: chain::Error,
    },
    #[error("{program}: the canonical IDL still differs from the release after {steps} transactions; rerun the step")]
    NoProgress { program: String, steps: usize },
    #[error("the rent sysvar is missing or malformed")]
    Rent,
}

/// What the canonical PDA holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Canonical {
    Absent {
        lamports: u64,
    },
    /// Allocated and maybe partly written, not yet initialized.
    Buffer {
        program: Option<Pubkey>,
        authority: Option<Pubkey>,
        canonical: bool,
        seed: [u8; SEED_LEN],
        written: Vec<u8>,
        lamports: u64,
    },
    Metadata {
        program: Pubkey,
        canonical: bool,
        mutable: bool,
        seed: [u8; SEED_LEN],
        /// The stored JSON, inflated; `None` unless the header says direct UTF-8 JSON.
        content: Option<Vec<u8>>,
        lamports: u64,
    },
    Unknown {
        reason: String,
    },
}

/// The one transaction the upload sends next, or `Done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Done,
    Allocate,
    /// Writes `compressed[start..end]` at `start`.
    Write {
        start: usize,
        end: usize,
    },
    Initialize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The canonical IDL already equalled the file; nothing was sent.
    Unchanged,
    Uploaded,
}

impl fmt::Display for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Done => formatter.write_str("done"),
            Self::Allocate => formatter.write_str("allocate"),
            Self::Write { start, end } => write!(formatter, "write {start}..{end}"),
            Self::Initialize => formatter.write_str("initialize"),
            Self::Close => formatter.write_str("close"),
        }
    }
}

pub fn canonical_address(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program.as_ref(), &SEED], &PROGRAM_METADATA_ID).0
}

/// zlib at the default level: deterministic for a given input, so a rerun writes only the
/// chunks a failed run left behind.
pub fn compress(idl: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(idl)
        .expect("writing to a Vec never fails");

    encoder.finish().expect("writing to a Vec never fails")
}

pub fn observe(account: Option<&Account>) -> Canonical {
    let Some(account) = account.filter(|account| account.lamports > 0) else {
        return Canonical::Absent { lamports: 0 };
    };
    if account.owner == solana_sdk_ids::system_program::id() && account.data.is_empty() {
        return Canonical::Absent {
            lamports: account.lamports,
        };
    }
    if account.owner != PROGRAM_METADATA_ID {
        return Canonical::Unknown {
            reason: format!("owned by {}", account.owner),
        };
    }
    let data = &account.data;
    if data.len() < HEADER_LEN {
        return Canonical::Unknown {
            reason: format!("{} bytes, shorter than the header", data.len()),
        };
    }
    let seed = data[67..83].try_into().expect("16 bytes");

    match data[0] {
        BUFFER => Canonical::Buffer {
            program: optional_pubkey(&data[1..33]),
            authority: optional_pubkey(&data[33..65]),
            canonical: data[65] == 1,
            seed: data[66..82].try_into().expect("16 bytes"),
            written: data[HEADER_LEN..].to_vec(),
            lamports: account.lamports,
        },
        METADATA => Canonical::Metadata {
            program: Pubkey::try_from(&data[1..33]).expect("32 bytes"),
            canonical: data[66] == 1,
            mutable: data[65] == 1,
            seed,
            content: content(data),
            lamports: account.lamports,
        },
        discriminator => Canonical::Unknown {
            reason: format!("account discriminator {discriminator}"),
        },
    }
}

/// Whether `account`, read at `program`'s canonical PDA, holds exactly `idl`.
pub fn matches(account: Option<&Account>, program: &Pubkey, idl: &[u8]) -> bool {
    matches!(
        observe(account),
        Canonical::Metadata { program: stored, canonical: true, seed: SEED, content: Some(content), .. }
            if stored == *program && content == idl
    )
}

/// The next transaction towards `program`'s canonical IDL holding `idl`, deflated to
/// `compressed`; an error when the account is in a state only a human should resolve.
pub fn next(
    state: &Canonical,
    program: &Pubkey,
    deployer: &Pubkey,
    compressed: &[u8],
    idl: &[u8],
) -> Result<Step, String> {
    match state {
        Canonical::Absent { .. } => Ok(Step::Allocate),
        Canonical::Unknown { reason } => Err(reason.clone()),
        Canonical::Buffer {
            program: stored,
            authority,
            canonical,
            seed,
            written,
            ..
        } => {
            if *stored != Some(*program) || !canonical || *seed != SEED {
                return Err("a buffer that is not this program's canonical IDL".into());
            }
            if *authority != Some(*deployer) {
                return Err(format!(
                    "a buffer held by {authority:?}; only it can close or finish it"
                ));
            }
            // `initialize` takes the whole buffer as the data: a longer one holds another IDL.
            if written.len() > compressed.len() {
                return Ok(Step::Close);
            }

            Ok((0..compressed.len())
                .step_by(WRITE_CHUNK)
                .map(|start| (start, (start + WRITE_CHUNK).min(compressed.len())))
                .find(|(start, end)| written.get(*start..*end) != Some(&compressed[*start..*end]))
                .map_or(Step::Initialize, |(start, end)| Step::Write { start, end }))
        }
        Canonical::Metadata {
            program: stored,
            canonical,
            mutable,
            seed,
            content,
            ..
        } => {
            if stored != program || !canonical || *seed != SEED {
                return Err("metadata that is not this program's canonical IDL".into());
            }
            match (content.as_deref() == Some(idl), mutable) {
                (true, _) => Ok(Step::Done),
                (false, true) => Ok(Step::Close),
                (false, false) => Err("immutable, and it differs from the release IDL".into()),
            }
        }
    }
}

/// The instructions of `step`, with a rent top-up first when the account will grow past what it
/// holds. `programdata` is the program's, which the metadata program checks the authority in.
pub fn instructions(
    step: Step,
    program: &Pubkey,
    programdata: &Pubkey,
    deployer: &Pubkey,
    compressed: &[u8],
    top_up: u64,
) -> Vec<Instruction> {
    let canonical = canonical_address(program);
    let authorized = |data: Vec<u8>, last: AccountMeta| Instruction {
        program_id: PROGRAM_METADATA_ID,
        accounts: vec![
            AccountMeta::new(canonical, false),
            AccountMeta::new_readonly(*deployer, true),
            AccountMeta::new_readonly(*program, false),
            AccountMeta::new_readonly(*programdata, false),
            last,
        ],
        data,
    };
    let system = AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false);
    let instruction = match step {
        Step::Done => return vec![],
        Step::Allocate => authorized([&[ALLOCATE][..], &SEED].concat(), system),
        Step::Write { start, end } => Instruction {
            program_id: PROGRAM_METADATA_ID,
            accounts: vec![
                AccountMeta::new(canonical, false),
                AccountMeta::new_readonly(*deployer, true),
                // The program's own ID: the data comes from the instruction, not a buffer.
                AccountMeta::new_readonly(PROGRAM_METADATA_ID, false),
            ],
            data: [
                &[WRITE][..],
                &u32::try_from(start)
                    .expect("an IDL fits in u32")
                    .to_le_bytes(),
                &compressed[start..end],
            ]
            .concat(),
        },
        Step::Initialize => authorized(
            [
                &[INITIALIZE][..],
                &SEED,
                &[
                    ENCODING_UTF8,
                    COMPRESSION_ZLIB,
                    FORMAT_JSON,
                    DATA_SOURCE_DIRECT,
                ],
            ]
            .concat(),
            system,
        ),
        // The rent goes back to the deployer.
        Step::Close => authorized(vec![CLOSE], AccountMeta::new(*deployer, false)),
    };
    let top_up = match (step, top_up) {
        (Step::Close, _) | (_, 0) => None,
        (_, lamports) => Some(solana_system_interface::instruction::transfer(
            deployer, &canonical, lamports,
        )),
    };

    top_up.into_iter().chain([instruction]).collect()
}

/// Drives `program`'s canonical IDL to `idl`, one transaction at a time, and returns once a read
/// shows the stored data inflating to exactly `idl`. Reports each transaction through `sent`.
/// A failed transaction is retried after `backoff` from a fresh read; the upload gives up after
/// `MAX_FAILURES` in a row.
#[allow(clippy::too_many_arguments)]
pub fn upload(
    chain: &mut impl Chain,
    name: &str,
    program: &Pubkey,
    programdata: &Pubkey,
    deployer: &Keypair,
    idl: &[u8],
    backoff: Duration,
    sent: &mut impl FnMut(Step, Signature),
) -> Result<Outcome, Error> {
    let address = canonical_address(program);
    let compressed = compress(idl);
    let mut rent_exempt = None;
    // Allocate, every chunk, initialize, and a close of whatever was there, each allowed one
    // duplicate from a stale read, plus the retries.
    let max_steps = 2 * (compressed.len().div_ceil(WRITE_CHUNK) + 3) + MAX_FAILURES as usize;
    let mut failures = 0;

    for steps in 0..=max_steps {
        let state = observe(chain.account(&address)?.as_ref());
        let step =
            next(&state, program, &deployer.pubkey(), &compressed, idl).map_err(|reason| {
                Error::Refused {
                    program: name.into(),
                    address,
                    reason,
                }
            })?;
        if step == Step::Done {
            return Ok(match steps {
                0 => Outcome::Unchanged,
                _ => Outcome::Uploaded,
            });
        }
        let rent = match rent_exempt {
            Some(rent) => rent,
            None => *rent_exempt.insert(minimum_balance(chain, HEADER_LEN + compressed.len())?),
        };
        let top_up = rent.saturating_sub(lamports(&state));
        let instructions = instructions(
            step,
            program,
            programdata,
            &deployer.pubkey(),
            &compressed,
            top_up,
        );

        match chain.send(&instructions, &[deployer]) {
            Ok(signature) => {
                failures = 0;
                sent(step, signature);
            }
            Err(source) => {
                failures += 1;
                if failures >= MAX_FAILURES {
                    return Err(Error::GaveUp {
                        program: name.into(),
                        failures,
                        source,
                    });
                }
                thread::sleep(backoff * failures);
            }
        }
    }

    Err(Error::NoProgress {
        program: name.into(),
        steps: max_steps,
    })
}

/// From the cluster's rent sysvar, read once a transaction is needed.
fn minimum_balance(chain: &impl Chain, size: usize) -> Result<u64, Error> {
    let account = chain
        .account(&solana_sdk_ids::sysvar::rent::id())?
        .ok_or(Error::Rent)?;
    let rent: Rent = bincode::deserialize(&account.data).map_err(|_| Error::Rent)?;

    Ok(rent.minimum_balance(size))
}

fn lamports(state: &Canonical) -> u64 {
    match state {
        Canonical::Absent { lamports }
        | Canonical::Buffer { lamports, .. }
        | Canonical::Metadata { lamports, .. } => *lamports,
        Canonical::Unknown { .. } => 0,
    }
}

fn optional_pubkey(bytes: &[u8]) -> Option<Pubkey> {
    let key = Pubkey::try_from(bytes).expect("32 bytes");

    (key != Pubkey::default()).then_some(key)
}

fn content(data: &[u8]) -> Option<Vec<u8>> {
    let [encoding, compression, format, data_source] = data[83..87] else {
        unreachable!("four bytes");
    };
    if (encoding, format, data_source) != (ENCODING_UTF8, FORMAT_JSON, DATA_SOURCE_DIRECT) {
        return None;
    }
    let length = u32::from_le_bytes(data[87..91].try_into().expect("4 bytes")) as usize;
    let stored = data.get(HEADER_LEN..HEADER_LEN.checked_add(length)?)?;
    let mut inflated = Vec::new();
    let read = match compression {
        COMPRESSION_NONE => return Some(stored.to_vec()),
        COMPRESSION_GZIP => GzDecoder::new(stored).read_to_end(&mut inflated),
        COMPRESSION_ZLIB => ZlibDecoder::new(stored).read_to_end(&mut inflated),
        _ => return None,
    };

    read.ok().map(|_| inflated)
}

/// The canonical metadata account `upload` leaves for `program` holding `idl`.
#[cfg(test)]
pub(crate) fn uploaded_account(program: &Pubkey, idl: &[u8]) -> Account {
    let stored = compress(idl);
    let mut data = vec![0; HEADER_LEN];
    data[0] = METADATA;
    data[1..33].copy_from_slice(program.as_ref());
    data[65] = 1;
    data[66] = 1;
    data[67..83].copy_from_slice(&SEED);
    data[83..87].copy_from_slice(&[
        ENCODING_UTF8,
        COMPRESSION_ZLIB,
        FORMAT_JSON,
        DATA_SOURCE_DIRECT,
    ]);
    data[87..91].copy_from_slice(&(stored.len() as u32).to_le_bytes());
    data.extend_from_slice(&stored);

    Account {
        lamports: 1,
        data,
        owner: PROGRAM_METADATA_ID,
        ..Account::default()
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::hash::Hash;

    use super::*;
    use crate::transaction;

    const IDL: &[u8] = br#"{"address":"x","metadata":{"name":"portal","version":"1.2.3"}}"#;

    fn program() -> Pubkey {
        Pubkey::new_from_array([7; 32])
    }

    fn deployer() -> Pubkey {
        Pubkey::new_from_array([9; 32])
    }

    fn buffer(written: &[u8]) -> Canonical {
        Canonical::Buffer {
            program: Some(program()),
            authority: Some(deployer()),
            canonical: true,
            seed: SEED,
            written: written.to_vec(),
            lamports: 1,
        }
    }

    fn metadata(content: Option<&[u8]>, mutable: bool) -> Canonical {
        Canonical::Metadata {
            program: program(),
            canonical: true,
            mutable,
            seed: SEED,
            content: content.map(<[u8]>::to_vec),
            lamports: 1,
        }
    }

    fn next_for(state: &Canonical, idl: &[u8]) -> Result<Step, String> {
        next(state, &program(), &deployer(), &compress(idl), idl)
    }

    /// A header as the metadata program writes it, then `data`.
    fn account(discriminator: u8, header: impl Fn(&mut [u8]), data: &[u8]) -> Account {
        let mut bytes = vec![0; HEADER_LEN];
        bytes[0] = discriminator;
        header(&mut bytes);
        bytes.extend_from_slice(data);

        Account {
            lamports: 1,
            data: bytes,
            owner: PROGRAM_METADATA_ID,
            ..Account::default()
        }
    }

    fn metadata_account(compression: u8, stored: &[u8]) -> Account {
        account(
            METADATA,
            |header| {
                header[1..33].copy_from_slice(program().as_ref());
                header[65] = 1;
                header[66] = 1;
                header[67..83].copy_from_slice(&SEED);
                header[83..87].copy_from_slice(&[ENCODING_UTF8, compression, FORMAT_JSON, 0]);
                header[87..91].copy_from_slice(&(stored.len() as u32).to_le_bytes());
            },
            stored,
        )
    }

    #[test]
    fn already_equal_is_done_and_sends_nothing() {
        assert_eq!(next_for(&metadata(Some(IDL), true), IDL), Ok(Step::Done));
        assert_eq!(next_for(&metadata(Some(IDL), false), IDL), Ok(Step::Done));
    }

    #[test]
    fn absent_allocates_first() {
        assert_eq!(
            next_for(&Canonical::Absent { lamports: 0 }, IDL),
            Ok(Step::Allocate)
        );
        assert_eq!(
            next_for(&Canonical::Absent { lamports: 5 }, IDL),
            Ok(Step::Allocate)
        );
    }

    #[test]
    fn a_buffer_resumes_at_the_first_chunk_that_differs() {
        let idl: Vec<u8> = (0..20_000u32)
            .flat_map(|index| index.to_le_bytes())
            .collect();
        let compressed = compress(&idl);
        assert!(compressed.len() > 2 * WRITE_CHUNK, "{}", compressed.len());
        let end = compressed.len().min(3 * WRITE_CHUNK);
        let mut corrupted = compressed[..2 * WRITE_CHUNK].to_vec();
        corrupted[WRITE_CHUNK + 1] ^= 1;

        assert_eq!(
            next_for(&buffer(&[]), &idl),
            Ok(Step::Write {
                start: 0,
                end: WRITE_CHUNK
            })
        );
        assert_eq!(
            next_for(&buffer(&compressed[..WRITE_CHUNK]), &idl),
            Ok(Step::Write {
                start: WRITE_CHUNK,
                end: 2 * WRITE_CHUNK
            })
        );
        assert_eq!(
            next_for(&buffer(&compressed[..WRITE_CHUNK + 10]), &idl),
            Ok(Step::Write {
                start: WRITE_CHUNK,
                end: 2 * WRITE_CHUNK
            })
        );
        assert_eq!(
            next_for(&buffer(&corrupted), &idl),
            Ok(Step::Write {
                start: WRITE_CHUNK,
                end: 2 * WRITE_CHUNK
            })
        );
        assert_eq!(
            next_for(&buffer(&compressed[..2 * WRITE_CHUNK]), &idl),
            Ok(Step::Write {
                start: 2 * WRITE_CHUNK,
                end
            })
        );
        assert_eq!(next_for(&buffer(&compressed), &idl), Ok(Step::Initialize));
    }

    #[test]
    fn a_buffer_longer_than_the_idl_is_closed() {
        let mut longer = compress(IDL);
        longer.push(0);

        assert_eq!(next_for(&buffer(&longer), IDL), Ok(Step::Close));
    }

    #[test]
    fn differing_metadata_is_closed_unless_immutable() {
        let other = br#"{"address":"x","metadata":{"version":"1.2.2"}}"#;

        assert_eq!(next_for(&metadata(Some(other), true), IDL), Ok(Step::Close));
        assert_eq!(next_for(&metadata(None, true), IDL), Ok(Step::Close));
        assert!(next_for(&metadata(Some(other), false), IDL)
            .unwrap_err()
            .contains("immutable"));
    }

    #[test]
    fn foreign_states_are_refused() {
        let mut foreign_authority = buffer(&[]);
        let mut other_program = buffer(&[]);
        let mut not_canonical = metadata(Some(IDL), true);
        if let Canonical::Buffer { authority, .. } = &mut foreign_authority {
            *authority = Some(Pubkey::new_unique());
        }
        if let Canonical::Buffer { program, .. } = &mut other_program {
            *program = Some(Pubkey::new_unique());
        }
        if let Canonical::Metadata { canonical, .. } = &mut not_canonical {
            *canonical = false;
        }

        [
            foreign_authority,
            other_program,
            not_canonical,
            Canonical::Unknown { reason: "x".into() },
        ]
        .iter()
        .for_each(|state| assert!(next_for(state, IDL).is_err(), "{state:?}"));
    }

    #[test]
    fn observe_reads_each_account_kind() {
        let system_owned = Account {
            lamports: 3,
            ..Account::default()
        };
        let written = b"partial".to_vec();
        let buffer_account = account(
            BUFFER,
            |header| {
                header[1..33].copy_from_slice(program().as_ref());
                header[33..65].copy_from_slice(deployer().as_ref());
                header[65] = 1;
                header[66..82].copy_from_slice(&SEED);
            },
            &written,
        );

        assert_eq!(observe(None), Canonical::Absent { lamports: 0 });
        assert_eq!(
            observe(Some(&system_owned)),
            Canonical::Absent { lamports: 3 }
        );
        assert_eq!(observe(Some(&buffer_account)), buffer(&written));
        assert_eq!(
            observe(Some(&metadata_account(COMPRESSION_ZLIB, &compress(IDL)))),
            metadata(Some(IDL), true)
        );
        assert!(matches!(
            observe(Some(&Account {
                owner: Pubkey::new_unique(),
                ..metadata_account(COMPRESSION_ZLIB, &compress(IDL))
            })),
            Canonical::Unknown { .. }
        ));
    }

    #[test]
    fn matches_inflates_whatever_compression_was_used() {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), Compression::best());
        gzip.write_all(IDL).unwrap();
        let gzip = gzip.finish().unwrap();
        let best_zlib = {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(IDL).unwrap();
            encoder.finish().unwrap()
        };

        [
            metadata_account(COMPRESSION_ZLIB, &compress(IDL)),
            metadata_account(COMPRESSION_ZLIB, &best_zlib),
            metadata_account(COMPRESSION_GZIP, &gzip),
            metadata_account(COMPRESSION_NONE, IDL),
        ]
        .iter()
        .for_each(|account| assert!(matches(Some(account), &program(), IDL)));
        assert!(!matches(
            Some(&metadata_account(COMPRESSION_ZLIB, &compress(b"{}"))),
            &program(),
            IDL
        ));
        assert!(!matches(
            Some(&metadata_account(COMPRESSION_ZLIB, b"not zlib")),
            &program(),
            IDL
        ));
        assert!(!matches(None, &program(), IDL));
        assert!(matches(
            Some(&uploaded_account(&program(), IDL)),
            &program(),
            IDL
        ));
        assert!(!matches(
            Some(&metadata_account(COMPRESSION_ZLIB, &compress(IDL))),
            &Pubkey::new_unique(),
            IDL
        ));
    }

    #[test]
    fn a_full_write_with_a_top_up_fits_a_transaction() {
        let deployer = Keypair::new();
        let compressed = vec![0xab; WRITE_CHUNK];
        let instructions = instructions(
            Step::Write {
                start: 0,
                end: WRITE_CHUNK,
            },
            &program(),
            &Pubkey::new_unique(),
            &deployer.pubkey(),
            &compressed,
            1,
        );
        let size = transaction::size(&deployer.pubkey(), &instructions).unwrap();

        assert_eq!(instructions.len(), 2);
        assert!(size <= transaction::MAX_SIZE, "{size}");
        assert!(transaction::signed(&instructions, &[&deployer], Hash::default(), 1).is_ok());
    }

    #[test]
    fn close_never_tops_up_and_returns_the_rent_to_the_deployer() {
        let instructions = instructions(
            Step::Close,
            &program(),
            &Pubkey::new_unique(),
            &deployer(),
            &[],
            100,
        );

        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].data, [CLOSE]);
        assert_eq!(
            instructions[0].accounts[4],
            AccountMeta::new(deployer(), false)
        );
    }
}
