use anchor_lang::prelude::borsh;
use anchor_lang::AccountDeserialize;
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::polymer;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::polymer_prover_context::intent_fulfilled_result;

pub mod common;

#[test]
fn mock_polymer_prover_is_at_the_id_polymer_prover_targets() {
    assert_eq!(mock_polymer_prover::ID, polymer::POLYMER_PROVER_ID);
}

#[test]
fn mock_polymer_load_result_roundtrip() {
    let mut ctx = common::Context::default();
    let authority = Keypair::new();
    ctx.polymer_prover()
        .polymer_create_accounts(&authority)
        .unwrap();

    let result = intent_fulfilled_result([0xab; 20], 7, 8453, vec![1u8; 72]);
    ctx.polymer_prover()
        .polymer_load_result(&authority, &result)
        .unwrap();

    let cache = ctx
        .get_account(&polymer::cache_pda(&authority.pubkey()).0)
        .unwrap();
    let cache = mock_polymer_prover::ProofCacheAccount::try_deserialize(&mut cache.data.as_slice())
        .unwrap();
    assert_eq!(cache.cache, borsh::to_vec(&result).unwrap());
    let _: ValidationResultAccount = ctx
        .account(&polymer::result_pda(&authority.pubkey()).0)
        .unwrap();
}
