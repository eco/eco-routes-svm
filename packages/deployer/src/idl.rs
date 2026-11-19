//! The release IDLs as each program's canonical Program Metadata account, where Anchor 1.x and
//! explorers read them. Only the program's upgrade authority can create or change that account,
//! so the upload runs in the deploy job, signed by the deployer, and must land before finalize.
//!
//! The upload is a state machine over two accounts, both re-read before every transaction: the
//! canonical PDA and a scratch buffer at the program's `"idl-scratch"` seed. A first upload
//! builds the canonical account in place: absent → allocate (rent transfer + `allocate`) →
//! buffer → `write` each chunk that differs → `initialize`. A canonical IDL that differs is never
//! closed: the new one is written to the scratch buffer and copied over with `set_data`, so the
//! account always holds a whole IDL, then the scratch buffer is closed. A delegated authority in
//! the canonical header is removed, since it would keep the IDL writable after finalize. Every
//! step is checked on-chain against the state it expects, so a stale read only costs a failed
//! transaction, and a run that stops anywhere resumes from what the chain holds: the official
//! client sends chunk writes in parallel and cannot continue a half-written buffer.

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
/// Bytes of IDL each `write` carries; a write with a rent top-up stays within a v1 transaction.
pub const WRITE_CHUNK: usize = 3_600;
/// Both account kinds start with a 96-byte header; the data follows it.
pub const HEADER_LEN: usize = 96;
/// Consecutive failed transactions before the upload gives up; a rerun resumes.
const MAX_FAILURES: u32 = 5;
const SEED_LEN: usize = 16;
/// `"idl"` zero-padded to the program's 16-byte seed; the canonical PDA has no authority seed.
const SEED: [u8; SEED_LEN] = *b"idl\0\0\0\0\0\0\0\0\0\0\0\0\0";
/// Where a replacement IDL is written before `set_data` copies it into the canonical account.
const SCRATCH_SEED: [u8; SEED_LEN] = *b"idl-scratch\0\0\0\0\0";

// Account discriminators.
const BUFFER: u8 = 1;
const METADATA: u8 = 2;
// Instruction discriminators.
const WRITE: u8 = 0;
const INITIALIZE: u8 = 1;
const SET_AUTHORITY: u8 = 2;
const SET_DATA: u8 = 3;
const CLOSE: u8 = 6;
const ALLOCATE: u8 = 7;
const EXTEND: u8 = 8;
/// The runtime's `MAX_PERMITTED_DATA_INCREASE`: how far one instruction may grow an account, so
/// `set_data` cannot grow the canonical account by more and `extend` steps make up the rest.
const MAX_GROWTH: usize = 10_240;
// Header values `initialize` and `set_data` record: UTF-8 JSON, zlib-compressed, in the account.
const ENCODING_UTF8: u8 = 1;
const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_GZIP: u8 = 1;
const COMPRESSION_ZLIB: u8 = 2;
const FORMAT_JSON: u8 = 1;
const DATA_SOURCE_DIRECT: u8 = 0;
const HEADER_VALUES: [u8; 4] = [
    ENCODING_UTF8,
    COMPRESSION_ZLIB,
    FORMAT_JSON,
    DATA_SOURCE_DIRECT,
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error("{program}: IDL {target} account {address}: {refusal}")]
    Refused {
        program: String,
        target: Target,
        address: Pubkey,
        refusal: Refusal,
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
    #[error("the rent sysvar account is missing")]
    RentSysvarMissing,
    #[error("the rent sysvar does not decode: {source}")]
    RentSysvarMalformed { source: bincode::Error },
}

/// A state only a human should resolve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("not this program's account at that seed")]
    NotThisProgram,
    #[error("a buffer held by {authority:?}; only it can close or finish it")]
    ForeignBuffer { authority: Option<Pubkey> },
    #[error("immutable, and it differs from the release IDL")]
    Immutable,
    #[error("{reason}")]
    Unknown { reason: String },
}

/// Which of the program's two Program Metadata accounts a step touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Canonical,
    Scratch,
}

/// What a PDA holds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Canonical {
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
        /// A delegated writer besides the program's upgrade authority.
        authority: Option<Pubkey>,
        canonical: bool,
        mutable: bool,
        seed: [u8; SEED_LEN],
        /// The stored JSON, inflated; `None` unless the header says direct UTF-8 JSON.
        content: Option<Vec<u8>>,
        /// The account's data length, header included.
        size: usize,
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
    Allocate(Target),
    /// Writes `compressed[start..end]` at `start`.
    Write {
        target: Target,
        start: usize,
        end: usize,
    },
    /// Turns the canonical buffer into metadata.
    Initialize,
    /// Grows the canonical metadata by `by` bytes, ahead of a `set_data` that would otherwise
    /// grow it past `MAX_GROWTH`.
    Extend {
        by: u16,
    },
    /// Copies the scratch buffer into the canonical metadata.
    SetData,
    /// Clears a delegated authority from the canonical metadata header.
    RemoveAuthority,
    Close(Target),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The canonical IDL already equalled the file; nothing was sent.
    Unchanged,
    Uploaded,
}

impl fmt::Display for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Canonical => "canonical",
            Self::Scratch => "scratch",
        })
    }
}

impl fmt::Display for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Done => formatter.write_str("done"),
            Self::Allocate(target) => write!(formatter, "allocate {target}"),
            Self::Write { target, start, end } => {
                write!(formatter, "write {target} {start}..{end}")
            }
            Self::Initialize => formatter.write_str("initialize"),
            Self::Extend { by } => write!(formatter, "extend canonical by {by}"),
            Self::SetData => formatter.write_str("set data from scratch"),
            Self::RemoveAuthority => formatter.write_str("remove delegated authority"),
            Self::Close(target) => write!(formatter, "close {target}"),
        }
    }
}

impl Target {
    pub fn address(self, program: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[program.as_ref(), &self.seed()], &PROGRAM_METADATA_ID).0
    }

    fn seed(self) -> [u8; SEED_LEN] {
        match self {
            Self::Canonical => SEED,
            Self::Scratch => SCRATCH_SEED,
        }
    }
}

pub fn canonical_address(program: &Pubkey) -> Pubkey {
    Target::Canonical.address(program)
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

fn observe(account: Option<&Account>) -> Canonical {
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
            authority: optional_pubkey(&data[33..65]),
            canonical: data[66] == 1,
            mutable: data[65] == 1,
            seed: data[67..83].try_into().expect("16 bytes"),
            content: content(data),
            size: data.len(),
            lamports: account.lamports,
        },
        discriminator => Canonical::Unknown {
            reason: format!("account discriminator {discriminator}"),
        },
    }
}

/// Whether `account`, read at `program`'s canonical PDA, holds exactly `idl` and only the
/// upgrade authority can change it: a delegated authority would outlive finalize.
pub fn matches(account: Option<&Account>, program: &Pubkey, idl: &[u8]) -> bool {
    matches!(
        observe(account),
        Canonical::Metadata { program: stored, authority, canonical: true, mutable, seed: SEED, content: Some(content), .. }
            if stored == *program && content == idl && (authority.is_none() || !mutable)
    )
}

/// The next transaction towards `program`'s canonical IDL holding `idl`, deflated to
/// `compressed`; a refusal when an account is in a state only a human should resolve.
fn next(
    canonical: &Canonical,
    scratch: &Canonical,
    program: &Pubkey,
    deployer: &Pubkey,
    compressed: &[u8],
    idl: &[u8],
) -> Result<Step, (Target, Refusal)> {
    let progress = |target: Target, state: &Canonical, complete: Step| {
        buffer_progress(target, state, program, deployer, compressed)
            .map(|step| step.unwrap_or(complete))
            .map_err(|refusal| (target, refusal))
    };

    match canonical {
        Canonical::Absent { .. } => Ok(Step::Allocate(Target::Canonical)),
        Canonical::Unknown { reason } => Err((
            Target::Canonical,
            Refusal::Unknown {
                reason: reason.clone(),
            },
        )),
        Canonical::Buffer { .. } => progress(Target::Canonical, canonical, Step::Initialize),
        Canonical::Metadata {
            program: stored,
            authority,
            canonical: is_canonical,
            mutable,
            seed,
            content,
            size,
            ..
        } => {
            if stored != program || !is_canonical || *seed != SEED {
                return Err((Target::Canonical, Refusal::NotThisProgram));
            }
            if content.as_deref() == Some(idl) {
                return Ok(
                    match (
                        authority,
                        mutable,
                        closable(scratch, Target::Scratch, program),
                    ) {
                        (Some(_), true, _) => Step::RemoveAuthority,
                        (_, _, true) => Step::Close(Target::Scratch),
                        _ => Step::Done,
                    },
                );
            }
            if !mutable {
                return Err((Target::Canonical, Refusal::Immutable));
            }

            match scratch {
                Canonical::Absent { .. } => Ok(Step::Allocate(Target::Scratch)),
                Canonical::Buffer { .. } => {
                    let shortfall = (HEADER_LEN + compressed.len()).saturating_sub(*size);
                    let copy = match shortfall > MAX_GROWTH {
                        true => Step::Extend {
                            by: MAX_GROWTH as u16,
                        },
                        false => Step::SetData,
                    };
                    progress(Target::Scratch, scratch, copy)
                }
                Canonical::Metadata { .. } => Err((Target::Scratch, Refusal::NotThisProgram)),
                Canonical::Unknown { reason } => Err((
                    Target::Scratch,
                    Refusal::Unknown {
                        reason: reason.clone(),
                    },
                )),
            }
        }
    }
}

/// The next write into the deployer's `target` buffer, a close when it holds more than
/// `compressed`, or `None` when it holds exactly `compressed`.
fn buffer_progress(
    target: Target,
    state: &Canonical,
    program: &Pubkey,
    deployer: &Pubkey,
    compressed: &[u8],
) -> Result<Option<Step>, Refusal> {
    let Canonical::Buffer {
        program: stored,
        authority,
        canonical,
        seed,
        written,
        ..
    } = state
    else {
        unreachable!("only called on a buffer");
    };
    if *stored != Some(*program) || !canonical || *seed != target.seed() {
        return Err(Refusal::NotThisProgram);
    }
    if *authority != Some(*deployer) {
        return Err(Refusal::ForeignBuffer {
            authority: *authority,
        });
    }
    // `initialize` and `set_data` take the whole buffer as the data: a longer one holds another
    // IDL.
    if written.len() > compressed.len() {
        return Ok(Some(Step::Close(target)));
    }

    Ok((0..compressed.len())
        .step_by(WRITE_CHUNK)
        .map(|start| (start, (start + WRITE_CHUNK).min(compressed.len())))
        .find(|(start, end)| written.get(*start..*end) != Some(&compressed[*start..*end]))
        .map(|(start, end)| Step::Write { target, start, end }))
}

/// Whether the upgrade authority can close what `target`'s PDA holds for `program`: a buffer at
/// its seed, or mutable metadata.
fn closable(state: &Canonical, target: Target, program: &Pubkey) -> bool {
    match state {
        Canonical::Buffer {
            program: stored,
            canonical: true,
            seed,
            ..
        } => *stored == Some(*program) && *seed == target.seed(),
        Canonical::Metadata {
            program: stored,
            canonical: true,
            mutable: true,
            seed,
            ..
        } => stored == program && *seed == target.seed(),
        _ => false,
    }
}

/// The next close towards removing both of `program`'s IDL accounts, before the program itself
/// is closed: once its program data is gone nobody can authorize closing them.
fn next_close(
    canonical: &Canonical,
    scratch: &Canonical,
    program: &Pubkey,
) -> Result<Step, (Target, Refusal)> {
    [(Target::Scratch, scratch), (Target::Canonical, canonical)]
        .into_iter()
        .find_map(|(target, state)| match state {
            Canonical::Absent { .. } => None,
            _ if closable(state, target, program) => Some(Ok(Step::Close(target))),
            Canonical::Metadata { mutable: false, .. } => Some(Err((target, Refusal::Immutable))),
            _ => Some(Err((target, Refusal::NotThisProgram))),
        })
        .unwrap_or(Ok(Step::Done))
}

/// The instructions of `step`, with a rent top-up of the account it grows first. `programdata`
/// is the program's, which the metadata program checks the authority in.
pub fn instructions(
    step: Step,
    program: &Pubkey,
    programdata: &Pubkey,
    deployer: &Pubkey,
    compressed: &[u8],
    top_up: u64,
) -> Vec<Instruction> {
    let canonical = Target::Canonical.address(program);
    let program_accounts = [
        AccountMeta::new_readonly(*program, false),
        AccountMeta::new_readonly(*programdata, false),
    ];
    let authorized =
        |account: Pubkey, middle: Vec<AccountMeta>, last: Option<AccountMeta>, data: Vec<u8>| {
            Instruction {
                program_id: PROGRAM_METADATA_ID,
                accounts: [
                    vec![
                        AccountMeta::new(account, false),
                        AccountMeta::new_readonly(*deployer, true),
                    ],
                    middle,
                    program_accounts.to_vec(),
                    last.into_iter().collect(),
                ]
                .concat(),
                data,
            }
        };
    let system = Some(AccountMeta::new_readonly(
        solana_sdk_ids::system_program::id(),
        false,
    ));
    let (instruction, grows) = match step {
        Step::Done => return vec![],
        Step::Allocate(target) => (
            authorized(
                target.address(program),
                vec![],
                system,
                [&[ALLOCATE][..], &target.seed()].concat(),
            ),
            Some(target.address(program)),
        ),
        Step::Write { target, start, end } => (
            Instruction {
                program_id: PROGRAM_METADATA_ID,
                accounts: vec![
                    AccountMeta::new(target.address(program), false),
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
            Some(target.address(program)),
        ),
        Step::Initialize => (
            authorized(
                canonical,
                vec![],
                system,
                [&[INITIALIZE][..], &SEED, &HEADER_VALUES].concat(),
            ),
            Some(canonical),
        ),
        Step::SetData => (
            authorized(
                canonical,
                vec![AccountMeta::new_readonly(
                    Target::Scratch.address(program),
                    false,
                )],
                None,
                [&[SET_DATA][..], &HEADER_VALUES].concat(),
            ),
            Some(canonical),
        ),
        Step::Extend { by } => (
            authorized(
                canonical,
                vec![],
                None,
                [&[EXTEND][..], &by.to_le_bytes()].concat(),
            ),
            Some(canonical),
        ),
        Step::RemoveAuthority => (
            authorized(canonical, vec![], None, vec![SET_AUTHORITY, 0]),
            None,
        ),
        // The rent goes back to the deployer.
        Step::Close(target) => (
            authorized(
                target.address(program),
                vec![],
                Some(AccountMeta::new(*deployer, false)),
                vec![CLOSE],
            ),
            None,
        ),
    };
    let top_up = grows
        .filter(|_| top_up > 0)
        .map(|account| solana_system_interface::instruction::transfer(deployer, &account, top_up));

    top_up.into_iter().chain([instruction]).collect()
}

/// Drives `program`'s canonical IDL to `idl`, one transaction at a time, and returns once a read
/// shows the stored data inflating to exactly `idl` with no delegated authority and no scratch
/// buffer left. Reports each transaction through `sent`. A failed transaction is retried after
/// `backoff` from a fresh read; the upload gives up after `MAX_FAILURES` in a row.
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
    let compressed = compress(idl);
    let size = HEADER_LEN + compressed.len();
    // Allocate, every chunk and every extend, initialize or set data, removing an authority and
    // closing a stale buffer, for each of the two accounts, each allowed one duplicate from a
    // stale read, plus the retries.
    let max_steps = 2
        * (2 * compressed.len().div_ceil(WRITE_CHUNK) + size.div_ceil(MAX_GROWTH) + 6)
        + MAX_FAILURES as usize;

    drive(
        chain,
        name,
        program,
        programdata,
        deployer,
        &compressed,
        size,
        max_steps,
        backoff,
        sent,
        |canonical, scratch| {
            next(
                canonical,
                scratch,
                program,
                &deployer.pubkey(),
                &compressed,
                idl,
            )
        },
    )
}

/// Closes `program`'s canonical IDL and scratch buffer, returning their rent to the deployer.
/// It must run while the deployer is still the program's upgrade authority, before the program
/// itself is closed.
pub fn close(
    chain: &mut impl Chain,
    name: &str,
    program: &Pubkey,
    programdata: &Pubkey,
    deployer: &Keypair,
    backoff: Duration,
    sent: &mut impl FnMut(Step, Signature),
) -> Result<Outcome, Error> {
    drive(
        chain,
        name,
        program,
        programdata,
        deployer,
        &[],
        0,
        4 + MAX_FAILURES as usize,
        backoff,
        sent,
        |canonical, scratch| next_close(canonical, scratch, program),
    )
}

/// Reads both accounts, sends the step `next` picks with any top-up the account it grows needs
/// to hold `size` bytes, and repeats until `next` says `Done` or `max_steps` transactions went
/// out. The last iteration only reads, so a final step that completes the job still succeeds.
#[allow(clippy::too_many_arguments)]
fn drive(
    chain: &mut impl Chain,
    name: &str,
    program: &Pubkey,
    programdata: &Pubkey,
    deployer: &Keypair,
    compressed: &[u8],
    size: usize,
    max_steps: usize,
    backoff: Duration,
    sent: &mut impl FnMut(Step, Signature),
    next: impl Fn(&Canonical, &Canonical) -> Result<Step, (Target, Refusal)>,
) -> Result<Outcome, Error> {
    let mut rent_exempt = None;
    let mut failures = 0;

    for steps in 0..=max_steps {
        let canonical = observe(chain.account(&Target::Canonical.address(program))?.as_ref());
        let scratch = observe(chain.account(&Target::Scratch.address(program))?.as_ref());
        let step = next(&canonical, &scratch).map_err(|(target, refusal)| Error::Refused {
            program: name.into(),
            target,
            address: target.address(program),
            refusal,
        })?;
        if step == Step::Done {
            return Ok(match steps {
                0 => Outcome::Unchanged,
                _ => Outcome::Uploaded,
            });
        }
        if steps == max_steps {
            break;
        }
        let grown = match step {
            Step::Allocate(target) | Step::Write { target, .. } => Some(target),
            Step::Initialize | Step::Extend { .. } | Step::SetData => Some(Target::Canonical),
            _ => None,
        };
        let top_up = match grown {
            Some(target) => {
                let rent = match rent_exempt {
                    Some(rent) => rent,
                    None => *rent_exempt.insert(minimum_balance(chain, size)?),
                };
                let state = match target {
                    Target::Canonical => &canonical,
                    Target::Scratch => &scratch,
                };
                rent.saturating_sub(lamports(state))
            }
            None => 0,
        };
        let instructions = instructions(
            step,
            program,
            programdata,
            &deployer.pubkey(),
            compressed,
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

/// From the cluster's rent sysvar, read once a transaction needs it.
fn minimum_balance(chain: &impl Chain, size: usize) -> Result<u64, Error> {
    let account = chain
        .account(&solana_sdk_ids::sysvar::rent::id())?
        .ok_or(Error::RentSysvarMissing)?;
    let rent: Rent = bincode::deserialize(&account.data)
        .map_err(|source| Error::RentSysvarMalformed { source })?;

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

/// What uploading `idl` for `program` takes from the deployer at most, besides fees: the rent the
/// canonical account still lacks, plus the scratch buffer's while a replacement is written (it
/// comes back when the buffer closes), and how many transactions it sends. Nothing when the
/// canonical IDL already matches.
pub fn cost(
    chain: &impl Chain,
    program: &Pubkey,
    idl: &[u8],
    rent: &Rent,
) -> Result<(u64, u64), chain::Error> {
    let account = chain.account(&canonical_address(program))?;
    if matches(account.as_ref(), program, idl) {
        return Ok((0, 0));
    }
    let size = HEADER_LEN + compress(idl).len();
    let chunks = size.div_ceil(WRITE_CHUNK) as u64;
    let state = observe(account.as_ref());
    let lacking = rent.minimum_balance(size).saturating_sub(lamports(&state));

    Ok(match state {
        // Allocate the scratch buffer, its chunks, every extend, set data, remove a delegated
        // authority and close the buffer.
        Canonical::Metadata { .. } => (
            lacking + rent.minimum_balance(size),
            chunks + size.div_ceil(MAX_GROWTH) as u64 + 4,
        ),
        // Allocate, every chunk, initialize, and closing a longer buffer left behind.
        _ => (lacking, chunks + 3),
    })
}

/// Lamports already held at `program`'s two IDL accounts: what closing them returns, and what an
/// upload need not pay again.
pub fn held_lamports(chain: &impl Chain, program: &Pubkey) -> Result<[u64; 2], chain::Error> {
    let held = |target: Target| -> Result<u64, chain::Error> {
        Ok(chain
            .account(&target.address(program))?
            .map_or(0, |account| account.lamports))
    };

    Ok([held(Target::Canonical)?, held(Target::Scratch)?])
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
    data[83..87].copy_from_slice(&HEADER_VALUES);
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
    const ABSENT: Canonical = Canonical::Absent { lamports: 0 };

    fn program() -> Pubkey {
        Pubkey::new_from_array([7; 32])
    }

    fn deployer() -> Pubkey {
        Pubkey::new_from_array([9; 32])
    }

    fn buffer_at(target: Target, written: &[u8]) -> Canonical {
        Canonical::Buffer {
            program: Some(program()),
            authority: Some(deployer()),
            canonical: true,
            seed: target.seed(),
            written: written.to_vec(),
            lamports: 1,
        }
    }

    fn buffer(written: &[u8]) -> Canonical {
        buffer_at(Target::Canonical, written)
    }

    fn metadata(content: Option<&[u8]>, mutable: bool) -> Canonical {
        Canonical::Metadata {
            program: program(),
            authority: None,
            canonical: true,
            mutable,
            seed: SEED,
            content: content.map(<[u8]>::to_vec),
            size: HEADER_LEN + content.map_or(0, |content| compress(content).len()),
            lamports: 1,
        }
    }

    fn next_for(
        canonical: &Canonical,
        scratch: &Canonical,
        idl: &[u8],
    ) -> Result<Step, (Target, Refusal)> {
        next(
            canonical,
            scratch,
            &program(),
            &deployer(),
            &compress(idl),
            idl,
        )
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
        assert_eq!(
            next_for(&metadata(Some(IDL), true), &ABSENT, IDL),
            Ok(Step::Done)
        );
        assert_eq!(
            next_for(&metadata(Some(IDL), false), &ABSENT, IDL),
            Ok(Step::Done)
        );
    }

    #[test]
    fn absent_allocates_the_canonical_account_first() {
        assert_eq!(
            next_for(&ABSENT, &ABSENT, IDL),
            Ok(Step::Allocate(Target::Canonical))
        );
        assert_eq!(
            next_for(&Canonical::Absent { lamports: 5 }, &ABSENT, IDL),
            Ok(Step::Allocate(Target::Canonical))
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
        let write = |start, end| {
            Ok(Step::Write {
                target: Target::Canonical,
                start,
                end,
            })
        };

        assert_eq!(next_for(&buffer(&[]), &ABSENT, &idl), write(0, WRITE_CHUNK));
        assert_eq!(
            next_for(&buffer(&compressed[..WRITE_CHUNK]), &ABSENT, &idl),
            write(WRITE_CHUNK, 2 * WRITE_CHUNK)
        );
        assert_eq!(
            next_for(&buffer(&compressed[..WRITE_CHUNK + 10]), &ABSENT, &idl),
            write(WRITE_CHUNK, 2 * WRITE_CHUNK)
        );
        assert_eq!(
            next_for(&buffer(&corrupted), &ABSENT, &idl),
            write(WRITE_CHUNK, 2 * WRITE_CHUNK)
        );
        assert_eq!(
            next_for(&buffer(&compressed[..2 * WRITE_CHUNK]), &ABSENT, &idl),
            write(2 * WRITE_CHUNK, end)
        );
        assert_eq!(
            next_for(&buffer(&compressed), &ABSENT, &idl),
            Ok(Step::Initialize)
        );
    }

    #[test]
    fn a_buffer_longer_than_the_idl_is_closed() {
        let mut longer = compress(IDL);
        longer.push(0);

        assert_eq!(
            next_for(&buffer(&longer), &ABSENT, IDL),
            Ok(Step::Close(Target::Canonical))
        );
        assert_eq!(
            next_for(
                &metadata(Some(b"{}"), true),
                &buffer_at(Target::Scratch, &longer),
                IDL
            ),
            Ok(Step::Close(Target::Scratch))
        );
    }

    /// The canonical account is never closed to replace it: the new IDL goes through the scratch
    /// buffer and `set_data`.
    #[test]
    fn differing_metadata_is_replaced_through_the_scratch_buffer() {
        let older = metadata(
            Some(br#"{"address":"x","metadata":{"version":"1.2.2"}}"#),
            true,
        );
        let compressed = compress(IDL);

        assert_eq!(
            next_for(&older, &ABSENT, IDL),
            Ok(Step::Allocate(Target::Scratch))
        );
        assert_eq!(
            next_for(&metadata(None, true), &ABSENT, IDL),
            Ok(Step::Allocate(Target::Scratch))
        );
        assert_eq!(
            next_for(&older, &buffer_at(Target::Scratch, &[]), IDL),
            Ok(Step::Write {
                target: Target::Scratch,
                start: 0,
                end: compressed.len()
            })
        );
        assert_eq!(
            next_for(&older, &buffer_at(Target::Scratch, &compressed), IDL),
            Ok(Step::SetData)
        );
        assert_eq!(
            next_for(
                &metadata(Some(IDL), true),
                &buffer_at(Target::Scratch, &compressed),
                IDL
            ),
            Ok(Step::Close(Target::Scratch))
        );
    }

    #[test]
    fn a_canonical_account_short_by_more_than_one_instruction_can_grow_is_extended_first() {
        let idl = vec![b'x'; 1];
        let compressed: Vec<u8> = (0..3 * MAX_GROWTH).map(|index| index as u8).collect();
        let mut small = metadata(Some(b"{}"), true);
        let set_size = |state: &mut Canonical, value| {
            if let Canonical::Metadata { size, .. } = state {
                *size = value;
            }
        };
        let scratch = buffer_at(Target::Scratch, &compressed);
        let step =
            |state: &Canonical| next(state, &scratch, &program(), &deployer(), &compressed, &idl);

        assert_eq!(
            step(&small),
            Ok(Step::Extend {
                by: MAX_GROWTH as u16
            })
        );
        set_size(&mut small, HEADER_LEN + compressed.len() - MAX_GROWTH);
        assert_eq!(step(&small), Ok(Step::SetData));
    }

    #[test]
    fn a_delegated_authority_is_removed_unless_immutable() {
        let mut delegated = metadata(Some(IDL), true);
        if let Canonical::Metadata { authority, .. } = &mut delegated {
            *authority = Some(Pubkey::new_unique());
        }
        let mut immutable = delegated.clone();
        if let Canonical::Metadata { mutable, .. } = &mut immutable {
            *mutable = false;
        }

        assert_eq!(
            next_for(&delegated, &ABSENT, IDL),
            Ok(Step::RemoveAuthority)
        );
        assert_eq!(next_for(&immutable, &ABSENT, IDL), Ok(Step::Done));
    }

    #[test]
    fn foreign_states_are_refused() {
        let other = br#"{"address":"x","metadata":{"version":"1.2.2"}}"#;
        let mut foreign_authority = buffer(&[]);
        let mut other_program = buffer(&[]);
        let mut not_canonical = metadata(Some(IDL), true);
        let foreign = Pubkey::new_unique();
        if let Canonical::Buffer { authority, .. } = &mut foreign_authority {
            *authority = Some(foreign);
        }
        if let Canonical::Buffer { program, .. } = &mut other_program {
            *program = Some(Pubkey::new_unique());
        }
        if let Canonical::Metadata { canonical, .. } = &mut not_canonical {
            *canonical = false;
        }
        let unknown = Canonical::Unknown { reason: "x".into() };
        let differing = metadata(Some(other), true);

        assert_eq!(
            next_for(&foreign_authority, &ABSENT, IDL),
            Err((
                Target::Canonical,
                Refusal::ForeignBuffer {
                    authority: Some(foreign)
                }
            ))
        );
        assert_eq!(
            next_for(&other_program, &ABSENT, IDL),
            Err((Target::Canonical, Refusal::NotThisProgram))
        );
        assert_eq!(
            next_for(&not_canonical, &ABSENT, IDL),
            Err((Target::Canonical, Refusal::NotThisProgram))
        );
        assert_eq!(
            next_for(&unknown, &ABSENT, IDL),
            Err((Target::Canonical, Refusal::Unknown { reason: "x".into() }))
        );
        assert_eq!(
            next_for(&metadata(Some(other), false), &ABSENT, IDL),
            Err((Target::Canonical, Refusal::Immutable))
        );
        // A canonical-seed buffer in the scratch slot is not the scratch buffer.
        assert_eq!(
            next_for(&differing, &buffer(&[]), IDL),
            Err((Target::Scratch, Refusal::NotThisProgram))
        );
        assert_eq!(
            next_for(&differing, &unknown, IDL),
            Err((Target::Scratch, Refusal::Unknown { reason: "x".into() }))
        );
    }

    #[test]
    fn closing_takes_the_scratch_buffer_then_the_canonical_account() {
        let canonical = metadata(Some(IDL), true);
        let scratch = buffer_at(Target::Scratch, b"partial");

        assert_eq!(
            next_close(&canonical, &scratch, &program()),
            Ok(Step::Close(Target::Scratch))
        );
        assert_eq!(
            next_close(&canonical, &ABSENT, &program()),
            Ok(Step::Close(Target::Canonical))
        );
        assert_eq!(
            next_close(&buffer(b"partial"), &ABSENT, &program()),
            Ok(Step::Close(Target::Canonical))
        );
        assert_eq!(next_close(&ABSENT, &ABSENT, &program()), Ok(Step::Done));
        assert_eq!(
            next_close(&metadata(Some(IDL), false), &ABSENT, &program()),
            Err((Target::Canonical, Refusal::Immutable))
        );
    }

    #[test]
    fn observe_reads_each_account_kind() {
        let system_owned = Account {
            lamports: 3,
            ..Account::default()
        };
        let written = b"partial".to_vec();
        let delegate = Pubkey::new_unique();
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
        let mut delegated = metadata_account(COMPRESSION_ZLIB, &compress(IDL));
        delegated.data[33..65].copy_from_slice(delegate.as_ref());

        assert_eq!(observe(None), ABSENT);
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
            observe(Some(&delegated)),
            Canonical::Metadata { authority: Some(authority), .. } if authority == delegate
        ));
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

    /// A delegated writer could still change the IDL after finalize, so it does not count as
    /// uploaded until it is removed, unless the account is immutable.
    #[test]
    fn matches_refuses_a_delegated_authority_on_a_mutable_idl() {
        let mut delegated = uploaded_account(&program(), IDL);
        delegated.data[33..65].copy_from_slice(Pubkey::new_unique().as_ref());
        let mut immutable = delegated.clone();
        immutable.data[65] = 0;

        assert!(!matches(Some(&delegated), &program(), IDL));
        assert!(matches(Some(&immutable), &program(), IDL));
    }

    #[test]
    fn a_full_write_with_a_top_up_fits_a_transaction() {
        let deployer = Keypair::new();
        let compressed = vec![0xab; WRITE_CHUNK];
        let instructions = instructions(
            Step::Write {
                target: Target::Scratch,
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
        assert_eq!(
            instructions[1].accounts[0].pubkey,
            Target::Scratch.address(&program())
        );
        assert!(size <= transaction::MAX_SIZE, "{size}");
        assert!(transaction::signed(&instructions, &[&deployer], Hash::default(), 1).is_ok());
    }

    #[test]
    fn close_never_tops_up_and_returns_the_rent_to_the_deployer() {
        let instructions = instructions(
            Step::Close(Target::Scratch),
            &program(),
            &Pubkey::new_unique(),
            &deployer(),
            &[],
            100,
        );

        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].data, [CLOSE]);
        assert_eq!(
            instructions[0].accounts[0].pubkey,
            Target::Scratch.address(&program())
        );
        assert_eq!(
            instructions[0].accounts[4],
            AccountMeta::new(deployer(), false)
        );
    }

    /// `set_data` takes `[metadata, authority, buffer, program, program_data]` and only the
    /// three header bytes plus the data source when the data comes from a buffer.
    #[test]
    fn set_data_copies_from_the_scratch_buffer_into_the_canonical_account() {
        let programdata = Pubkey::new_unique();
        let instructions =
            instructions(Step::SetData, &program(), &programdata, &deployer(), &[], 7);
        let keys: Vec<_> = instructions[1]
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect();

        assert_eq!(
            instructions[0],
            solana_system_interface::instruction::transfer(
                &deployer(),
                &canonical_address(&program()),
                7
            )
        );
        assert_eq!(
            keys,
            [
                canonical_address(&program()),
                deployer(),
                Target::Scratch.address(&program()),
                program(),
                programdata
            ]
        );
        assert_eq!(
            instructions[1].data,
            [
                SET_DATA,
                ENCODING_UTF8,
                COMPRESSION_ZLIB,
                FORMAT_JSON,
                DATA_SOURCE_DIRECT
            ]
        );
    }
}
