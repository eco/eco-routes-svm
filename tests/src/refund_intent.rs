use anchor_lang::{AccountDeserialize, AnchorSerialize, InstructionData, ToAccountMetas};
use anyhow::Result;
use solana_sdk::{
    instruction::Instruction, message::Message, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::Transaction,
};

use eco_routes::{
    instruction as ix,
    instructions::{NativeToRefund, TokenToRefund},
    state::*,
};

use crate::{common::*, utils::init_svm};

fn refund_spl(
    args: eco_routes::instructions::refund_intent_spl::RefundIntentSplArgs,
    destination: Pubkey,
    mint: Pubkey,
    refundee: Pubkey,
    payer: Pubkey,
    intent_hash: [u8; 32],
    reward_token: bool,
) -> Instruction {
    Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::RefundIntentSpl {
            intent: Intent::pda(intent_hash).0,
            source_token: Pubkey::find_program_address(
                &[
                    if reward_token {
                        b"reward-token"
                    } else {
                        b"routed-token"
                    },
                    intent_hash.as_ref(),
                    mint.as_ref(),
                ],
                &eco_routes::ID,
            )
            .0,
            destination_token: destination,
            mint,
            refundee,
            payer,
            system_program: solana_sdk::system_program::ID,
            token_program: spl_token::id(),
        }
        .to_account_metas(None),
        data: ix::RefundIntentSpl { args }.data(),
    }
}

#[ignore]
#[test]
fn refunding_complete() -> Result<()> {
    let mut svm = init_svm();

    let creator_keypair = Keypair::new();
    let payer_keypair = Keypair::new();
    airdrop_initial_amount(&mut svm, &creator_keypair.pubkey())?;
    airdrop_initial_amount(&mut svm, &payer_keypair.pubkey())?;

    let route_mint_keypair = Keypair::new();
    let reward_mint_keypair = Keypair::new();

    let intent_salt = [7u8; 32];
    let intent_hash = [8u8; 32];
    let route_vault_pda = Pubkey::find_program_address(
        &[
            b"routed-token",
            intent_salt.as_ref(),
            route_mint_keypair.pubkey().as_ref(),
        ],
        &eco_routes::ID,
    )
    .0;
    let reward_vault_pda = Pubkey::find_program_address(
        &[
            b"reward-token",
            intent_salt.as_ref(),
            reward_mint_keypair.pubkey().as_ref(),
        ],
        &eco_routes::ID,
    )
    .0;

    let creator_route_ata = spl_associated_token_account::get_associated_token_address(
        &creator_keypair.pubkey(),
        &route_mint_keypair.pubkey(),
    );
    let creator_reward_ata = spl_associated_token_account::get_associated_token_address(
        &creator_keypair.pubkey(),
        &reward_mint_keypair.pubkey(),
    );

    write_mint_with_distribution(
        &mut svm,
        &route_mint_keypair,
        6,
        vec![
            (&Intent::pda(intent_hash).0, &route_vault_pda, 1_000_000), // vault funded
            (&creator_keypair.pubkey(), &creator_route_ata, 0),         // destination empties
        ],
    )?;
    write_mint_with_distribution(
        &mut svm,
        &reward_mint_keypair,
        6,
        vec![
            (&Intent::pda(intent_hash).0, &reward_vault_pda, 500_000),
            (&creator_keypair.pubkey(), &creator_reward_ata, 0),
        ],
    )?;

    let native_reward_lamports = 2_000_000;

    svm.airdrop(&Intent::pda(intent_hash).0, native_reward_lamports)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let deadline_timestamp = now_ts(&svm) - 3600; // already expired

    write_account(
        &mut svm,
        Intent::pda(intent_hash).0,
        Intent {
            salt: intent_salt,
            intent_hash,
            status: IntentStatus::Funded,
            creator: creator_keypair.pubkey(),
            prover: Pubkey::default(),
            deadline: deadline_timestamp,
            route: Route {
                source_domain_id: eco_routes::hyperlane::DOMAIN_ID,
                destination_domain_id: 777,
                inbox: [0u8; 32],
                prover: Pubkey::default(),
                calls_root: [0u8; 32],
                route_root: [0u8; 32],
                tokens: vec![TokenAmount {
                    mint: route_mint_keypair.pubkey(),
                    amount: 1_000_000,
                }],
                tokens_funded: 1,
                calls: vec![],
            },
            reward: Reward {
                tokens: vec![TokenAmount {
                    mint: reward_mint_keypair.pubkey(),
                    amount: 500_000,
                }],
                tokens_funded: 1,
                native_reward: native_reward_lamports,
                native_funded: native_reward_lamports,
            },
            solver: Pubkey::default(),
            bump: Intent::pda(intent_hash).1,
        }
        .try_to_vec()?
        .as_slice(),
        creator_keypair.pubkey(),
    )?;

    let route_args = eco_routes::instructions::refund_intent_spl::RefundIntentSplArgs {
        intent_hash,
        token_to_refund: TokenToRefund::Route(0),
    };

    let transaction_1 = Transaction::new(
        &[&creator_keypair, &payer_keypair],
        Message::new(
            &[refund_spl(
                route_args,
                creator_route_ata,
                route_mint_keypair.pubkey(),
                creator_keypair.pubkey(),
                payer_keypair.pubkey(),
                intent_hash,
                false,
            )],
            Some(&creator_keypair.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction_1)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let mut intent_account = svm
        .get_account(&Intent::pda(intent_hash).0)
        .expect("intent PDA missing");

    let mut intent: Intent = Intent::try_deserialize(&mut intent_account.data.as_slice())?;

    assert_eq!(intent.status, IntentStatus::Funded);

    let refund_args = eco_routes::instructions::refund_intent_spl::RefundIntentSplArgs {
        intent_hash,
        token_to_refund: TokenToRefund::Route(0),
    };

    let refund_native_ix = Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::RefundIntentNative {
            intent: Intent::pda(intent_hash).0,
            refundee: creator_keypair.pubkey(),
            payer: payer_keypair.pubkey(),
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: ix::RefundIntentNative {
            args: eco_routes::instructions::refund_intent_native::RefundIntentNativeArgs {
                intent_hash,
                native_to_refund: NativeToRefund::Reward,
            },
        }
        .data(),
    };

    let transaction_2 = Transaction::new(
        &[&creator_keypair, &payer_keypair],
        Message::new(
            &[
                refund_spl(
                    refund_args,
                    creator_reward_ata,
                    reward_mint_keypair.pubkey(),
                    creator_keypair.pubkey(),
                    payer_keypair.pubkey(),
                    intent_hash,
                    true,
                ),
                refund_native_ix,
            ],
            Some(&creator_keypair.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction_2)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    intent_account = svm
        .get_account(&Intent::pda(intent_hash).0)
        .expect("intent PDA missing");

    intent = Intent::try_deserialize(&mut intent_account.data.as_slice())?;

    assert_eq!(intent.status, IntentStatus::Refunded);

    Ok(())
}
