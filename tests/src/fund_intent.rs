// The entire funding flow: Route token + Reward token + Native.
// The Intent only moves -> `Funded` when all three legs are funded.
use anchor_lang::{AccountDeserialize, AnchorSerialize, InstructionData, ToAccountMetas};
use anyhow::Result;
use solana_sdk::{
    instruction::Instruction, message::Message, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::Transaction,
};

use eco_routes::{
    instruction as ix,
    instructions::{NativeToFund, TokenToFund},
    state::*,
};

use crate::{common::*, utils::init_svm};

fn fund_spl(
    args: eco_routes::instructions::fund_intent_spl::FundIntentSplArgs,
    source: Pubkey,
    mint: Pubkey,
    funder: Pubkey,
    payer: Pubkey,
    intent_hash: [u8; 32],
    reward_token: bool,
) -> Instruction {
    Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::FundIntentSpl {
            intent: Intent::pda(intent_hash).0,
            source_token: source,
            destination_token: Pubkey::find_program_address(
                &[
                    match reward_token {
                        true => b"reward-token",
                        false => b"routed-token",
                    },
                    args.salt.as_ref(),
                    mint.as_ref(),
                ],
                &eco_routes::ID,
            )
            .0,
            mint,
            funder: funder,
            payer: payer,
            system_program: solana_sdk::system_program::ID,
            token_program: spl_token::id(),
        }
        .to_account_metas(None),
        data: ix::FundIntentSpl { args }.data(),
    }
}

#[ignore]
#[test]
fn funding_complete() -> Result<()> {
    let mut svm = init_svm();

    let creator = Keypair::new();
    let funder = Keypair::new();
    let payer = Keypair::new();
    let route_mint = Keypair::new();
    let reward_mint = Keypair::new();
    airdrop_initial_amount(&mut svm, &creator.pubkey())?;
    airdrop_initial_amount(&mut svm, &funder.pubkey())?;
    airdrop_initial_amount(&mut svm, &payer.pubkey())?;

    let route_source = spl_associated_token_account::get_associated_token_address(
        &funder.pubkey(),
        &route_mint.pubkey(),
    );
    let reward_source = spl_associated_token_account::get_associated_token_address(
        &funder.pubkey(),
        &reward_mint.pubkey(),
    );

    let route_mint = write_mint_with_distribution(
        &mut svm,
        &route_mint,
        6,
        vec![(&funder.pubkey(), &route_source, 1_000_000)],
    )?;

    let reward_mint = write_mint_with_distribution(
        &mut svm,
        &reward_mint,
        6,
        vec![(&funder.pubkey(), &reward_source, 500_000)],
    )?;

    let salt = [7u8; 32];
    let intent_hash = [8u8; 32];
    let deadline = now_ts(&svm) + 3_600;

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
                tokens_funded: 0,
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
                tokens_funded: 0,
                native_reward: 2_000_000,
                native_funded: 0,
            },
            solver: Pubkey::default(),
            bump: Intent::pda(intent_hash).1,
        }
        .try_to_vec()?
        .as_slice(),
        creator.pubkey(),
    )?;

    let args_route = eco_routes::instructions::fund_intent_spl::FundIntentSplArgs {
        salt,
        amount: 500_000,
        token_to_fund: TokenToFund::Route(0),
    };

    let transaction_1 = Transaction::new(
        &[&funder],
        Message::new(
            &[fund_spl(
                args_route,
                route_source,
                route_mint,
                funder.pubkey(),
                payer.pubkey(),
                intent_hash,
                false,
            )],
            Some(&funder.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction_1)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let mut intent_account = svm
        .get_account(&Intent::pda(intent_hash).0)
        .ok_or(anyhow::anyhow!("Intent not found"))?;

    let mut intent: Intent = Intent::try_deserialize(&mut intent_account.data.as_slice())?;

    assert_eq!(intent.status, IntentStatus::Initialized);

    let args_route_2 = eco_routes::instructions::fund_intent_spl::FundIntentSplArgs {
        salt,
        amount: 500_000,
        token_to_fund: TokenToFund::Route(0),
    };
    let args_reward = eco_routes::instructions::fund_intent_spl::FundIntentSplArgs {
        salt,
        amount: 500_000,
        token_to_fund: TokenToFund::Reward(0),
    };
    let args_native = eco_routes::instructions::fund_intent_native::FundIntentNativeArgs {
        salt,
        amount: 2_000_000,
        native_to_fund: NativeToFund::Reward,
    };

    let native_ix = Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::FundIntentNative {
            intent: Intent::pda(intent_hash).0,
            funder: funder.pubkey(),
            payer: payer.pubkey(),
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: ix::FundIntentNative { args: args_native }.data(),
    };

    let transaction_2 = Transaction::new(
        &[&funder],
        Message::new(
            &[
                fund_spl(
                    args_route_2,
                    route_source,
                    route_mint,
                    funder.pubkey(),
                    payer.pubkey(),
                    intent_hash,
                    false,
                ),
                fund_spl(
                    args_reward,
                    reward_source,
                    reward_mint,
                    funder.pubkey(),
                    payer.pubkey(),
                    intent_hash,
                    true,
                ),
                native_ix,
            ],
            Some(&funder.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction_2)
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    intent_account = svm
        .get_account(&Intent::pda(intent_hash).0)
        .ok_or(anyhow::anyhow!("Intent not found"))?;

    intent = Intent::try_deserialize(&mut intent_account.data.as_slice())?;
    assert_eq!(intent.status, IntentStatus::Funded);

    Ok(())
}
