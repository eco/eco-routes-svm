use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::{Error as ClientError, ErrorKind};
use solana_rpc_client_api::config::RpcSendTransactionConfig;
use solana_sdk::account::Account;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::chain::{self, Chain};

/// The validator rebroadcasts a transaction until it lands or its blockhash expires (about 60
/// seconds); this many attempts cover a congested leader without flooding the endpoint.
const SEND_MAX_RETRIES: usize = 10;
const SCHEMES: [&str; 4] = ["http://", "https://", "ws://", "wss://"];
const REDACTED_URL: &str = "<rpc-url>";

pub struct RpcChain {
    client: RpcClient,
    url: String,
}

impl RpcChain {
    pub fn new(url: String) -> Self {
        let client = RpcClient::new_with_commitment(url.clone(), CommitmentConfig::confirmed());

        Self { client, url }
    }

    /// The RPC URL is a secret (it carries the API key): no error text may contain it.
    fn describe(&self, error: ClientError) -> String {
        let text = match *error.kind {
            ErrorKind::Reqwest(error) => format!("{:?}", error.without_url()),
            kind => format!("{kind:?}"),
        };

        redact_urls(&text.replace(&self.url, REDACTED_URL))
    }
}

/// Client libraries normalize the URL they echo, so the configured string alone is not enough:
/// every URL-shaped token goes.
fn redact_urls(text: &str) -> String {
    let start = SCHEMES.iter().filter_map(|scheme| text.find(scheme)).min();
    let Some(start) = start else {
        return text.to_owned();
    };
    let (before, url) = text.split_at(start);
    let end = url
        .find(|character: char| character.is_whitespace() || "()\\\"'<>".contains(character))
        .unwrap_or(url.len());

    format!("{before}{REDACTED_URL}{}", redact_urls(&url[end..]))
}

impl Chain for RpcChain {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>, chain::Error> {
        self.client
            .get_account_with_commitment(address, self.client.commitment())
            .map(|response| response.value)
            .map_err(|error| chain::Error::AccountFetchFailed {
                address: *address,
                reason: self.describe(error),
            })
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, chain::Error> {
        let send_failed = |error| chain::Error::SendFailed {
            reason: self.describe(error),
        };
        let payer = signers.first().expect("a transaction needs a fee payer");
        let blockhash = self.client.get_latest_blockhash().map_err(send_failed)?;
        let transaction = Transaction::new(
            signers,
            Message::new(instructions, Some(&payer.pubkey())),
            blockhash,
        );
        let config = RpcSendTransactionConfig {
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            max_retries: Some(SEND_MAX_RETRIES),
            ..RpcSendTransactionConfig::default()
        };

        self.client
            .send_and_confirm_transaction_with_spinner_and_config(
                &transaction,
                CommitmentConfig::confirmed(),
                config,
            )
            .map_err(send_failed)
    }

    /// Finalized, not confirmed: the lookup-table program takes a `recent_slot` only if it is in
    /// `SlotHashes`, and a confirmed slot can sit on a fork that never lands there.
    fn slot(&self) -> Result<u64, chain::Error> {
        self.client
            .get_slot_with_commitment(CommitmentConfig::finalized())
            .map_err(|error| chain::Error::SlotFetchFailed {
                reason: self.describe(error),
            })
    }

    fn genesis_hash(&self) -> Result<Hash, chain::Error> {
        self.client
            .get_genesis_hash()
            .map_err(|error| chain::Error::GenesisHashFetchFailed {
                reason: self.describe(error),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second is normalized by the HTTP client (a `/` is added), so its errors echo a URL
    /// that differs from the configured string.
    const UNREACHABLE: [&str; 2] = [
        "http://127.0.0.1:1/?api-key=SECRET",
        "http://127.0.0.1:1?api-key=SECRET",
    ];

    #[test]
    fn errors_never_contain_the_rpc_url() {
        UNREACHABLE.into_iter().for_each(errors_omit);
    }

    fn errors_omit(url: &str) {
        let mut chain = RpcChain::new(url.into());
        let deployer = Keypair::new();
        let transfer = solana_system_interface::instruction::transfer(
            &deployer.pubkey(),
            &Pubkey::new_unique(),
            1,
        );

        let errors = [
            chain.account(&Pubkey::new_unique()).unwrap_err(),
            chain.slot().unwrap_err(),
            chain.genesis_hash().unwrap_err(),
            chain.send(&[transfer], &[&deployer]).unwrap_err(),
        ];

        errors.iter().for_each(|error| {
            let text = format!("{error} {error:?}");

            assert!(!text.contains("SECRET"), "{text}");
            assert!(!text.contains("api-key"), "{text}");
            assert!(text.len() > 40, "error lost its context: {text}");
        });
    }
}
