use std::time::Duration;

use cctp_prover::cctp::{self, MESSAGE_BODY_INDEX, RECIPIENT_INDEX, SOLANA_DOMAIN};
use cctp_prover::constants::MAX_PAIRS_PER_MESSAGE;
use eco_svm_std::prover::{Proof, ProofData};
use solana_address_lookup_table_interface::state::AddressLookupTable;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{v0, AddressLookupTableAccount, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;

use crate::iris::{self, Attested};
use crate::rpc;

/// Must cover the measured full batch (`cctp_prover_batch_limits`).
pub const COMPUTE_UNIT_LIMIT: u32 = 400_000;
const IRIS_TIMEOUT: Duration = Duration::from_secs(1800);

/// An attested message addressed to cctp-prover on Solana.
pub fn is_for_cctp_prover(attested: &Attested) -> bool {
    attested.message.len() >= MESSAGE_BODY_INDEX
        && attested.message[8..12] == SOLANA_DOMAIN.to_be_bytes()
        && attested.message[RECIPIENT_INDEX..RECIPIENT_INDEX + 32] == cctp_prover::ID.to_bytes()
}

/// The proof body of a message the relay is willing to deliver. Refuses, before any
/// transaction is built, a message not addressed to cctp-prover on Solana or carrying more
/// than `MAX_PAIRS_PER_MESSAGE` pairs (the source `prove` never sends more, so a larger
/// body did not come from a cctp-prover peer).
pub fn check_deliverable(attested: &Attested) -> Result<ProofData, String> {
    if !is_for_cctp_prover(attested) {
        return Err(format!(
            "refusing to relay: message is not addressed to cctp-prover ({}) on Solana domain {SOLANA_DOMAIN}",
            cctp_prover::ID
        ));
    }
    let body = ProofData::from_bytes(&attested.message[MESSAGE_BODY_INDEX..])
        .map_err(|e| format!("refusing to relay: malformed proof body: {e}"))?;
    let pairs = body.intent_hashes_claimants.len();
    if pairs > MAX_PAIRS_PER_MESSAGE {
        return Err(format!(
            "refusing to relay: {pairs} pairs exceed the {MAX_PAIRS_PER_MESSAGE}-pair cap"
        ));
    }

    Ok(body)
}

/// `[ComputeBudget, receive_message]` with the body's `Proof` PDAs in order.
pub fn instructions(payer: &Pubkey, attested: Attested) -> Result<Vec<Instruction>, String> {
    let body = check_deliverable(&attested)?;
    let proofs: Vec<Pubkey> = body
        .intent_hashes_claimants
        .iter()
        .map(|pair| Proof::pda(&pair.intent_hash, &cctp_prover::ID).0)
        .collect();
    let receive =
        cctp::receive_message_instruction(payer, attested.message, attested.attestation, &proofs)
            .map_err(|e| e.to_string())?;

    Ok(vec![
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        receive,
    ])
}

pub fn run(
    rpc_url: &str,
    keypair_path: &str,
    iris_base: &str,
    source_domain: u32,
    tx: &str,
    lookup_table: Pubkey,
    dry_run: bool,
) -> Result<(), String> {
    let payer: Keypair = rpc::keypair(keypair_path)?;
    let client = rpc::client(rpc_url);
    let attested = iris::poll(
        iris_base,
        source_domain,
        tx,
        IRIS_TIMEOUT,
        is_for_cctp_prover,
    )
    .map_err(|e| e.to_string())?;
    let instructions = instructions(&payer.pubkey(), attested)?;
    let table_data = client
        .get_account_data(&lookup_table)
        .map_err(|e| rpc::redact(e, rpc_url))?;
    let table = AddressLookupTable::deserialize(&table_data).map_err(|e| e.to_string())?;
    let blockhash = client
        .get_latest_blockhash()
        .map_err(|e| rpc::redact(e, rpc_url))?;
    let message = v0::Message::try_compile(
        &payer.pubkey(),
        &instructions,
        &[AddressLookupTableAccount {
            key: lookup_table,
            addresses: table.addresses.to_vec(),
        }],
        blockhash,
    )
    .map_err(|e| e.to_string())?;
    let transaction = VersionedTransaction::try_new(VersionedMessage::V0(message), &[&payer])
        .map_err(|e| e.to_string())?;

    if dry_run {
        let simulation = client
            .simulate_transaction(&transaction)
            .map_err(|e| rpc::redact(e, rpc_url))?;
        println!(
            "simulation: err={:?} units={:?}",
            simulation.value.err, simulation.value.units_consumed
        );
        return Ok(());
    }
    let signature = client
        .send_and_confirm_transaction(&transaction)
        .map_err(|e| rpc::redact(e, rpc_url))?;
    println!("delivered: {signature}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use eco_svm_std::prover::IntentHashClaimant;

    use super::*;

    fn attested(destination_domain: u32, recipient: &Pubkey, pairs: usize) -> Attested {
        let body = ProofData::new(
            8453,
            (0..pairs)
                .map(|i| IntentHashClaimant::new([i as u8 + 1; 32].into(), [7; 32].into()))
                .collect(),
        );
        Attested {
            message: cctp::format_message(
                6,
                destination_domain,
                [9; 32],
                &Pubkey::new_unique(),
                recipient,
                1000,
                1000,
                &body.to_bytes(),
            ),
            attestation: vec![0; 130],
        }
    }

    #[test]
    fn accepts_only_messages_for_cctp_prover_on_solana() {
        assert!(is_for_cctp_prover(&attested(
            SOLANA_DOMAIN,
            &cctp_prover::ID,
            1
        )));
        assert!(!is_for_cctp_prover(&attested(3, &cctp_prover::ID, 1)));
        assert!(!is_for_cctp_prover(&attested(
            SOLANA_DOMAIN,
            &Pubkey::new_unique(),
            1
        )));
    }

    #[test]
    fn instructions_append_proofs_in_body_order() {
        let payer = Pubkey::new_unique();
        let ixs = instructions(&payer, attested(SOLANA_DOMAIN, &cctp_prover::ID, 2)).unwrap();
        let receive = &ixs[1];
        let first: eco_svm_std::Bytes32 = [1; 32].into();
        let second: eco_svm_std::Bytes32 = [2; 32].into();
        let n = receive.accounts.len();
        assert_eq!(
            receive.accounts[n - 2].pubkey,
            Proof::pda(&first, &cctp_prover::ID).0
        );
        assert_eq!(
            receive.accounts[n - 1].pubkey,
            Proof::pda(&second, &cctp_prover::ID).0
        );
    }

    #[test]
    fn refuses_a_recipient_that_is_not_cctp_prover() {
        let payer = Pubkey::new_unique();
        let error =
            instructions(&payer, attested(SOLANA_DOMAIN, &Pubkey::new_unique(), 1)).unwrap_err();
        assert!(error.contains("not addressed to cctp-prover"), "{error}");
    }

    #[test]
    fn refuses_a_message_for_another_destination_domain() {
        let payer = Pubkey::new_unique();
        assert!(instructions(&payer, attested(3, &cctp_prover::ID, 1)).is_err());
    }

    #[test]
    fn pair_cap_is_inclusive() {
        let payer = Pubkey::new_unique();
        let at_cap = attested(SOLANA_DOMAIN, &cctp_prover::ID, MAX_PAIRS_PER_MESSAGE);
        assert!(instructions(&payer, at_cap).is_ok());

        let over = attested(SOLANA_DOMAIN, &cctp_prover::ID, MAX_PAIRS_PER_MESSAGE + 1);
        let error = instructions(&payer, over).unwrap_err();
        assert!(error.contains("pair cap"), "{error}");
    }

    #[test]
    fn refuses_truncated_and_malformed_messages_without_panicking() {
        let payer = Pubkey::new_unique();
        let mut short = attested(SOLANA_DOMAIN, &cctp_prover::ID, 1);
        short.message.truncate(MESSAGE_BODY_INDEX - 1);
        assert!(instructions(&payer, short).is_err());

        let mut ragged = attested(SOLANA_DOMAIN, &cctp_prover::ID, 1);
        ragged.message.push(0);
        assert!(instructions(&payer, ragged)
            .unwrap_err()
            .contains("malformed"));
    }
}
