//! The RPC URL is a secret (it carries the API key): no output may contain it.
use solana_commitment_config::CommitmentConfig;
use solana_rpc_client::rpc_client::RpcClient;
use solana_sdk::signature::{read_keypair_file, Keypair};

pub fn client(url: &str) -> RpcClient {
    RpcClient::new_with_commitment(url.to_string(), CommitmentConfig::confirmed())
}

/// Error text with the URL replaced, for any client error.
pub fn redact(error: impl std::fmt::Display, url: &str) -> String {
    redact_all(&error.to_string(), url)
}

/// Parts of a URL shorter than this are not treated as secrets: `v2`, `rpc` or a port would
/// otherwise be scrubbed out of every message.
const MIN_SECRET_PART_LEN: usize = 8;

/// Replaces the whole URL (as given and as the HTTP client re-serializes it) with `<rpc-url>`,
/// then each part an API key can hide in (userinfo, path segments, query values) with
/// `<redacted>`, longest first so a part never splits a larger match.
fn redact_all(text: &str, url: &str) -> String {
    if url.is_empty() {
        return text.to_string();
    }
    let mut whole = vec![url.to_string(), url.trim_end_matches('/').to_string()];
    let mut parts = Vec::new();
    if let Ok(parsed) = reqwest::Url::parse(url) {
        whole.push(parsed.as_str().to_string());
        whole.push(parsed.as_str().trim_end_matches('/').to_string());
        parts.push(parsed.username().to_string());
        parts.extend(parsed.password().map(str::to_string));
        parts.extend(
            parsed
                .path_segments()
                .into_iter()
                .flatten()
                .map(str::to_string),
        );
        parts.extend(parsed.query_pairs().map(|(_, value)| value.into_owned()));
    }
    whole.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    parts.retain(|part| part.len() >= MIN_SECRET_PART_LEN);
    parts.sort_by_key(|secret| std::cmp::Reverse(secret.len()));

    let mut redacted = text.to_string();
    for secret in whole.iter().filter(|secret| !secret.is_empty()) {
        redacted = redacted.replace(secret.as_str(), "<rpc-url>");
    }
    for secret in &parts {
        redacted = redacted.replace(secret.as_str(), "<redacted>");
    }

    redacted
}

/// Keypairs are read from a file path, never passed inline.
pub fn keypair(path: &str) -> Result<Keypair, String> {
    read_keypair_file(path).map_err(|_| format!("cannot read keypair file {path}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn redact_removes_url() {
        let url = "https://example.invalid/v2/KEY";
        assert_eq!(
            super::redact(format!("failed {url} 429"), url),
            "failed <rpc-url> 429"
        );
    }

    #[test]
    fn redact_removes_normalized_and_partial_forms() {
        let url = "https://user:hunter22hunter@example.invalid/v2/APIKEY12345?api-key=QUERYKEY999";
        // The HTTP client re-serializes the URL, so the exact input string may not appear.
        let normalized =
            "https://user:hunter22hunter@example.invalid/v2/APIKEY12345?api-key=QUERYKEY999";
        for text in [
            format!("error sending request for url ({normalized})"),
            "path /v2/APIKEY12345 rejected".to_string(),
            "bad query api-key=QUERYKEY999".to_string(),
            "auth hunter22hunter failed".to_string(),
        ] {
            let redacted = super::redact(&text, url);
            for secret in ["APIKEY12345", "QUERYKEY999", "hunter22hunter"] {
                assert!(!redacted.contains(secret), "{secret} leaked in {redacted}");
            }
        }
    }

    #[test]
    fn redact_keeps_unrelated_text_and_short_segments() {
        let url = "https://example.invalid/v2/APIKEY12345";
        assert_eq!(
            super::redact("slot 5 behind, v2 ok", url),
            "slot 5 behind, v2 ok"
        );
    }

    #[test]
    fn redact_with_empty_url_is_identity() {
        assert_eq!(super::redact("boom", ""), "boom");
    }
}
