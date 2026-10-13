//! Hand mirror of Circle's CCTP V2 `message_transmitter_v2`
//! (circlefin/solana-cctp-contracts@ec16e95d, `programs/v2/message-transmitter-v2`):
//! the IDs, PDAs, discriminators and argument layouts this program and its
//! clients need. Circle's crate is not a dependency, the same way
//! `hyperlane.rs` mirrors the mailbox.
use anchor_lang::prelude::*;

/// Identical on mainnet and devnet, so no `mainnet` feature gate.
pub const MESSAGE_TRANSMITTER_V2_ID: Pubkey =
    pubkey!("CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC");
/// Solana's CCTP domain (`MessageTransmitter.local_domain` on both clusters).
pub const SOLANA_DOMAIN: u32 = 5;
