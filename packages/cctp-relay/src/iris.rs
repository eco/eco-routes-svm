//! Circle Iris v2: `GET {base}/v2/messages/{sourceDomain}?transactionHash={hash}`.
//! Mainnet `https://iris-api.circle.com`, testnets `https://iris-api-sandbox.circle.com`.
//! Iris answers 404 until it has indexed the transaction.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::Deserialize;

pub const MAINNET: &str = "https://iris-api.circle.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum IrisError {
    #[error("Iris request failed: {0}")]
    Http(String),
    #[error("Iris returned malformed data: {0}")]
    Malformed(String),
    #[error("no attested message after {0:?}")]
    Timeout(Duration),
}

#[derive(Deserialize)]
struct Response {
    messages: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    message: String,
    attestation: String,
    status: String,
}

/// An attested message: the bytes `receive_message` / `reclaim_event_account` take.
#[derive(Debug, Clone, PartialEq)]
pub struct Attested {
    pub message: Vec<u8>,
    pub attestation: Vec<u8>,
}

fn decode(hex_string: &str) -> Result<Vec<u8>, IrisError> {
    hex::decode(hex_string.trim_start_matches("0x"))
        .map_err(|e| IrisError::Malformed(e.to_string()))
}

/// Every `complete` message in an Iris response body; pending entries are skipped.
pub fn parse(body: &str) -> Result<Vec<Attested>, IrisError> {
    let response: Response =
        serde_json::from_str(body).map_err(|e| IrisError::Malformed(e.to_string()))?;
    response
        .messages
        .into_iter()
        .filter(|entry| entry.status == "complete")
        .map(|entry| {
            Ok(Attested {
                message: decode(&entry.message)?,
                attestation: decode(&entry.attestation)?,
            })
        })
        .collect()
}

pub fn fetch(base: &str, source_domain: u32, tx: &str) -> Result<Vec<Attested>, IrisError> {
    let url = format!("{base}/v2/messages/{source_domain}?transactionHash={tx}");
    let http = |e: reqwest::Error| IrisError::Http(e.without_url().to_string());
    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(http)?;
    let response = client.get(url).send().map_err(http)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }
    let body = response
        .error_for_status()
        .map_err(http)?
        .text()
        .map_err(http)?;

    parse(&body)
}

/// Polls `fetch` every 2 s until `accept` picks a message or `timeout` passes.
pub fn poll(
    base: &str,
    source_domain: u32,
    tx: &str,
    timeout: Duration,
    accept: impl Fn(&Attested) -> bool,
) -> Result<Attested, IrisError> {
    let start = Instant::now();
    loop {
        if let Some(found) = fetch(base, source_domain, tx)?
            .into_iter()
            .find(|m| accept(m))
        {
            return Ok(found);
        }
        if start.elapsed() >= timeout {
            return Err(IrisError::Timeout(timeout));
        }
        sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_and_skips_pending() {
        let complete = parse(include_str!("../testdata/iris_complete.json")).unwrap();
        assert_eq!(complete.len(), 1);
        assert_eq!(complete[0].attestation, vec![0xaa, 0xbb]);
        assert_eq!(complete[0].message[4..8], 6u32.to_be_bytes());
        assert!(parse(include_str!("../testdata/iris_pending.json"))
            .unwrap()
            .is_empty());
        assert!(parse("{").is_err());
    }
}
