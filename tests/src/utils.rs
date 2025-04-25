use anyhow::Result;
use solana_compute_budget::compute_budget::ComputeBudget;

use litesvm::LiteSVM;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

use crate::multisig_ism_stub;

const MAILBOX_BIN: &[u8] = include_bytes!("../../bins/mailbox.so");
const MULTISIG_ISM_BIN: &[u8] = include_bytes!("../../bins/multisig_ism.so");
const ECO_ROUTES_BIN: &[u8] = include_bytes!("../../target/deploy/eco_routes.so");

pub fn init_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();

    svm.airdrop(&Keypair::new().pubkey(), 1).unwrap();

    svm.add_program(eco_routes::ID, &ECO_ROUTES_BIN);
    svm.add_program(eco_routes::hyperlane::MAILBOX_ID, &MAILBOX_BIN);
    svm.add_program(eco_routes::hyperlane::MULTISIG_ISM_ID, &MULTISIG_ISM_BIN);

    multisig_ism_stub::write_domain_data(
        &mut svm,
        eco_routes::hyperlane::MULTISIG_ISM_ID,
        eco_routes::hyperlane::DOMAIN_ID,
        multisig_ism_stub::ValidatorsAndThreshold {
            validators: vec![],
            threshold: 0,
        },
    )
    .unwrap();

    svm
}
