use anchor_lang::{InstructionData, ToAccountMetas};
use anyhow::Result;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

use crate::{common::*, utils::init_svm};
use eco_routes::{instruction as ix, state::*};

// #[ignore]
#[test]
fn full_intent_creation() -> Result<()> {
    let mut svm = init_svm();

    let creator = Keypair::new();
    let payer = Keypair::new();
    let mint = Keypair::new();

    airdrop_initial_amount(&mut svm, &creator.pubkey())?;

    Ok(())
    // airdrop_initial_amount(&mut svm, &payer.pubkey())?;

    // let route_mint = write_mint_with_distribution(&mut svm, &mint, 6, vec![])?;
    // let reward_mint = write_mint_with_distribution(&mut svm, &mint, 6, vec![])?;

    // let salt = [1u8; 32];
    // let intent_hash = [2u8; 32];
    // let calls_root = [3u8; 32];
    // let route_root = [4u8; 32];
    // let inbox = [5u8; 32];
    // let deadline = now_ts(&svm) + 1_800; // 30 min

    // let route_tokens = vec![TokenAmount {
    //     mint: route_mint,
    //     amount: 1_000_000,
    // }];
    // let reward_tokens = vec![TokenAmount {
    //     mint: reward_mint,
    //     amount: 500_000,
    // }];

    // let calls = vec![Call {
    //     destination: [9u8; 32],
    //     calldata: b"\x01\x02payload".to_vec(),
    // }];

    // let args = eco_routes::instructions::publish_intent::PublishIntentArgs {
    //     salt,
    //     intent_hash,
    //     destination_domain_id: 123,
    //     inbox,
    //     route_tokens,
    //     calls: calls.clone(),
    //     reward_tokens,
    //     native_reward: 2_000_000,
    //     deadline,
    //     calls_root,
    //     route_root,
    // };

    // let instruction = Instruction {
    //     program_id: eco_routes::ID,
    //     accounts: eco_routes::accounts::PublishIntent {
    //         intent: Intent::pda(intent_hash).0,
    //         creator: creator.pubkey(),
    //         payer: payer.pubkey(),
    //         system_program: solana_system_interface::program::ID,
    //     }
    //     .to_account_metas(None),
    //     data: ix::PublishIntent { args }.data(),
    // };

    // let transaction = Transaction::new(
    //     &[&payer],
    //     Message::new(
    //         &[solana_system_interface::instruction::transfer(
    //             &payer.pubkey(),
    //             &creator.pubkey(),
    //             1,
    //         )],
    //         Some(&payer.pubkey()),
    //     ),
    //     svm.latest_blockhash(),
    // );

    // println!("transaction: {:?}", transaction);

    // svm.send_transaction(transaction)
    //     .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    // // let intent_account = svm
    // //     .get_account(&Intent::pda(intent_hash).0)
    // //     .ok_or(anyhow::anyhow!("Intent not found"))?;

    // // let intent: Intent = Intent::try_deserialize(&mut intent_account.data.as_slice())?;

    // // assert_eq!(intent.route.tokens[0].mint, route_mint);
    // // assert_eq!(intent.reward.tokens[0].mint, reward_mint);
    // // assert_eq!(intent.route.calls, calls);
    // // assert_eq!(intent.reward.native_reward, 2_000_000);

    // Ok(())
}
