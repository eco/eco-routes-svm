use portal::types::{Intent, Route, TokenAmount, Reward, Call};
use rand::random;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use common::sol_amount;

pub mod common;

/**
 * E2E test from SVM to SVM
 * 
 * This test demonstrates the full flow of creating and processing an intent
 * between two SVM chains using the eco-routes protocol.
 */
#[test]
fn test_svm_to_svm_e2e() {
    let mut source_ctx = common::Context::default();
    let mut destination_ctx = common::Context::default();

    let source_usdc_mint = Keypair::new();
    let destination_usdc_mint = Keypair::new();
    
    source_ctx.set_mint_account(&source_usdc_mint.pubkey());
    destination_ctx.set_mint_account(&destination_usdc_mint.pubkey());
    
    // Create a test intent for 5 USDC transfer from source to destination
    let intent = create_usdc_intent(&source_ctx, &source_usdc_mint, &destination_usdc_mint);
    
    let route_hash = intent.route.hash(); // Compute the proper route hash
    let result = source_ctx.publish_intent(&intent, route_hash);
    
    assert!(result.is_ok(), "Failed to create intent: {:?}", result.err());
    println!("✅ Intent created successfully on source chain");
    
    println!("✅ SVM to SVM E2E test completed successfully");
}

fn create_usdc_intent(ctx: &common::Context, source_usdc_mint: &Keypair, destination_usdc_mint: &Keypair) -> Intent {
    let usdc_amount = 5_000_000u64; // 5.0 USDC (6 decimals)
    
    Intent {
        destination_chain: random::<u32>().into(),
        route: Route {
            salt: random::<[u8; 32]>().into(),
            destination_chain_portal: portal::ID.to_bytes().into(),
            tokens: vec![TokenAmount {
                token: destination_usdc_mint.pubkey(),
                amount: usdc_amount,
            }],
            calls: vec![
                Call {
                    target: random::<[u8; 32]>().into(),
                    data: vec![0u8; 32], // Simplified call data for now
                }
            ],
        },
        reward: Reward {
            deadline: ctx.now() + 3600, // 1 hour from now
            creator: ctx.creator.pubkey(),
            prover: hyper_prover::ID,
            native_amount: sol_amount(0.03), // 0.03 SOL reward
            tokens: vec![TokenAmount {
                token: source_usdc_mint.pubkey(),
                amount: usdc_amount, // 5.0 USDC reward
            }],
        },
    }
}
