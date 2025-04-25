use anchor_lang::{AccountDeserialize, AnchorSerialize, InstructionData, ToAccountMetas};
use anyhow::Result;
use solana_sdk::{
    instruction::Instruction, message::Message, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::Transaction,
};

use eco_routes::{
    instruction as ix,
    instructions::{NativeToClaim, TokenToClaim},
    state::*,
};

use crate::{common::*, utils::init_svm};

fn claim_spl(
    args: eco_routes::instructions::claim_intent_spl::ClaimIntentSplArgs,
    destination: Pubkey,
    mint: Pubkey,
    claimer: Pubkey,
    payer: Pubkey,
    intent_hash: [u8; 32],
    reward_token: bool,
) -> Instruction {
    Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::ClaimIntentSpl {
            intent: Intent::pda(intent_hash).0,
            destination_token: destination,
            source_token: Pubkey::find_program_address(
                &[
                    match reward_token {
                        true => b"reward-token",
                        false => b"routed-token",
                    },
                    args.intent_hash.as_ref(),
                    mint.as_ref(),
                ],
                &eco_routes::ID,
            )
            .0,
            mint,
            claimer: claimer,
            payer: payer,
            system_program: solana_sdk::system_program::ID,
            token_program: spl_token::id(),
        }
        .to_account_metas(None),
        data: ix::ClaimIntentSpl { args }.data(),
    }
}

#[ignore]
#[test]
fn funding_complete() -> Result<()> {
    let mut svm = init_svm();

    let creator = Keypair::new();
    let claimer = Keypair::new(); // this is the solver
    let payer = Keypair::new();
    let route_mint = Keypair::new();
    let reward_mint = Keypair::new();
    airdrop_initial_amount(&mut svm, &creator.pubkey())?;
    airdrop_initial_amount(&mut svm, &claimer.pubkey())?;
    airdrop_initial_amount(&mut svm, &payer.pubkey())?;

    let salt = [7u8; 32];
    let intent_hash = [8u8; 32];
    let deadline = now_ts(&svm) + 3_600;

    let route_source = Pubkey::find_program_address(
        &[b"routed-token", &intent_hash, &route_mint.pubkey().as_ref()],
        &eco_routes::ID,
    )
    .0;

    let route_destination = spl_associated_token_account::get_associated_token_address(
        &claimer.pubkey(),
        &route_mint.pubkey(),
    );

    let reward_source = Pubkey::find_program_address(
        &[
            b"reward-token",
            &intent_hash,
            &reward_mint.pubkey().as_ref(),
        ],
        &eco_routes::ID,
    )
    .0;

    let reward_destination = spl_associated_token_account::get_associated_token_address(
        &claimer.pubkey(),
        &reward_mint.pubkey(),
    );

    let route_mint = write_mint_with_distribution(
        &mut svm,
        &route_mint,
        6,
        vec![
            (&Intent::pda(intent_hash).0, &route_source, 1_000_000),
            (&claimer.pubkey(), &reward_destination, 0),
        ],
    )?;

    let reward_mint = write_mint_with_distribution(
        &mut svm,
        &reward_mint,
        6,
        vec![
            (&Intent::pda(intent_hash).0, &reward_source, 500_000),
            (&claimer.pubkey(), &reward_destination, 0),
        ],
    )?;

    write_account(
        &mut svm,
        Intent::pda(intent_hash).0,
        Intent {
            salt,
            intent_hash,
            status: IntentStatus::Initialized,
            creator: creator.pubkey(),
            prover: creator.pubkey(),
            deadline,
            route: Route {
                source_domain_id: eco_routes::hyperlane::DOMAIN_ID,
                destination_domain_id: 777,
                inbox: [11u8; 32],
                prover: creator.pubkey(),
                calls_root: [1u8; 32],
                route_root: [2u8; 32],
                tokens: vec![TokenAmount {
                    mint: route_mint,
                    amount: 1_000_000,
                }],
                tokens_funded: 1,
                calls: vec![Call {
                    destination: [3u8; 32],
                    calldata: vec![1u8],
                }],
            },
            reward: Reward {
                tokens: vec![TokenAmount {
                    mint: reward_mint,
                    amount: 500_000,
                }],
                tokens_funded: 1,
                native_reward: 2_000_000,
                native_funded: 2_000_000,
            },
            solver: claimer.pubkey(),
            bump: Intent::pda(intent_hash).1,
        }
        .try_to_vec()?
        .as_slice(),
        creator.pubkey(),
    )?;

    let args_route = eco_routes::instructions::claim_intent_spl::ClaimIntentSplArgs {
        intent_hash,
        token_to_claim: TokenToClaim::Route(0),
    };
    let args_reward = eco_routes::instructions::claim_intent_spl::ClaimIntentSplArgs {
        intent_hash,
        token_to_claim: TokenToClaim::Reward(0),
    };
    let args_native = eco_routes::instructions::claim_intent_native::ClaimIntentNativeArgs {
        intent_hash,
        native_to_claim: NativeToClaim::Reward,
    };

    let native_ix = Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::ClaimIntentNative {
            intent: Intent::pda(intent_hash).0,
            claimer: claimer.pubkey(),
            payer: payer.pubkey(),
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: ix::ClaimIntentNative { args: args_native }.data(),
    };

    let transaction_2 = Transaction::new(
        &[&claimer],
        Message::new(
            &[
                claim_spl(
                    args_route,
                    route_destination,
                    route_mint,
                    claimer.pubkey(),
                    payer.pubkey(),
                    intent_hash,
                    false,
                ),
                claim_spl(
                    args_reward,
                    reward_source,
                    reward_mint,
                    claimer.pubkey(),
                    payer.pubkey(),
                    intent_hash,
                    true,
                ),
                native_ix,
            ],
            Some(&claimer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction_2)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let intent_account = svm
        .get_account(&Intent::pda(intent_hash).0)
        .ok_or(anyhow::anyhow!("Intent not found"))?;

    let intent: Intent = Intent::try_deserialize(&mut intent_account.data.as_slice())?;
    assert_eq!(intent.status, IntentStatus::Claimed);

    Ok(())
}
