use {
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::Message,
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    solana_system_interface::instruction::{create_account, transfer},
    solana_transaction::Transaction,
};

fn main() {
    println!("Hello, world!");
}

#[test]
fn system_transfer() {
    let from_keypair = Keypair::new();
    let from = from_keypair.pubkey();
    let to = Pubkey::new_unique();

    let mut svm = LiteSVM::new();
    let expected_fee = 5000;
    svm.airdrop(&from, 100 + expected_fee).unwrap();

    let instruction = transfer(&from, &to, 64);
    let tx = Transaction::new(
        &[&from_keypair],
        Message::new(&[instruction], Some(&from)),
        svm.latest_blockhash(),
    );
    let tx_res = svm.send_transaction(tx);

    let from_account = svm.get_account(&from);
    let to_account = svm.get_account(&to);

    assert!(tx_res.is_ok());
    assert_eq!(from_account.unwrap().lamports, 36);
    assert_eq!(to_account.unwrap().lamports, 64);
}
