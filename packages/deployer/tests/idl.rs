//! The canonical IDL upload against the Program Metadata program itself.

pub mod common;

use std::time::Duration;

use common::litesvm_chain::LitesvmChain;
use common::Context;
use deployer::idl::{self, Error, Outcome, Step, PROGRAM_METADATA_ID, WRITE_CHUNK};
use deployer::Chain;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

/// The mainnet deployment of `ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S` (last deployed in
/// slot 431987340), dumped with `solana program dump`; sha256
/// 3d0c0f7504c01b1f6996d30a5c3dc2cccb8b65dedba80d6fceb60a7cada093e0.
const PROGRAM_METADATA_BIN: &[u8] = include_bytes!("fixtures/program_metadata.so");
const DEPLOYER_LAMPORTS: u64 = 10_000_000_000;
const PROGRAM: Pubkey = hyper_prover::ID;

struct Env {
    context: Context,
    deployer: Keypair,
}

impl Env {
    /// The hyper prover deployed with the deployer as its upgrade authority.
    fn new() -> Self {
        let mut context = Context::default();
        context
            .add_program(PROGRAM_METADATA_ID, PROGRAM_METADATA_BIN)
            .unwrap();
        let deployer = Keypair::new();
        context
            .airdrop(&deployer.pubkey(), DEPLOYER_LAMPORTS)
            .unwrap();
        context.set_upgrade_authority(&PROGRAM, Some(deployer.pubkey()));

        Self { context, deployer }
    }

    fn programdata(&self) -> Pubkey {
        let loader = self.context.get_account(&PROGRAM).unwrap().owner;

        Pubkey::find_program_address(&[PROGRAM.as_ref()], &loader).0
    }

    /// Runs the upload and returns its outcome and the steps it sent.
    fn upload(&mut self, idl: &[u8]) -> Result<(Outcome, Vec<Step>), Error> {
        let programdata = self.programdata();
        let mut steps = Vec::new();
        let outcome = idl::upload(
            &mut LitesvmChain::new(&mut self.context),
            "hyper_prover",
            &PROGRAM,
            &programdata,
            &self.deployer,
            idl,
            Duration::ZERO,
            &mut |step, _| steps.push(step),
        )?;

        Ok((outcome, steps))
    }

    /// Sends `step` alone, as a run that stopped right after it would have.
    fn send(&mut self, step: Step, idl: &[u8], top_up: u64) {
        let instructions = idl::instructions(
            step,
            &PROGRAM,
            &self.programdata(),
            &self.deployer.pubkey(),
            &idl::compress(idl),
            top_up,
        );

        LitesvmChain::new(&mut self.context)
            .send(&instructions, &[&self.deployer])
            .unwrap();
    }

    fn holds(&self, idl: &[u8]) -> bool {
        let account = self.context.get_account(&idl::canonical_address(&PROGRAM));

        idl::matches(account.as_ref(), &PROGRAM, idl)
    }

    fn balance(&self) -> u64 {
        self.context.get_balance(&self.deployer.pubkey()).unwrap()
    }
}

/// JSON that deflates to more than `chunks` writes.
fn large_idl(chunks: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let words: Vec<String> = (0..chunks * WRITE_CHUNK / 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            format!("\"{:016x}\"", state)
        })
        .collect();

    format!(
        r#"{{"address":"{PROGRAM}","metadata":{{"name":"hyper_prover","version":"1.0.0"}},"docs":[{}]}}"#,
        words.join(",")
    )
    .into_bytes()
}

fn small_idl(version: &str) -> Vec<u8> {
    format!(
        r#"{{"address":"{PROGRAM}","metadata":{{"name":"hyper_prover","version":"{version}"}}}}"#
    )
    .into_bytes()
}

#[test]
fn an_absent_idl_is_uploaded_then_a_rerun_sends_nothing() {
    let mut env = Env::new();
    let idl = large_idl(3);
    let chunks = idl::compress(&idl).len().div_ceil(WRITE_CHUNK);
    assert!(chunks >= 3, "{chunks}");
    let before = env.balance();

    let (outcome, steps) = env.upload(&idl).unwrap();

    assert_eq!(outcome, Outcome::Uploaded);
    assert_eq!(steps.first(), Some(&Step::Allocate));
    assert_eq!(steps.last(), Some(&Step::Initialize));
    assert_eq!(steps.len(), chunks + 2, "{steps:?}");
    assert!(env.holds(&idl));
    assert!(!env.holds(&small_idl("1.0.0")));
    let paid = before - env.balance();
    let canonical = env
        .context
        .get_account(&idl::canonical_address(&PROGRAM))
        .unwrap();
    assert_eq!(
        canonical.lamports,
        env.context
            .minimum_balance_for_rent_exemption(canonical.data.len())
    );
    assert!(paid >= canonical.lamports, "{paid}");

    let before = env.balance();
    let (rerun, steps) = env.upload(&idl).unwrap();

    assert_eq!(rerun, Outcome::Unchanged);
    assert!(steps.is_empty(), "{steps:?}");
    assert_eq!(env.balance(), before);
}

#[test]
fn a_half_written_buffer_is_resumed_not_restarted() {
    let mut env = Env::new();
    let idl = large_idl(3);
    let compressed = idl::compress(&idl);
    let rent = env
        .context
        .minimum_balance_for_rent_exemption(96 + compressed.len());
    env.send(Step::Allocate, &idl, rent);
    env.send(
        Step::Write {
            start: 0,
            end: WRITE_CHUNK,
        },
        &idl,
        0,
    );

    let (outcome, steps) = env.upload(&idl).unwrap();

    assert_eq!(outcome, Outcome::Uploaded);
    assert_eq!(
        steps.first(),
        Some(&Step::Write {
            start: WRITE_CHUNK,
            end: 2 * WRITE_CHUNK
        })
    );
    assert!(!steps.contains(&Step::Allocate), "{steps:?}");
    assert!(env.holds(&idl));
}

#[test]
fn an_underfunded_buffer_is_topped_up_while_resuming() {
    let mut env = Env::new();
    let idl = large_idl(2);
    // Funded only for its header, as a tool that grows the rent chunk by chunk leaves it.
    let header_rent = env.context.minimum_balance_for_rent_exemption(96);
    env.send(Step::Allocate, &idl, header_rent);

    let (outcome, steps) = env.upload(&idl).unwrap();

    assert_eq!(outcome, Outcome::Uploaded);
    assert!(!steps.contains(&Step::Allocate), "{steps:?}");
    assert!(env.holds(&idl));
}

#[test]
fn an_idl_of_another_release_is_closed_and_replaced() {
    let mut env = Env::new();
    env.upload(&small_idl("1.0.0")).unwrap();
    // The allocate below would otherwise repeat the first upload's signature.
    env.context.expire_blockhash();
    let new = small_idl("1.1.0");

    let (outcome, steps) = env.upload(&new).unwrap();

    assert_eq!(outcome, Outcome::Uploaded);
    assert_eq!(
        steps,
        [
            Step::Close,
            Step::Allocate,
            Step::Write {
                start: 0,
                end: idl::compress(&new).len()
            },
            Step::Initialize
        ]
    );
    assert!(env.holds(&new));
    assert!(!env.holds(&small_idl("1.0.0")));
}

#[test]
fn a_buffer_holding_a_longer_idl_is_closed_and_rebuilt() {
    let mut env = Env::new();
    let longer = large_idl(2);
    let rent = env
        .context
        .minimum_balance_for_rent_exemption(96 + idl::compress(&longer).len());
    env.send(Step::Allocate, &longer, rent);
    env.send(
        Step::Write {
            start: 0,
            end: idl::compress(&longer).len().min(2 * WRITE_CHUNK),
        },
        &longer,
        0,
    );
    let idl = small_idl("1.0.0");

    let (_, steps) = env.upload(&idl).unwrap();

    assert_eq!(steps[..2], [Step::Close, Step::Allocate]);
    assert!(env.holds(&idl));
}

#[test]
fn only_the_upgrade_authority_can_write_the_canonical_idl() {
    let mut env = Env::new();
    env.context
        .set_upgrade_authority(&PROGRAM, Some(Pubkey::new_unique()));
    let before = env.balance();

    let error = env.upload(&small_idl("1.0.0")).unwrap_err();

    assert!(
        matches!(error, Error::GaveUp { ref program, failures: 5, .. } if program == "hyper_prover"),
        "{error:?}"
    );
    assert!(env
        .context
        .get_account(&idl::canonical_address(&PROGRAM))
        .is_none());
    // Every attempt failed as a whole, so it paid at most the fees.
    assert!(before - env.balance() < 1_000_000);
}
