use solana_sdk::pubkey::Pubkey;

use crate::config::Configs;
use crate::layerzero_state::AltFault;
use crate::plan::LAYERZERO_PROVER;

/// A part of the LayerZero setup that is not on chain, or not usable.
#[derive(Debug, thiserror::Error)]
pub enum Incomplete {
    #[error("{LAYERZERO_PROVER}: the store is absent")]
    StoreAbsent,
    #[error("{LAYERZERO_PROVER}: no lookup table is recorded in the store")]
    AltNotSet,
    #[error("{LAYERZERO_PROVER}: lookup table {alt} is missing or not a lookup table")]
    AltInvalid { alt: Pubkey },
    #[error("{LAYERZERO_PROVER}: lookup table {alt} is not frozen")]
    AltUnfrozen { alt: Pubkey },
    #[error("{LAYERZERO_PROVER}: lookup table {alt} is deactivated")]
    AltDeactivated { alt: Pubkey },
    #[error("{LAYERZERO_PROVER}: lookup table {alt} lacks {missing}")]
    AltIncomplete { alt: Pubkey, missing: Pubkey },
    #[error("{LAYERZERO_PROVER}: eid {eid} path is missing {account}")]
    PathMissing { eid: u32, account: Pubkey },
}

/// Every peer's path accounts and the lookup table exist and the table is usable. Says nothing
/// about their contents: compare those with [`Configs::deviation`] and
/// [`Configs::paths_conflict`].
pub fn check(live: &Configs) -> Result<(), Incomplete> {
    if live.layerzero_peers.is_none() {
        return Err(Incomplete::StoreAbsent);
    }
    live.layerzero_paths.iter().try_for_each(|path| {
        let missing = [
            (path.nonce, path.accounts.nonce),
            (path.send.is_some(), path.accounts.send_config),
            (path.receive.is_some(), path.accounts.receive_config),
        ]
        .into_iter()
        .find_map(|(exists, account)| (!exists).then_some(account));

        match missing {
            Some(account) => Err(Incomplete::PathMissing {
                eid: path.eid,
                account,
            }),
            None => Ok(()),
        }
    })?;
    let alt = live.layerzero_alt.as_ref().ok_or(Incomplete::AltNotSet)?;
    let address = alt.address;

    match &alt.fault {
        None => Ok(()),
        Some(AltFault::Invalid) => Err(Incomplete::AltInvalid { alt: address }),
        Some(AltFault::Unfrozen) => Err(Incomplete::AltUnfrozen { alt: address }),
        Some(AltFault::Deactivated) => Err(Incomplete::AltDeactivated { alt: address }),
        Some(AltFault::Incomplete { missing }) => Err(Incomplete::AltIncomplete {
            alt: address,
            missing: *missing,
        }),
    }
}
