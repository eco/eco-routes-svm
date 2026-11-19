use std::str::FromStr;
use std::thread;
use std::time::Duration;

use solana_account_decoder_client_types::{UiAccount, UiAccountEncoding};
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::{Error as ClientError, ErrorKind};
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcSendTransactionConfig};
use solana_rpc_client_api::custom_error::JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED;
use solana_rpc_client_api::request::RpcError;
use solana_sdk::account::Account;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};

use crate::chain::{self, Chain};
use crate::transaction;

/// The validator rebroadcasts a transaction until it lands or its blockhash expires (about 60
/// seconds); this many attempts cover a congested leader without flooding the endpoint.
const SEND_MAX_RETRIES: usize = 10;
/// A load-balanced endpoint can route a read to a node behind the last confirmed send; reads wait
/// this long, this many times, for it to catch up.
const CATCH_UP_ATTEMPTS: u32 = 20;
const CATCH_UP_WAIT: Duration = Duration::from_millis(500);
const SCHEMES: [&str; 4] = ["http://", "https://", "ws://", "wss://"];
const REDACTED_URL: &str = "<rpc-url>";

pub struct RpcChain {
    client: RpcClient,
    url: String,
    /// Micro-lamports per compute unit, paid as each transaction's priority fee.
    compute_unit_price: u64,
    /// The slot of the last transaction this chain confirmed. Every read asks for state at least
    /// that recent, so a lagging node cannot show the state from before it: the IDL upload
    /// would otherwise redo a step that already landed.
    min_context_slot: u64,
}

impl RpcChain {
    pub fn new(url: String) -> Self {
        let client = RpcClient::new_with_commitment(url.clone(), CommitmentConfig::confirmed());

        Self {
            client,
            url,
            compute_unit_price: 0,
            min_context_slot: 0,
        }
    }

    pub fn with_compute_unit_price(self, compute_unit_price: u64) -> Self {
        Self {
            compute_unit_price,
            ..self
        }
    }

    /// The RPC URL is a secret (it carries the API key): no error text may contain it.
    fn describe(&self, error: ClientError) -> String {
        let text = match *error.kind {
            ErrorKind::Reqwest(error) => format!("{:?}", error.without_url()),
            kind => format!("{kind:?}"),
        };

        redact_urls(&text.replace(&self.url, REDACTED_URL))
    }

    /// The slot `signature` landed in, once a node reports it; `None` if none does in time,
    /// which only leaves reads without the guard.
    fn landed_slot(&self, signature: &Signature) -> Option<u64> {
        (0..CATCH_UP_ATTEMPTS).find_map(|attempt| {
            if attempt > 0 {
                thread::sleep(CATCH_UP_WAIT);
            }
            self.client
                .get_signature_statuses(&[*signature])
                .ok()?
                .value
                .into_iter()
                .next()
                .flatten()
                .map(|status| status.slot)
        })
    }
}

fn behind_min_context_slot(error: &ClientError) -> bool {
    matches!(
        &*error.kind,
        ErrorKind::RpcError(RpcError::RpcResponseError { code, .. })
            if *code == JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED
    )
}

fn to_account(account: UiAccount) -> Option<Account> {
    Some(Account {
        lamports: account.lamports,
        data: account.data.decode()?,
        owner: Pubkey::from_str(&account.owner).ok()?,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
    })
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
        let config = RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            commitment: Some(self.client.commitment()),
            min_context_slot: Some(self.min_context_slot),
            data_slice: None,
        };
        let failed = |reason| chain::Error::AccountFetchFailed {
            address: *address,
            reason,
        };
        let mut attempt = 0;

        loop {
            match self
                .client
                .get_ui_account_with_config(address, config.clone())
            {
                Ok(response) => {
                    return response
                        .value
                        .map(|account| to_account(account).ok_or(()))
                        .transpose()
                        .map_err(|()| failed("undecodable base64 account".to_owned()));
                }
                Err(error) if behind_min_context_slot(&error) && attempt < CATCH_UP_ATTEMPTS => {
                    attempt += 1;
                    thread::sleep(CATCH_UP_WAIT);
                }
                Err(error) => return Err(failed(self.describe(error))),
            }
        }
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, chain::Error> {
        let send_failed = |error| chain::Error::SendFailed {
            reason: self.describe(error),
        };
        let blockhash = self.client.get_latest_blockhash().map_err(send_failed)?;
        let transaction =
            transaction::signed(instructions, signers, blockhash, self.compute_unit_price)
                .map_err(|error| chain::Error::SendFailed {
                    reason: error.to_string(),
                })?;
        let config = RpcSendTransactionConfig {
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            max_retries: Some(SEND_MAX_RETRIES),
            ..RpcSendTransactionConfig::default()
        };

        let signature = self
            .client
            .send_and_confirm_transaction_with_spinner_and_config(
                &transaction,
                CommitmentConfig::confirmed(),
                config,
            )
            .map_err(send_failed)?;
        if let Some(slot) = self.landed_slot(&signature) {
            self.min_context_slot = self.min_context_slot.max(slot);
        }

        Ok(signature)
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
    use solana_sdk::signer::Signer;

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
