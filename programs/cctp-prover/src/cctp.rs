//! Hand mirror of Circle's CCTP V2 `message_transmitter_v2`
//! (circlefin/solana-cctp-contracts@ec16e95d, `programs/v2/message-transmitter-v2`):
//! the IDs, PDAs, discriminators and argument layouts this program and its
//! clients need. Circle's crate is not a dependency, the same way
//! `hyperlane.rs` mirrors the mailbox.
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::system_program;

use crate::instructions::CctpProverError;
use crate::state::{sender_authority_pda, Config};

/// Identical on mainnet and devnet, so no `mainnet` feature gate.
pub const MESSAGE_TRANSMITTER_V2_ID: Pubkey =
    pubkey!("CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC");
/// Solana's CCTP domain (`MessageTransmitter.local_domain` on both clusters).
pub const SOLANA_DOMAIN: u32 = 5;
/// Always fast (spec D2): Iris attests at 1000 where the source supports it,
/// and at 2000 everywhere else.
pub const MIN_FINALITY_THRESHOLD: u32 = 1000;
/// `MessageTransmitter.version` on both clusters.
pub const MESSAGE_VERSION: u32 = 1;
/// `reclaim_event_account` refuses event accounts younger than this.
pub const EVENT_ACCOUNT_WINDOW_SECONDS: i64 = 60 * 60 * 24 * 5;

// Header byte offsets (`message.rs`); every integer is big-endian.
pub const NONCE_INDEX: usize = 12;
pub const SENDER_INDEX: usize = 44;
pub const RECIPIENT_INDEX: usize = 76;
pub const DESTINATION_CALLER_INDEX: usize = 108;
pub const MIN_FINALITY_THRESHOLD_INDEX: usize = 140;
pub const FINALITY_THRESHOLD_EXECUTED_INDEX: usize = 144;
pub const MESSAGE_BODY_INDEX: usize = 148;

pub const MESSAGE_TRANSMITTER_SEED: &[u8] = b"message_transmitter";
pub const AUTHORITY_SEED: &[u8] = b"message_transmitter_authority";
pub const USED_NONCE_SEED: &[u8] = b"used_nonce";
pub const EVENT_AUTHORITY_SEED: &[u8] = b"__event_authority";

// `sha256("global:<name>")[..8]` / `sha256("account:<Name>")[..8]`.
pub const SEND_MESSAGE_DISCRIMINATOR: [u8; 8] = [57, 40, 34, 178, 189, 10, 65, 26];
pub const RECEIVE_MESSAGE_DISCRIMINATOR: [u8; 8] = [38, 144, 127, 225, 31, 225, 238, 25];
pub const RECLAIM_EVENT_ACCOUNT_DISCRIMINATOR: [u8; 8] = [94, 198, 180, 159, 131, 236, 15, 174];
pub const HANDLE_RECEIVE_FINALIZED_MESSAGE_DISCRIMINATOR: [u8; 8] =
    [186, 252, 239, 70, 86, 180, 110, 95];
pub const HANDLE_RECEIVE_UNFINALIZED_MESSAGE_DISCRIMINATOR: [u8; 8] =
    [200, 169, 175, 20, 200, 58, 182, 61];
pub const MESSAGE_TRANSMITTER_ACCOUNT_DISCRIMINATOR: [u8; 8] = [71, 40, 180, 142, 19, 203, 35, 252];
pub const MESSAGE_SENT_ACCOUNT_DISCRIMINATOR: [u8; 8] = [131, 100, 133, 56, 166, 225, 151, 60];

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct SendMessageParams {
    pub destination_domain: u32,
    pub recipient: Pubkey,
    pub destination_caller: Pubkey,
    pub min_finality_threshold: u32,
    pub message_body: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct ReceiveMessageParams {
    pub message: Vec<u8>,
    pub attestation: Vec<u8>,
}

/// What the transmitter passes both `handle_receive_*` handlers; field order
/// is Circle's Borsh ABI.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct HandleReceiveMessageParams {
    pub remote_domain: u32,
    pub sender: Pubkey,
    pub finality_threshold_executed: u32,
    pub message_body: Vec<u8>,
    pub authority_bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct ReclaimEventAccountParams {
    pub attestation: Vec<u8>,
    pub destination_message: Vec<u8>,
}

/// Circle's `MessageSent` event account (`send_message`'s
/// `message_sent_event_data`).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct MessageSentAccount {
    pub rent_payer: Pubkey,
    pub created_at: i64,
    pub message: Vec<u8>,
}

impl MessageSentAccount {
    pub fn try_from_data(data: &[u8]) -> Option<Self> {
        let body = data.strip_prefix(&MESSAGE_SENT_ACCOUNT_DISCRIMINATOR)?;
        Self::deserialize(&mut &body[..]).ok()
    }
}

pub fn message_transmitter_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MESSAGE_TRANSMITTER_SEED], &MESSAGE_TRANSMITTER_V2_ID)
}

/// Signs every `handle_receive_*` CPI into `receiver`.
pub fn authority_pda(receiver: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[AUTHORITY_SEED, receiver.as_ref()],
        &MESSAGE_TRANSMITTER_V2_ID,
    )
}

/// `authority_pda` for a caller-supplied bump, `None` when it is not a valid PDA.
pub fn authority_pda_with_bump(receiver: &Pubkey, bump: u8) -> Option<Pubkey> {
    Pubkey::create_program_address(
        &[AUTHORITY_SEED, receiver.as_ref(), &[bump]],
        &MESSAGE_TRANSMITTER_V2_ID,
    )
    .ok()
}

pub fn used_nonce_pda(nonce: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[USED_NONCE_SEED, nonce], &MESSAGE_TRANSMITTER_V2_ID)
}

/// The transmitter's own `#[event_cpi]` authority (`receive_message`).
pub fn event_authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[EVENT_AUTHORITY_SEED], &MESSAGE_TRANSMITTER_V2_ID)
}

/// The Iris-assigned nonce in an attested message (zero in a source message).
pub fn nonce(message: &[u8]) -> Option<[u8; 32]> {
    message.get(NONCE_INDEX..SENDER_INDEX)?.try_into().ok()
}

/// Circle's `Message::format_message` with the Iris-written fields (nonce,
/// executed finality) filled in, as an attested message carries them. Used by
/// tests to build inbound messages.
#[allow(clippy::too_many_arguments)]
pub fn format_message(
    source_domain: u32,
    destination_domain: u32,
    nonce: [u8; 32],
    sender: &Pubkey,
    recipient: &Pubkey,
    min_finality_threshold: u32,
    finality_threshold_executed: u32,
    message_body: &[u8],
) -> Vec<u8> {
    MESSAGE_VERSION
        .to_be_bytes()
        .into_iter()
        .chain(source_domain.to_be_bytes())
        .chain(destination_domain.to_be_bytes())
        .chain(nonce)
        .chain(sender.to_bytes())
        .chain(recipient.to_bytes())
        .chain([0u8; 32])
        .chain(min_finality_threshold.to_be_bytes())
        .chain(finality_threshold_executed.to_be_bytes())
        .chain(message_body.iter().copied())
        .collect()
}

fn instruction_data(discriminator: [u8; 8], params: &impl AnchorSerialize) -> Result<Vec<u8>> {
    Ok(discriminator
        .into_iter()
        .chain(anchor_lang::prelude::borsh::to_vec(params)?)
        .collect())
}

/// `send_message` as `prove` CPIs it: `payer` funds the fresh
/// `message_sent_event_data` keypair account, our `sender_authority` signs.
pub fn send_message_instruction(
    payer: &Pubkey,
    message_sent_event_data: &Pubkey,
    params: &SendMessageParams,
) -> Result<Instruction> {
    Ok(Instruction {
        program_id: MESSAGE_TRANSMITTER_V2_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(sender_authority_pda().0, true),
            AccountMeta::new(message_transmitter_pda().0, false),
            AccountMeta::new(*message_sent_event_data, true),
            AccountMeta::new_readonly(crate::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: instruction_data(SEND_MESSAGE_DISCRIMINATOR, params)?,
    })
}

/// `receive_message` delivering an attested `message` to this program, `payer`
/// signing as Circle's payer and caller and again in the forwarded tail, where
/// it pays the `Proof` rent. `proofs` are our `Proof` PDAs in body order. The
/// single source of the account layout `handle_receive_*` validates, shared by
/// the integration tests.
pub fn receive_message_instruction(
    payer: &Pubkey,
    message: Vec<u8>,
    attestation: Vec<u8>,
    proofs: &[Pubkey],
) -> Result<Instruction> {
    let nonce = nonce(&message).ok_or(CctpProverError::InvalidData)?;
    let accounts = [
        AccountMeta::new(*payer, true),
        AccountMeta::new_readonly(*payer, true),
        AccountMeta::new_readonly(authority_pda(&crate::ID).0, false),
        AccountMeta::new_readonly(message_transmitter_pda().0, false),
        AccountMeta::new(used_nonce_pda(&nonce).0, false),
        AccountMeta::new_readonly(crate::ID, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(event_authority_pda().0, false),
        AccountMeta::new_readonly(MESSAGE_TRANSMITTER_V2_ID, false),
        // Forwarded by the transmitter to `handle_receive_*`, after its authority.
        AccountMeta::new_readonly(Config::pda().0, false),
        AccountMeta::new(*payer, true),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(eco_svm_std::event_authority_pda(&crate::ID).0, false),
        AccountMeta::new_readonly(crate::ID, false),
    ]
    .into_iter()
    .chain(proofs.iter().map(|proof| AccountMeta::new(*proof, false)))
    .collect();

    Ok(Instruction {
        program_id: MESSAGE_TRANSMITTER_V2_ID,
        accounts,
        data: instruction_data(
            RECEIVE_MESSAGE_DISCRIMINATOR,
            &ReceiveMessageParams {
                message,
                attestation,
            },
        )?,
    })
}

/// Every `receive_message` account that exists before delivery and is neither
/// a signer nor the invoked program: the contents of a relayer's lookup table.
/// `used_nonce` and the `Proof` PDAs are new per message and stay static; the
/// payer signs, so it is static too and its tail entry dedups to its index.
pub fn receive_lookup_table_addresses() -> Vec<Pubkey> {
    vec![
        authority_pda(&crate::ID).0,
        message_transmitter_pda().0,
        crate::ID,
        system_program::ID,
        event_authority_pda().0,
        Config::pda().0,
        eco_svm_std::event_authority_pda(&crate::ID).0,
    ]
}

/// Circle's `reclaim_event_account`: returns `send_message`'s event-account
/// rent to `payee` (its recorded `rent_payer`) after
/// `EVENT_ACCOUNT_WINDOW_SECONDS`, given Iris's attested `destination_message`.
pub fn reclaim_event_account_instruction(
    payee: &Pubkey,
    message_sent_event_data: &Pubkey,
    attestation: Vec<u8>,
    destination_message: Vec<u8>,
) -> Result<Instruction> {
    Ok(Instruction {
        program_id: MESSAGE_TRANSMITTER_V2_ID,
        accounts: vec![
            AccountMeta::new(*payee, true),
            AccountMeta::new(message_transmitter_pda().0, false),
            AccountMeta::new(*message_sent_event_data, false),
        ],
        data: instruction_data(
            RECLAIM_EVENT_ACCOUNT_DISCRIMINATOR,
            &ReclaimEventAccountParams {
                attestation,
                destination_message,
            },
        )?,
    })
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    fn anchor_discriminator(preimage: &str) -> [u8; 8] {
        Sha256::digest(preimage.as_bytes())[..8].try_into().unwrap()
    }

    #[test]
    fn mirrored_discriminators_match_anchor_derivation() {
        let pairs = [
            ("global:send_message", SEND_MESSAGE_DISCRIMINATOR),
            ("global:receive_message", RECEIVE_MESSAGE_DISCRIMINATOR),
            (
                "global:reclaim_event_account",
                RECLAIM_EVENT_ACCOUNT_DISCRIMINATOR,
            ),
            (
                "global:handle_receive_finalized_message",
                HANDLE_RECEIVE_FINALIZED_MESSAGE_DISCRIMINATOR,
            ),
            (
                "global:handle_receive_unfinalized_message",
                HANDLE_RECEIVE_UNFINALIZED_MESSAGE_DISCRIMINATOR,
            ),
            (
                "account:MessageTransmitter",
                MESSAGE_TRANSMITTER_ACCOUNT_DISCRIMINATOR,
            ),
            ("account:MessageSent", MESSAGE_SENT_ACCOUNT_DISCRIMINATOR),
        ];
        pairs
            .iter()
            .for_each(|(preimage, mirrored)| assert_eq!(anchor_discriminator(preimage), *mirrored));
    }

    /// The live state account on both clusters (read on-chain 2026-10-12):
    /// pins the seed, not just the derivation.
    #[test]
    fn message_transmitter_pda_matches_live_account() {
        assert_eq!(
            message_transmitter_pda().0,
            pubkey!("W1k5ijkaSTo5iA5zChNpfzcy796fLhkBxfmJuR8W8HU")
        );
    }

    #[test]
    fn transmitter_pdas_deterministic() {
        goldie::assert_json!(vec![
            authority_pda(&crate::ID),
            event_authority_pda(),
            used_nonce_pda(&[7; 32]),
        ]);
    }

    #[test]
    fn format_message_lays_out_circle_header() {
        let sender = Pubkey::new_from_array([1; 32]);
        let recipient = Pubkey::new_from_array([2; 32]);
        let message = format_message(
            6,
            SOLANA_DOMAIN,
            [9; 32],
            &sender,
            &recipient,
            1000,
            2000,
            &[0xab, 0xcd],
        );

        assert_eq!(message.len(), MESSAGE_BODY_INDEX + 2);
        assert_eq!(message[0..4], MESSAGE_VERSION.to_be_bytes());
        assert_eq!(message[4..8], 6u32.to_be_bytes());
        assert_eq!(message[8..12], SOLANA_DOMAIN.to_be_bytes());
        assert_eq!(nonce(&message), Some([9; 32]));
        assert_eq!(message[SENDER_INDEX..RECIPIENT_INDEX], sender.to_bytes());
        assert_eq!(
            message[RECIPIENT_INDEX..DESTINATION_CALLER_INDEX],
            recipient.to_bytes()
        );
        assert_eq!(
            message[DESTINATION_CALLER_INDEX..MIN_FINALITY_THRESHOLD_INDEX],
            [0; 32]
        );
        assert_eq!(
            message[MIN_FINALITY_THRESHOLD_INDEX..FINALITY_THRESHOLD_EXECUTED_INDEX],
            1000u32.to_be_bytes()
        );
        assert_eq!(
            message[FINALITY_THRESHOLD_EXECUTED_INDEX..MESSAGE_BODY_INDEX],
            2000u32.to_be_bytes()
        );
        assert_eq!(message[MESSAGE_BODY_INDEX..], [0xab, 0xcd]);
        assert_eq!(nonce(&message[..NONCE_INDEX + 31]), None);
    }

    /// `receive_message` head accounts in Circle's `ReceiveMessageContext`
    /// order (+ `#[event_cpi]`), then the tail Circle forwards to our handler
    /// in `HandleReceiveMessage` order (+ our `#[event_cpi]`), then proofs.
    #[test]
    fn receive_message_instruction_orders_accounts() {
        let payer = Pubkey::new_unique();
        let message = format_message(
            6,
            SOLANA_DOMAIN,
            [3; 32],
            &payer,
            &crate::ID,
            1000,
            1000,
            &[],
        );
        let proofs = [Pubkey::new_unique(), Pubkey::new_unique()];
        let ix = receive_message_instruction(&payer, message, vec![0; 65], &proofs).unwrap();

        let keys: Vec<_> = ix.accounts.iter().map(|meta| meta.pubkey).collect();
        assert_eq!(ix.program_id, MESSAGE_TRANSMITTER_V2_ID);
        assert_eq!(
            keys,
            vec![
                payer,
                payer,
                authority_pda(&crate::ID).0,
                message_transmitter_pda().0,
                used_nonce_pda(&[3; 32]).0,
                crate::ID,
                anchor_lang::system_program::ID,
                event_authority_pda().0,
                MESSAGE_TRANSMITTER_V2_ID,
                crate::state::Config::pda().0,
                payer,
                anchor_lang::system_program::ID,
                eco_svm_std::event_authority_pda(&crate::ID).0,
                crate::ID,
                proofs[0],
                proofs[1],
            ]
        );
        assert!(ix.accounts[0].is_signer && ix.accounts[0].is_writable);
        assert!(ix.accounts[4].is_writable);
        assert!(ix.accounts[10].is_writable && ix.accounts[10].is_signer);
        assert!(ix.accounts[14..]
            .iter()
            .all(|meta| meta.is_writable && !meta.is_signer));
        assert_eq!(ix.data[..8], RECEIVE_MESSAGE_DISCRIMINATOR);
        // A lookup table cannot carry a signer.
        let table = receive_lookup_table_addresses();
        assert!(ix
            .accounts
            .iter()
            .filter(|meta| table.contains(&meta.pubkey))
            .all(|meta| !meta.is_signer));
    }

    /// Circle computes the CPI discriminator from the handler's name; ours
    /// must be Anchor's default for the identically named instructions.
    #[test]
    fn handler_discriminators_match_generated_instructions() {
        use anchor_lang::Discriminator;
        assert_eq!(
            crate::instruction::HandleReceiveFinalizedMessage::DISCRIMINATOR,
            HANDLE_RECEIVE_FINALIZED_MESSAGE_DISCRIMINATOR.as_slice()
        );
        assert_eq!(
            crate::instruction::HandleReceiveUnfinalizedMessage::DISCRIMINATOR,
            HANDLE_RECEIVE_UNFINALIZED_MESSAGE_DISCRIMINATOR.as_slice()
        );
    }

    #[test]
    fn message_sent_account_parses_circle_layout() {
        let message = vec![1, 2, 3];
        let mut data = MESSAGE_SENT_ACCOUNT_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&[4; 32]);
        data.extend_from_slice(&77i64.to_le_bytes());
        data.extend_from_slice(&(message.len() as u32).to_le_bytes());
        data.extend_from_slice(&message);

        let parsed = MessageSentAccount::try_from_data(&data).unwrap();
        assert_eq!(parsed.rent_payer, Pubkey::new_from_array([4; 32]));
        assert_eq!(parsed.created_at, 77);
        assert_eq!(parsed.message, message);
        assert!(MessageSentAccount::try_from_data(&data[1..]).is_none());
    }
}
