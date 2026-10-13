use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, extend_lookup_table, freeze_lookup_table,
};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::rpc;

/// Creates, fills with `cctp::receive_lookup_table_addresses()` and freezes
/// the relayer's lookup table (three transactions; the table must be one slot
/// old before use).
pub fn run(rpc_url: &str, keypair_path: &str) -> Result<(), String> {
    let payer = rpc::keypair(keypair_path)?;
    let client = rpc::client(rpc_url);
    let slot = client.get_slot().map_err(|e| rpc::redact(e, rpc_url))?;
    let (create, table) = create_lookup_table(payer.pubkey(), payer.pubkey(), slot);
    let extend = extend_lookup_table(
        table,
        payer.pubkey(),
        Some(payer.pubkey()),
        cctp_prover::cctp::receive_lookup_table_addresses(),
    );
    let freeze = freeze_lookup_table(table, payer.pubkey());
    for instruction in [create, extend, freeze] {
        let blockhash = client
            .get_latest_blockhash()
            .map_err(|e| rpc::redact(e, rpc_url))?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        client
            .send_and_confirm_transaction(&transaction)
            .map_err(|e| rpc::redact(e, rpc_url))?;
    }
    println!("lookup table: {table}");

    Ok(())
}
