//! Decoders for the per-OApp accounts of LayerZero's ULN302 program, mirrored from
//! LayerZero-v2@9c741e7f, `packages/layerzero-v2/solana/programs/programs/uln/src/state/uln.rs`:
//! Anchor accounts `SendConfig { bump, uln: UlnConfig, executor: ExecutorConfig }` and
//! `ReceiveConfig { bump, uln: UlnConfig }`, allocated at their maximum size (trailing zeros).

use anchor_lang::AnchorDeserialize;
use layerzero_prover::layerzero::{ExecutorConfig, UlnConfig, ULN_ID};
use solana_sdk::pubkey::Pubkey;
use solana_sdk_ids::system_program;

use crate::chain::{self, Chain};

const DISCRIMINATOR_LEN: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error("ULN config {address} is owned by {owner}")]
    ForeignOwner { address: Pubkey, owner: Pubkey },
    #[error("ULN config {address} does not deserialize: {reason}")]
    Malformed { address: Pubkey, reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SendConfig {
    pub uln: UlnConfig,
    pub executor: ExecutorConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiveConfig {
    pub uln: UlnConfig,
}

#[derive(AnchorDeserialize)]
struct SendConfigAccount {
    _bump: u8,
    uln: UlnConfig,
    executor: ExecutorConfig,
}

#[derive(AnchorDeserialize)]
struct ReceiveConfigAccount {
    _bump: u8,
    uln: UlnConfig,
}

/// `None` when the account does not exist; a pre-funded, never-initialized PDA is system-owned
/// and still absent.
pub fn read_send(chain: &impl Chain, address: &Pubkey) -> Result<Option<SendConfig>, Error> {
    let account: Option<SendConfigAccount> = read(chain, address, "SendConfig")?;

    Ok(account.map(|account| SendConfig {
        uln: account.uln,
        executor: account.executor,
    }))
}

pub fn read_receive(chain: &impl Chain, address: &Pubkey) -> Result<Option<ReceiveConfig>, Error> {
    let account: Option<ReceiveConfigAccount> = read(chain, address, "ReceiveConfig")?;

    Ok(account.map(|account| ReceiveConfig { uln: account.uln }))
}

fn read<T: AnchorDeserialize>(
    chain: &impl Chain,
    address: &Pubkey,
    name: &str,
) -> Result<Option<T>, Error> {
    let Some(account) = chain.account(address)? else {
        return Ok(None);
    };
    if account.owner == system_program::id() && account.data.is_empty() {
        return Ok(None);
    }
    if account.owner != ULN_ID {
        return Err(Error::ForeignOwner {
            address: *address,
            owner: account.owner,
        });
    }
    let malformed = |reason: String| Error::Malformed {
        address: *address,
        reason,
    };
    let (discriminator, mut body) = account
        .data
        .split_at_checked(DISCRIMINATOR_LEN)
        .ok_or_else(|| malformed("shorter than a discriminator".into()))?;
    if discriminator != discriminator_of(name) {
        return Err(malformed(format!("not a {name} account")));
    }

    T::deserialize(&mut body)
        .map(Some)
        .map_err(|error| malformed(error.to_string()))
}

fn discriminator_of(name: &str) -> [u8; DISCRIMINATOR_LEN] {
    solana_sha256_hasher::hash(format!("account:{name}").as_bytes()).to_bytes()[..DISCRIMINATOR_LEN]
        .try_into()
        .expect("a hash is longer than a discriminator")
}
