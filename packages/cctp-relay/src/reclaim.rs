use cctp_prover::cctp::{
    self, MessageSentAccount, EVENT_ACCOUNT_WINDOW_SECONDS, MESSAGE_SENT_ACCOUNT_DISCRIMINATOR,
    SOLANA_DOMAIN,
};
use solana_account_decoder_client_types::UiAccountEncoding;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_rpc_client_api::filter::{Memcmp, RpcFilterType};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::iris::{self, Attested};
use crate::rpc;

/// Circle's `hash_source_fields` preimage: everything in the header except
/// the Iris-written nonce and executed finality, plus the length.
pub fn source_fields(message: &[u8]) -> Vec<u8> {
    [
        &message[0..cctp::NONCE_INDEX],
        &message[cctp::SENDER_INDEX..cctp::FINALITY_THRESHOLD_EXECUTED_INDEX],
        &(message.len() as u64).to_be_bytes(),
    ]
    .concat()
}

pub fn is_reclaimable(account: &MessageSentAccount, now: i64) -> bool {
    now >= account
        .created_at
        .saturating_add(EVENT_ACCOUNT_WINDOW_SECONDS)
}

pub fn matches(event: &MessageSentAccount, attested: &Attested) -> bool {
    source_fields(&event.message) == source_fields(&attested.message)
}

/// Finds this payer's event accounts older than 5 days, looks up each one's
/// attested message on Iris by its creating transaction, and reclaims it.
pub fn run(
    rpc_url: &str,
    keypair_path: &str,
    iris_base: &str,
    dry_run: bool,
) -> Result<(), String> {
    let payee = rpc::keypair(keypair_path)?;
    let client = rpc::client(rpc_url);
    let config = RpcProgramAccountsConfig {
        filters: Some(vec![
            RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                0,
                MESSAGE_SENT_ACCOUNT_DISCRIMINATOR.to_vec(),
            )),
            RpcFilterType::Memcmp(Memcmp::new_raw_bytes(8, payee.pubkey().to_bytes().to_vec())),
        ]),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            ..Default::default()
        },
        ..Default::default()
    };
    let accounts = client
        .get_program_ui_accounts_with_config(&cctp::MESSAGE_TRANSMITTER_V2_ID, config)
        .map_err(|e| rpc::redact(e, rpc_url))?;
    let now = client
        .get_block_time(client.get_slot().map_err(|e| rpc::redact(e, rpc_url))?)
        .map_err(|e| rpc::redact(e, rpc_url))?;
    for (address, account) in accounts {
        let data = account.data.decode().ok_or("undecodable account data")?;
        let Some(event) = MessageSentAccount::try_from_data(&data) else {
            continue;
        };
        if !is_reclaimable(&event, now) {
            continue;
        }
        let signatures = client
            .get_signatures_for_address(&address)
            .map_err(|e| rpc::redact(e, rpc_url))?;
        let Some(creation) = signatures.last() else {
            continue;
        };
        let Some(attested) = iris::fetch(iris_base, SOLANA_DOMAIN, &creation.signature)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|attested| matches(&event, attested))
        else {
            println!("{address}: not attested yet, skipped");
            continue;
        };
        if dry_run {
            println!("{address}: reclaimable");
            continue;
        }
        let ix = cctp::reclaim_event_account_instruction(
            &payee.pubkey(),
            &address,
            attested.attestation,
            attested.message,
        )
        .map_err(|e| e.to_string())?;
        let blockhash = client
            .get_latest_blockhash()
            .map_err(|e| rpc::redact(e, rpc_url))?;
        let transaction =
            Transaction::new_signed_with_payer(&[ix], Some(&payee.pubkey()), &[&payee], blockhash);
        let signature = client
            .send_and_confirm_transaction(&transaction)
            .map_err(|e| rpc::redact(e, rpc_url))?;
        println!("{address}: reclaimed in {signature}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use solana_sdk::pubkey::Pubkey;

    use super::*;

    #[test]
    fn source_fields_ignore_nonce_and_executed_finality() {
        let sender = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let source = cctp::format_message(
            SOLANA_DOMAIN,
            6,
            [0; 32],
            &sender,
            &recipient,
            1000,
            0,
            &[1, 2],
        );
        let attested = cctp::format_message(
            SOLANA_DOMAIN,
            6,
            [9; 32],
            &sender,
            &recipient,
            1000,
            1000,
            &[1, 2],
        );
        let other = cctp::format_message(
            SOLANA_DOMAIN,
            6,
            [9; 32],
            &sender,
            &recipient,
            1000,
            1000,
            &[1, 2, 3],
        );
        assert_eq!(source_fields(&source), source_fields(&attested));
        assert_ne!(source_fields(&source), source_fields(&other));
    }

    #[test]
    fn reclaimable_after_five_days() {
        let event = MessageSentAccount {
            rent_payer: Pubkey::new_unique(),
            created_at: 1_000,
            message: vec![],
        };
        assert!(!is_reclaimable(
            &event,
            1_000 + EVENT_ACCOUNT_WINDOW_SECONDS - 1
        ));
        assert!(is_reclaimable(&event, 1_000 + EVENT_ACCOUNT_WINDOW_SECONDS));
    }
}
